//! Phase correction and automatic phasing.
//!
//! The correction is `S_k · e^{−i(φ0 + φ1·k/N)}` with `k` counted from the left (high-ppm) edge
//! and the phases in degrees.
//!
//! **Automatic phasing** minimises the ACME objective of Chen, Weng, Goh and Garland (J. Magn.
//! Reson. 158, 164–168, 2002) — the Shannon entropy of the normalised absolute first derivative
//! of the real spectrum plus a penalty on negative intensities — restricted to the signal
//! regions, with a weak prior against large first-order phases:
//!
//! - signal regions: points whose magnitude `|S_k|` exceeds 5 × the noise SD of the real part
//!   ([`crate::nmr::peaks::noise_sd`]), widened by 16 points on each side; when fewer than 16
//!   points qualify, the whole spectrum is used;
//! - with `R` the real part scaled so that `max|S| = 1`, `h` the absolute differences of
//!   neighbouring signal-region points, `p = h / Σh`:
//!   `objective = −Σ p ln p + 1000 · Σ_{R<0} R² + 0.05 · (φ1 / 360°)²`;
//! - a grid over `φ0 ∈ [0°, 360°)` (15° steps) × `φ1 ∈ [−1440°, 1440°]` (45° steps), evaluated on
//!   at most 8192 evenly strided signal points, picks four start points; a Nelder–Mead simplex
//!   refines each on all signal points, with `φ1` confined to ±1440°.
//!
//! First-order phases beyond four turns (an uncorrected receiver delay of more than four
//! points) are not found.
//!
//! [`negative_fraction`] measures how well a phase pair works: the share of the signal-region
//! real intensity that is negative after a first-order baseline is removed (0 for a perfectly
//! absorptive spectrum with positive lines).
//! `PhaseMode::Default` uses it to reject stored phases that do not fit the data.

use crate::fft::Complex;
use crate::nmr::peaks::noise_sd;

/// Weight of the negative-intensity penalty.
pub const NEGATIVE_PENALTY: f64 = 1000.0;
/// Weight of the `(φ1/360°)²` prior.
pub const FIRST_ORDER_PRIOR: f64 = 0.05;
/// Largest |φ1| searched, degrees.
pub const MAX_FIRST_ORDER: f64 = 1440.0;

/// Apply `S_k · e^{−i(φ0 + φ1·k/N)}` in place (degrees).
pub fn apply_phase(s: &mut [Complex], phase0_deg: f64, phase1_deg: f64) {
    let n = s.len();
    if n == 0 {
        return;
    }
    let a = -phase0_deg.to_radians();
    let b = -phase1_deg.to_radians() / n as f64;
    // exact rotation every 1024 points keeps the recurrence from drifting
    let step = Complex::from_phase(b);
    let mut w = Complex::from_phase(a);
    for (k, v) in s.iter_mut().enumerate() {
        if k % 1024 == 0 {
            w = Complex::from_phase(a + b * k as f64);
        }
        *v *= w;
        w *= step;
    }
}

/// Indices of the signal regions of `s` (module docs), ascending.
pub fn signal_points(s: &[Complex]) -> Vec<usize> {
    let n = s.len();
    let re: Vec<f64> = s.iter().map(|v| v.re).collect();
    let sd = noise_sd(&re);
    let thr = 5.0 * sd;
    let hits: Vec<usize> = if sd > 0.0 {
        (0..n).filter(|&k| s[k].abs() > thr).collect()
    } else {
        Vec::new()
    };
    if hits.len() < 16 {
        return (0..n).collect();
    }
    let mut keep = vec![false; n];
    for &k in &hits {
        let a = k.saturating_sub(16);
        let b = (k + 16).min(n - 1);
        for v in &mut keep[a..=b] {
            *v = true;
        }
    }
    (0..n).filter(|&k| keep[k]).collect()
}

/// The objective of the module docs over the points `idx` of `s`.
fn objective(s: &[Complex], idx: &[usize], scale: f64, p0: f64, p1: f64) -> f64 {
    let n = s.len() as f64;
    let a = -p0.to_radians();
    let b = -p1.to_radians() / n;
    let mut prev: Option<(usize, f64)> = None;
    let mut diffs = Vec::with_capacity(idx.len());
    let mut penalty = 0.0;
    for &k in idx {
        let r = (s[k] * Complex::from_phase(a + b * k as f64)).re / scale;
        if r < 0.0 {
            penalty += r * r;
        }
        if let Some((pk, pr)) = prev {
            // neighbours in the original spectrum (or in the strided subset)
            if k > pk {
                diffs.push((r - pr).abs());
            }
        }
        prev = Some((k, r));
    }
    let total: f64 = diffs.iter().sum();
    let entropy = if total > 0.0 {
        diffs
            .iter()
            .filter(|&&h| h > 0.0)
            .map(|&h| {
                let p = h / total;
                -p * p.ln()
            })
            .sum()
    } else {
        0.0
    };
    let prior = FIRST_ORDER_PRIOR * (p1 / 360.0).powi(2);
    entropy + NEGATIVE_PENALTY * penalty + prior
}

