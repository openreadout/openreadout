//! Butterworth band-pass filtering, zero phase.
//!
//! [`bandpass_sos`] designs a digital Butterworth band-pass of order `n` (2n poles) as
//! second-order sections: the analog low-pass prototype poles `e^{iπ(2k+n+1)/(2n)}` are moved to
//! the band `[f1, f2]` (pre-warped `ω = 2·fs·tan(π f / fs)`) by the low-pass → band-pass
//! substitution `s → (s² + ω0²)/(s·B)`, then mapped to z by the bilinear transform. Each section
//! gets one zero at z = 1 and one at z = −1; the gain is normalised to 1 at the geometric centre
//! frequency. This is the textbook construction (e.g. Oppenheim & Schafer, ch. 7).
//!
//! [`filtfilt`] runs the sections forward and backward (zero phase, squared magnitude
//! response). The signal is extended at both ends by an odd reflection of
//! `min(3 × (2·sections + 1), len − 1)` points about the end values, and each section starts from
//! its steady state for a constant input equal to the first extended value, so a DC offset does
//! not ring at the edges.

use crate::fft::Complex;

/// One biquad `(b0, b1, b2, a1, a2)` with `a0 = 1`.
pub type Section = [f64; 5];

fn cdiv(a: Complex, b: Complex) -> Complex {
    let d = b.re * b.re + b.im * b.im;
    Complex::new(
        (a.re * b.re + a.im * b.im) / d,
        (a.im * b.re - a.re * b.im) / d,
    )
}

fn csqrt(z: Complex) -> Complex {
    let r = z.abs();
    let re = f64::midpoint(r, z.re).max(0.0).sqrt();
    let im = ((r - z.re) / 2.0).max(0.0).sqrt();
    Complex::new(re, if z.im < 0.0 { -im } else { im })
}

/// Band-pass Butterworth sections (module docs). `None` when the band is not inside
/// `(0, fs/2)` or `order` is 0 or above 10.
pub fn bandpass_sos(order: usize, low_hz: f64, high_hz: f64, fs: f64) -> Option<Vec<Section>> {
    if order == 0 || order > 10 || !(fs > 0.0) {
        return None;
    }
    if !(low_hz > 0.0 && high_hz > low_hz && high_hz < fs / 2.0) {
        return None;
    }
    let warp = |f: f64| 2.0 * fs * (std::f64::consts::PI * f / fs).tan();
    let (w1, w2) = (warp(low_hz), warp(high_hz));
    let bw = w2 - w1;
    let w0sq = w1 * w2;
    let n = order as f64;
    // all 2n analog band-pass poles: s^2 - p·B·s + w0^2 = 0 for each prototype pole p
    let mut poles: Vec<Complex> = Vec::new();
    for k in 0..order {
        let th = std::f64::consts::PI * (2.0 * k as f64 + n + 1.0) / (2.0 * n);
        let pb = Complex::from_phase(th).scale(bw); // prototype pole (left half plane) × B
        let disc = pb * pb - Complex::new(4.0 * w0sq, 0.0);
        let r = csqrt(disc);
        poles.push((pb + r).scale(0.5));
        poles.push((pb - r).scale(0.5));
    }
    let two_fs = 2.0 * fs;
    let to_z = |s: Complex| {
        cdiv(
            Complex::new(two_fs + s.re, s.im),
            Complex::new(two_fs - s.re, -s.im),
        )
    };
    let eps = 1e-9 * (w0sq.sqrt() + bw);
    // complex poles: one section per conjugate pair (take the upper member); real poles: paired
    let mut sos: Vec<Section> = Vec::new();
    let mut reals: Vec<f64> = Vec::new();
    for &p in &poles {
        if p.im > eps {
            let z = to_z(p);
            sos.push([1.0, 0.0, -1.0, -2.0 * z.re, z.re * z.re + z.im * z.im]);
        } else if p.im.abs() <= eps {
            reals.push(to_z(Complex::new(p.re, 0.0)).re);
        }
    }
    reals.sort_by(f64::total_cmp);
    for pair in reals.chunks(2) {
        if let [z1, z2] = *pair {
            sos.push([1.0, 0.0, -1.0, -(z1 + z2), z1 * z2]);
        }
    }
    if sos.len() != order {
        return None;
    }
    // normalise the gain at the centre frequency
    let wc = 2.0 * (w0sq.sqrt() / two_fs).atan();
    let zc1 = Complex::from_phase(-wc);
    let zc2 = Complex::from_phase(-2.0 * wc);
    let mut g = Complex::new(1.0, 0.0);
    for s in &sos {
        let num = Complex::new(s[0], 0.0) + zc1.scale(s[1]) + zc2.scale(s[2]);
        let den = Complex::new(1.0, 0.0) + zc1.scale(s[3]) + zc2.scale(s[4]);
        g *= cdiv(num, den);
    }
    let gain = g.abs();
    if !(gain > 0.0 && gain.is_finite()) {
        return None;
    }
    let per = gain.powf(-1.0 / sos.len() as f64);
    for s in &mut sos {
        s[0] *= per;
        s[1] *= per;
        s[2] *= per;
    }
    Some(sos)
}

