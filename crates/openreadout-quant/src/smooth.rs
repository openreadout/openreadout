//! Signal conditioning for peak detection: Savitzky–Golay smoothing, a robust noise estimate and
//! a running low-quantile baseline. Everything is deterministic (fixed summation order, no
//! threads) so the same input gives the same peaks on every machine.

/// Savitzky–Golay smoothing weights for the centre point of a window of `2m + 1` points, for a
/// least-squares polynomial of degree 2 (identical to degree 3 for the centre point):
/// `w_i = (3(3m² + 3m − 1) − 15 i²) / ((2m − 1)(2m + 1)(2m + 3))`, `i = −m..=m`.
/// `m = 0` is the identity. The weights sum to 1.
pub fn savgol_weights(m: usize) -> Vec<f64> {
    if m == 0 {
        return vec![1.0];
    }
    let mf = m as f64;
    let den = (2.0 * mf - 1.0) * (2.0 * mf + 1.0) * (2.0 * mf + 3.0);
    let a = 3.0 * (3.0 * mf * mf + 3.0 * mf - 1.0);
    (0..=2 * m)
        .map(|k| {
            let i = k as f64 - mf;
            (a - 15.0 * i * i) / den
        })
        .collect()
}

/// Quadratic Savitzky–Golay smoothing with a window of `window` points (odd; even values are
/// rounded up, values below 5 leave the signal unchanged). Near the ends the window shrinks
/// symmetrically so that every output point is a centred fit (the first and last points are
/// returned unchanged).
pub fn savgol(y: &[f64], window: usize) -> Vec<f64> {
    let m = half_window(window);
    if m == 0 || y.len() < 3 {
        return y.to_vec();
    }
    let weights: Vec<Vec<f64>> = (0..=m).map(savgol_weights).collect();
    let n = y.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let k = m.min(i).min(n - 1 - i);
        let w = &weights[k];
        let mut s = 0.0;
        for (j, wj) in w.iter().enumerate() {
            s += wj * y[i + j - k];
        }
        out.push(s);
    }
    out
}

/// Half-width `m` of a smoothing window of `window` points (`window` 0–4 → 0: no smoothing).
pub fn half_window(window: usize) -> usize {
    if window < 5 { 0 } else { window / 2 }
}

/// Factor by which smoothing with `window` points scales white noise: the Euclidean norm of the
/// centre weights (1 without smoothing).
pub fn noise_gain(window: usize) -> f64 {
    savgol_weights(half_window(window))
        .iter()
        .map(|w| w * w)
        .sum::<f64>()
        .sqrt()
}

/// How the noise level was estimated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoiseMethod {
    /// Root-mean-square residual of a straight-line fit in short consecutive segments; the
    /// 25th percentile over the segments (segments with peaks are larger and fall above it).
    SegmentRms,
    /// As `SegmentRms`, over the segments that are not entirely flat (more than a quarter of
    /// the segments are exactly constant: zero-filled data such as extracted-ion chromatograms).
    SegmentRmsNonzero,
    /// `1.4826 · MAD(Δy) / √2` over the first differences (too few samples for segments).
    MadDiff,
    /// Given by the caller.
    User,
    /// The signal is flat (or too short): noise 0.
    None,
}

impl NoiseMethod {
    /// Name used in JSON output.
    pub fn id(self) -> &'static str {
        match self {
            NoiseMethod::SegmentRms => "segment_rms",
            NoiseMethod::SegmentRmsNonzero => "segment_rms_nonzero",
            NoiseMethod::MadDiff => "mad_first_difference",
            NoiseMethod::User => "user",
            NoiseMethod::None => "none",
        }
    }
}

/// Number of samples per noise segment for a signal of `n` samples: 1 % of the signal, at
/// least 16 and at most 2000.
pub fn segment_len(n: usize) -> usize {
    (n / 100).clamp(16, 2000)
}

/// RMS residual of a least-squares line through `y` (index as abscissa), with `n − 2` degrees
/// of freedom. `None` when non-finite values are present or fewer than 3 samples.
fn detrended_rms(y: &[f64]) -> Option<f64> {
    let n = y.len();
    if n < 3 || y.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let nf = n as f64;
    let xm = (nf - 1.0) / 2.0;
    let ym = y.iter().sum::<f64>() / nf;
    let (mut sxy, mut sxx) = (0.0, 0.0);
    for (i, v) in y.iter().enumerate() {
        let dx = i as f64 - xm;
        sxy += dx * (v - ym);
        sxx += dx * dx;
    }
    let b = if sxx > 0.0 { sxy / sxx } else { 0.0 };
    let ss: f64 = y
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let r = v - (ym + b * (i as f64 - xm));
            r * r
        })
        .sum();
    Some((ss / (nf - 2.0)).sqrt())
}