/// The ACME objective of the whole spectrum `s` (every `stride`-th point) at `(φ0, φ1)`,
/// without the signal-region restriction and prior. Kept for comparisons and tests.
pub fn acme_objective(s: &[Complex], phase0_deg: f64, phase1_deg: f64, stride: usize) -> f64 {
    let stride = stride.max(1);
    let idx: Vec<usize> = (0..s.len()).step_by(stride).collect();
    let scale = s.iter().map(|v| v.abs()).fold(0.0_f64, f64::max);
    if !(scale > 0.0 && scale.is_finite()) || idx.len() < 2 {
        return 0.0;
    }
    objective(s, &idx, scale, phase0_deg, phase1_deg)
        - FIRST_ORDER_PRIOR * (phase1_deg / 360.0).powi(2)
}

/// Nelder–Mead minimisation of `f` in two dimensions from `start` with initial step `step`.
fn nelder_mead(
    f: &dyn Fn(f64, f64) -> f64,
    start: (f64, f64),
    step: (f64, f64),
) -> (f64, f64, f64) {
    let mut pts = [
        (start.0, start.1),
        (start.0 + step.0, start.1),
        (start.0, start.1 + step.1),
    ];
    let mut vals = pts.map(|p| f(p.0, p.1));
    for _ in 0..400 {
        let mut idx = [0usize, 1, 2];
        idx.sort_by(|&i, &j| vals[i].total_cmp(&vals[j]));
        pts = idx.map(|i| pts[i]);
        vals = idx.map(|i| vals[i]);
        let spread = (pts[0].0 - pts[2].0).abs().max((pts[0].1 - pts[2].1).abs());
        if spread < 0.01 {
            break;
        }
        let c = (
            f64::midpoint(pts[0].0, pts[1].0),
            f64::midpoint(pts[0].1, pts[1].1),
        );
        let refl = (2.0 * c.0 - pts[2].0, 2.0 * c.1 - pts[2].1);
        let fr = f(refl.0, refl.1);
        if fr < vals[0] {
            let exp = (3.0 * c.0 - 2.0 * pts[2].0, 3.0 * c.1 - 2.0 * pts[2].1);
            let fe = f(exp.0, exp.1);
            if fe < fr {
                pts[2] = exp;
                vals[2] = fe;
            } else {
                pts[2] = refl;
                vals[2] = fr;
            }
        } else if fr < vals[1] {
            pts[2] = refl;
            vals[2] = fr;
        } else {
            let con = (f64::midpoint(c.0, pts[2].0), f64::midpoint(c.1, pts[2].1));
            let fc = f(con.0, con.1);
            if fc < vals[2] {
                pts[2] = con;
                vals[2] = fc;
            } else {
                for i in 1..3 {
                    pts[i] = (
                        f64::midpoint(pts[i].0, pts[0].0),
                        f64::midpoint(pts[i].1, pts[0].1),
                    );
                    vals[i] = f(pts[i].0, pts[i].1);
                }
            }
        }
    }
    let best = (0..3)
        .min_by(|&i, &j| vals[i].total_cmp(&vals[j]))
        .unwrap_or(0);
    (pts[best].0, pts[best].1, vals[best])
}

/// Wrap a phase into `[−180°, 180°)`.
pub fn wrap_degrees(p: f64) -> f64 {
    (p + 180.0).rem_euclid(360.0) - 180.0
}

/// Automatic phases `(φ0, φ1)` in degrees (module docs). `(0, 0)` for tiny or all-zero input.
pub fn autophase(s: &[Complex]) -> (f64, f64) {
    let n = s.len();
    if n < 4 {
        return (0.0, 0.0);
    }
    let scale = s.iter().map(|v| v.abs()).fold(0.0_f64, f64::max);
    if !(scale > 0.0 && scale.is_finite()) {
        return (0.0, 0.0);
    }
    let idx = signal_points(s);
    let stride = (idx.len() / 8192).max(1);
    let coarse: Vec<usize> = idx.iter().copied().step_by(stride).collect();
    let mut grid: Vec<(f64, f64, f64)> = Vec::new();
    let mut p1 = -MAX_FIRST_ORDER;
    while p1 <= MAX_FIRST_ORDER {
        let mut p0 = 0.0;
        while p0 < 360.0 {
            grid.push((p0, p1, objective(s, &coarse, scale, p0, p1)));
            p0 += 15.0;
        }
        p1 += 45.0;
    }
    grid.sort_by(|a, b| a.2.total_cmp(&b.2));
    let f = |a: f64, b: f64| {
        if b.abs() > MAX_FIRST_ORDER {
            return f64::INFINITY;
        }
        objective(s, &idx, scale, a, b)
    };
    let mut best = (0.0, 0.0, f64::INFINITY);
    for &(p0, p1, _) in grid.iter().take(4) {
        let r = nelder_mead(&f, (p0, p1), (10.0, 20.0));
        if r.2 < best.2 {
            best = r;
        }
    }
    if !best.2.is_finite() {
        return (0.0, 0.0);
    }
    (wrap_degrees(best.0), best.1)
}