/// Direct form II transposed, in place, with initial states `zi` (updated).
fn run(sos: &[Section], x: &mut [f64], zi: &mut [[f64; 2]]) {
    for (s, z) in sos.iter().zip(zi.iter_mut()) {
        let [b0, b1, b2, a1, a2] = *s;
        for v in x.iter_mut() {
            let xin = *v;
            let y = b0 * xin + z[0];
            z[0] = b1 * xin - a1 * y + z[1];
            z[1] = b2 * xin - a2 * y;
            *v = y;
        }
    }
}

/// Steady-state states of each section for a constant input `c` (section inputs propagate).
fn steady(sos: &[Section], c: f64) -> Vec<[f64; 2]> {
    let mut u = c;
    sos.iter()
        .map(|s| {
            let [b0, b1, b2, a1, a2] = *s;
            let den = 1.0 + a1 + a2;
            let y = if den.abs() > 1e-300 {
                u * (b0 + b1 + b2) / den
            } else {
                0.0
            };
            // DF2T at steady state: z1 = b2 u - a2 y; z0 = b1 u - a1 y + z1 (= y - b0 u)
            let z1 = b2 * u - a2 * y;
            let z0 = b1 * u - a1 * y + z1;
            u = y;
            [z0, z1]
        })
        .collect()
}

/// Zero-phase filtering (module docs). Signals shorter than 2 samples are returned unchanged.
pub fn filtfilt(sos: &[Section], x: &[f64]) -> Vec<f64> {
    let n = x.len();
    if n < 2 || sos.is_empty() {
        return x.to_vec();
    }
    let pad = (3 * (2 * sos.len() + 1)).min(n - 1);
    let mut ext = Vec::with_capacity(n + 2 * pad);
    for i in (1..=pad).rev() {
        ext.push(2.0 * x[0] - x[i]);
    }
    ext.extend_from_slice(x);
    for i in 1..=pad {
        ext.push(2.0 * x[n - 1] - x[n - 1 - i]);
    }
    let mut zi = steady(sos, ext[0]);
    run(sos, &mut ext, &mut zi);
    ext.reverse();
    let mut zi = steady(sos, ext[0]);
    run(sos, &mut ext, &mut zi);
    ext.reverse();
    ext[pad..pad + n].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gain_at(sos: &[Section], f: f64, fs: f64) -> f64 {
        let w = 2.0 * std::f64::consts::PI * f / fs;
        let z1 = Complex::from_phase(-w);
        let z2 = Complex::from_phase(-2.0 * w);
        let mut g = Complex::new(1.0, 0.0);
        for s in sos {
            let num = Complex::new(s[0], 0.0) + z1.scale(s[1]) + z2.scale(s[2]);
            let den = Complex::new(1.0, 0.0) + z1.scale(s[3]) + z2.scale(s[4]);
            g *= cdiv(num, den);
        }
        g.abs()
    }

    #[test]
    fn butterworth_response() {
        let fs = 30000.0;
        for order in [1, 2, 3, 5] {
            let sos = bandpass_sos(order, 300.0, 6000.0, fs).unwrap();
            assert_eq!(sos.len(), order);
            // -3 dB at the band edges, flat in the middle, strong rejection far away
            for f in [300.0, 6000.0] {
                let g = gain_at(&sos, f, fs);
                assert!(
                    (g - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-6,
                    "order {order} f {f} g {g}"
                );
            }
            assert!((gain_at(&sos, 1500.0, fs) - 1.0).abs() < 0.02);
            if order >= 3 {
                assert!(gain_at(&sos, 30.0, fs) < 1e-2);
                assert!(gain_at(&sos, 14000.0, fs) < 1e-2);
            }
        }
        assert!(bandpass_sos(3, 300.0, 16000.0, fs).is_none());
        assert!(bandpass_sos(0, 300.0, 6000.0, fs).is_none());
        assert!(bandpass_sos(3, 600.0, 300.0, fs).is_none());
    }

    #[test]
    fn filtfilt_removes_offset_and_keeps_band() {
        let fs = 30000.0;
        let sos = bandpass_sos(3, 300.0, 6000.0, fs).unwrap();
        let x: Vec<f64> = (0..30000)
            .map(|i| 100.0 + (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / fs).sin())
            .collect();
        let y = filtfilt(&sos, &x);
        let mid = &y[5000..25000];
        let max = mid.iter().copied().fold(f64::MIN, f64::max);
        assert!((max - 1.0).abs() < 0.02, "{max}");
        let mean: f64 = mid.iter().sum::<f64>() / mid.len() as f64;
        assert!(mean.abs() < 0.01);
        assert_eq!(filtfilt(&sos, &[1.0]), vec![1.0]);
    }
}