/// Robust estimate of the standard deviation of the baseline noise on `y`, in the manner of
/// the ASTM E685 short-term noise: `y` is cut into consecutive segments of [`segment_len`]
/// samples, a straight line is fitted to each (removing drift), and the root-mean-square
/// residuals are ranked; the 25th percentile is the noise (segments that contain peaks have
/// larger residuals and rank above it). Unlike first differences, this is not fooled by
/// detector signals whose noise is low-pass filtered (correlated from sample to sample). When
/// more than a quarter of the segments are exactly flat (zero-filled data such as extracted-ion
/// chromatograms), the percentile is taken over the other segments. Signals too short for four
/// segments use the median absolute first difference, `1.4826 · MAD(Δy) / √2`.
pub fn noise_sigma(y: &[f64]) -> (f64, NoiseMethod) {
    noise_sigma_with(y, segment_len(y.len()))
}

/// [`noise_sigma`] with an explicit segment length.
pub fn noise_sigma_with(y: &[f64], seg: usize) -> (f64, NoiseMethod) {
    let seg = seg.max(3);
    if y.len() >= 4 * seg {
        let mut rms: Vec<f64> = y.chunks_exact(seg).filter_map(detrended_rms).collect();
        if rms.len() >= 4 {
            let flat = rms.iter().filter(|v| **v == 0.0).count();
            let method = if flat * 4 > rms.len() {
                rms.retain(|v| *v > 0.0);
                NoiseMethod::SegmentRmsNonzero
            } else {
                NoiseMethod::SegmentRms
            };
            if !rms.is_empty() {
                let k = ((rms.len() - 1) as f64 * 0.25).round() as usize;
                let (_, v, _) = rms.select_nth_unstable_by(k, f64::total_cmp);
                if *v > 0.0 {
                    return (*v, method);
                }
            }
        }
    }
    let d: Vec<f64> = y
        .windows(2)
        .map(|w| w[1] - w[0])
        .filter(|v| v.is_finite())
        .collect();
    if d.len() < 2 {
        return (0.0, NoiseMethod::None);
    }
    let s = mad_sigma(&d);
    if s > 0.0 {
        return (s, NoiseMethod::MadDiff);
    }
    let nz: Vec<f64> = d.into_iter().filter(|v| *v != 0.0).collect();
    if nz.len() >= 2 {
        let s = mad_sigma(&nz);
        if s > 0.0 {
            return (s, NoiseMethod::MadDiff);
        }
        let step = nz.iter().fold(f64::INFINITY, |a, v| a.min(v.abs()));
        return (step / std::f64::consts::SQRT_2, NoiseMethod::MadDiff);
    }
    (0.0, NoiseMethod::None)
}

fn mad_sigma(d: &[f64]) -> f64 {
    let mut v = d.to_vec();
    let med = median_in_place(&mut v);
    let mut dev: Vec<f64> = d.iter().map(|x| (x - med).abs()).collect();
    let mad = median_in_place(&mut dev);
    1.482_602_218_505_602 * mad / std::f64::consts::SQRT_2
}

/// Median (mean of the two middle values for an even count). `v` must be non-empty and
/// NaN-free; it is reordered.
pub fn median_in_place(v: &mut [f64]) -> f64 {
    let n = v.len();
    let mid = n / 2;
    let (_, m, _) = v.select_nth_unstable_by(mid, f64::total_cmp);
    let hi = *m;
    if n % 2 == 1 {
        return hi;
    }
    let lo = v[..mid].iter().copied().fold(f64::NEG_INFINITY, f64::max);
    f64::midpoint(lo, hi)
}