/// Share of the signal-region real intensity that is negative after phasing by `(φ0, φ1)` and
/// removing a first-order baseline ([`crate::nmr::baseline::polynomial_baseline`], so a constant
/// offset does not count as negative signal).
pub fn negative_fraction(s: &[Complex], phase0_deg: f64, phase1_deg: f64) -> f64 {
    let mut t = s.to_vec();
    apply_phase(&mut t, phase0_deg, phase1_deg);
    let mut re: Vec<f64> = t.iter().map(|v| v.re).collect();
    if let Some(b) = crate::nmr::baseline::polynomial_baseline(&re, 1) {
        for (v, b) in re.iter_mut().zip(b) {
            *v -= b;
        }
    }
    let idx = signal_points(&t);
    let (mut pos, mut neg) = (0.0, 0.0);
    for k in idx {
        let r = re[k];
        if r > 0.0 {
            pos += r;
        } else {
            neg -= r;
        }
    }
    if pos + neg > 0.0 {
        neg / (pos + neg)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fft::fft_in_place;

    fn lorentz_spectrum(n: usize, centres: &[f64], p0: f64, p1: f64) -> Vec<Complex> {
        // complex Lorentzians (absorptive real part) then dephased by (p0, p1)
        let mut s: Vec<Complex> = (0..n)
            .map(|k| {
                let mut v = Complex::default();
                for &c in centres {
                    let x = k as f64 - c;
                    let w = 3.0;
                    v += Complex::new(w / (w * w + x * x), -x / (w * w + x * x));
                }
                v + Complex::new(
                    1e-4 * crate::nmr::baseline::tests::noise(k),
                    1e-4 * crate::nmr::baseline::tests::noise(k + n),
                )
            })
            .collect();
        // dephase: multiply by e^{+i(p0 + p1 k/n)} so apply_phase(p0, p1) restores it
        apply_phase(&mut s, -p0, -p1);
        s
    }

    #[test]
    fn apply_phase_round_trips() {
        let mut s = lorentz_spectrum(4096, &[1000.0], 0.0, 0.0);
        let orig = s.clone();
        apply_phase(&mut s, 37.0, -120.0);
        apply_phase(&mut s, -37.0, 120.0);
        for (a, b) in s.iter().zip(&orig) {
            assert!((a.re - b.re).abs() < 1e-9 && (a.im - b.im).abs() < 1e-9);
        }
    }

    #[test]
    fn autophase_recovers_known_phases() {
        for &(p0, p1) in &[(40.0, 0.0), (-120.0, 60.0), (170.0, -150.0), (10.0, 900.0)] {
            let s = lorentz_spectrum(8192, &[900.0, 3000.0, 5200.0, 7000.0], p0, p1);
            let (a, b) = autophase(&s);
            for k in [900.0, 3000.0, 7000.0] {
                let want = p0 + p1 * k / 8192.0;
                let got = a + b * k / 8192.0;
                let d = wrap_degrees(want - got).abs();
                assert!(d < 3.0, "({p0},{p1}) -> ({a},{b}): {d} deg off at {k}");
            }
            assert!(negative_fraction(&s, a, b) < 0.05);
            assert!(negative_fraction(&s, a + 180.0, b) > 0.9);
        }
    }

    #[test]
    fn degenerate_input_is_harmless() {
        assert_eq!(autophase(&[]), (0.0, 0.0));
        let z = vec![Complex::default(); 64];
        assert_eq!(autophase(&z), (0.0, 0.0));
        assert!(negative_fraction(&z, 0.0, 0.0).abs() < f64::EPSILON);
        let mut v = vec![Complex::new(1.0, 0.0); 64];
        fft_in_place(&mut v, false).unwrap();
        let (a, b) = autophase(&v);
        assert!(a.is_finite() && b.is_finite());
        assert!(acme_objective(&v, 0.0, 0.0, 1).is_finite());
    }
}