/// Running `q`-quantile (0..1) of `y` over a centred window of `window` points, evaluated every
/// `window / 8` points and linearly interpolated in between. With `q` = 0.25 it follows the
/// baseline under isolated peaks (a peak must fill three quarters of the window to lift it) and
/// sits about 0.67 σ below the mean of pure noise.
pub fn running_quantile(y: &[f64], window: usize, q: f64) -> Vec<f64> {
    let n = y.len();
    if n == 0 {
        return Vec::new();
    }
    let window = window.clamp(3, n.max(3));
    let half = window / 2;
    let step = (window / 8).max(1);
    let mut grid: Vec<(usize, f64)> = Vec::new();
    let mut buf = Vec::with_capacity(window + 1);
    let mut i = 0usize;
    loop {
        let lo = i.saturating_sub(half);
        let hi = (i + half + 1).min(n);
        buf.clear();
        buf.extend(y[lo..hi].iter().copied().filter(|v| v.is_finite()));
        let v = if buf.is_empty() {
            0.0
        } else {
            let k = ((buf.len() - 1) as f64 * q).round() as usize;
            let (_, x, _) = buf.select_nth_unstable_by(k, f64::total_cmp);
            *x
        };
        grid.push((i, v));
        if i == n - 1 {
            break;
        }
        i = (i + step).min(n - 1);
    }
    let mut out = Vec::with_capacity(n);
    for w in grid.windows(2) {
        let ((i0, v0), (i1, v1)) = (w[0], w[1]);
        for j in i0..i1 {
            let f = (j - i0) as f64 / (i1 - i0) as f64;
            out.push(v0 + (v1 - v0) * f);
        }
    }
    out.push(grid.last().map_or(0.0, |g| g.1));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn savgol_weights_match_the_tables() {
        let w = savgol_weights(2);
        let expect = [-3.0, 12.0, 17.0, 12.0, -3.0].map(|v| v / 35.0);
        for (a, b) in w.iter().zip(expect) {
            assert!((a - b).abs() < 1e-12);
        }
        let w = savgol_weights(3);
        let expect = [-2.0, 3.0, 6.0, 7.0, 6.0, 3.0, -2.0].map(|v| v / 21.0);
        for (a, b) in w.iter().zip(expect) {
            assert!((a - b).abs() < 1e-12);
        }
        for m in 1..30 {
            let s: f64 = savgol_weights(m).iter().sum();
            assert!((s - 1.0).abs() < 1e-12, "m={m} sum={s}");
        }
    }

    #[test]
    fn savgol_keeps_quadratics() {
        let y: Vec<f64> = (0..50)
            .map(|i| 3.0 + 0.5 * i as f64 - 0.01 * (i * i) as f64)
            .collect();
        let s = savgol(&y, 9);
        for (a, b) in s.iter().zip(&y) {
            assert!((a - b).abs() < 1e-9);
        }
        assert_eq!(savgol(&y, 3), y);
    }

    #[test]
    fn noise_of_white_noise() {
        // deterministic pseudo-noise: sum of 12 uniforms (Irwin–Hall) is close to Gaussian
        let mut state = 12345u64;
        let mut next = || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        let y: Vec<f64> = (0..20000)
            .map(|i| {
                let g: f64 = (0..12).map(|_| next()).sum::<f64>() - 6.0;
                100.0
                    + 2.0 * g
                    + if (9000..9100).contains(&i) {
                        500.0
                    } else {
                        0.0
                    }
            })
            .collect();
        let (s, m) = noise_sigma(&y);
        assert_eq!(m, NoiseMethod::SegmentRms);
        // the 25th percentile of 100 segment estimates sits slightly below σ
        assert!((s - 2.0).abs() < 0.15, "{s}");
        // low-pass filtered noise: first differences would underestimate it several-fold
        let f: Vec<f64> = y.windows(9).map(|w| w.iter().sum::<f64>() / 9.0).collect();
        let (sf, _) = noise_sigma(&f);
        assert!((sf - 2.0 / 3.0).abs() < 0.1, "{sf}");
        let d: Vec<f64> = f.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(mad_sigma(&d) < 0.4 * sf);
    }

    #[test]
    fn noise_of_sparse_signals() {
        let mut y = vec![0.0; 100];
        y[10] = 5.0;
        y[50] = 7.0;
        let (s, m) = noise_sigma(&y);
        assert_ne!(m, NoiseMethod::None);
        assert!(s > 0.0);
        let mut z = vec![0.0; 2000];
        for i in (0..2000).step_by(37) {
            z[i] = 10.0;
        }
        let (s, m) = noise_sigma(&z);
        assert_eq!(m, NoiseMethod::SegmentRmsNonzero);
        assert!(s > 0.0);
        assert_eq!(noise_sigma(&[1.0; 10]).1, NoiseMethod::None);
        assert_eq!(noise_sigma(&[]).0, 0.0);
    }

    #[test]
    fn running_quantile_ignores_a_peak() {
        let y: Vec<f64> = (0..400)
            .map(|i| if (200..210).contains(&i) { 100.0 } else { 1.0 })
            .collect();
        let b = running_quantile(&y, 101, 0.25);
        assert_eq!(b.len(), y.len());
        assert!(b.iter().all(|v| (v - 1.0).abs() < 1e-12));
        assert_eq!(running_quantile(&[], 5, 0.5), Vec::<f64>::new());
        assert_eq!(running_quantile(&[2.0], 5, 0.5), vec![2.0]);
    }
}
