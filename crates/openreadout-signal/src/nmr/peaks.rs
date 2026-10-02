//! Peak picking and integration on a real spectrum with a ppm axis.
//!
//! **Noise.** [`noise_sd`]: the spectrum is cut into 64 equal blocks; in each, a straight line
//! is fitted by least squares and the robust standard deviation `1.4826 · MAD` of the residuals
//! is computed (the line removes offset and slow baseline); the noise SD is the 25th percentile
//! over blocks, so blocks holding signal do not inflate it. S/N is `|height| / noise SD`.
//!
//! **Peaks.** A peak is a local maximum (strictly above its left neighbour, not below its right
//! one) of height ≥ `min_snr` × noise SD and ≥ `min_height_fraction` × the tallest point in the
//! searched range, whose prominence is ≥ `min_prominence_snr` × noise SD. The prominence is the
//! height above the higher of the two lowest points reached walking left and right until a
//! higher point (or the spectrum edge): it rejects noise ripples riding on the flanks of large
//! lines. Position and height come from the parabola through the maximum and its two
//! neighbours. Width at half height is found by walking outwards to the first points below half
//! height and interpolating linearly; when the signal rises again before reaching half height
//! (overlapping lines) the width is `None`. With `include_negative`, local minima of the negated
//! spectrum are reported as negative peaks (DEPT, APT).
//!
//! **Integrals.** The sum of the points whose shift lies in `[low, high]` ppm, times the point
//! spacing in ppm (a rectangle-rule area, in intensity·ppm). `normalized` rescales every integral
//! so that the reference region (default: the first) has the reference value (default 1).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::nmr::process::PpmAxis;

/// Blocks used by [`noise_sd`].
const NOISE_BLOCKS: usize = 64;

/// Robust noise standard deviation of `y` (module docs). 0 for fewer than 16 points.
pub fn noise_sd(y: &[f64]) -> f64 {
    let n = y.len();
    if n < 16 {
        return 0.0;
    }
    let bs = (n / NOISE_BLOCKS).max(8);
    let mut sds: Vec<f64> = y
        .chunks(bs)
        .filter(|c| c.len() >= 8 && c.iter().all(|v| v.is_finite()))
        .map(|c| {
            // least-squares line through the block, x centred
            let m = c.len() as f64;
            let xm = (m - 1.0) / 2.0;
            let ym = c.iter().sum::<f64>() / m;
            let (mut sxy, mut sxx) = (0.0, 0.0);
            for (i, v) in c.iter().enumerate() {
                let dx = i as f64 - xm;
                sxy += dx * (v - ym);
                sxx += dx * dx;
            }
            let slope = if sxx > 0.0 { sxy / sxx } else { 0.0 };
            let mut dev: Vec<f64> = c
                .iter()
                .enumerate()
                .map(|(i, v)| v - ym - slope * (i as f64 - xm))
                .collect();
            let med = median_in_place(&mut dev);
            let mut abs: Vec<f64> = dev.iter().map(|v| (v - med).abs()).collect();
            1.4826 * median_in_place(&mut abs)
        })
        .collect();
    if sds.is_empty() {
        return 0.0;
    }
    sds.sort_by(f64::total_cmp);
    sds[(sds.len() - 1) / 4]
}

fn median_in_place(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n == 0 {
        0.0
    } else if n % 2 == 1 {
        v[n / 2]
    } else {
        f64::midpoint(v[n / 2 - 1], v[n / 2])
    }
}

/// Peak-picking thresholds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PeakOptions {
    /// Minimum height as a multiple of the noise SD.
    pub min_snr: f64,
    /// Minimum height as a fraction of the tallest point in the searched range (0 = off).
    pub min_height_fraction: f64,
    /// Minimum prominence as a multiple of the noise SD.
    pub min_prominence_snr: f64,
    /// Also report negative peaks.
    pub include_negative: bool,
    /// Only search between these shifts (ppm, either order).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_ppm: Option<(f64, f64)>,
    /// Keep at most this many peaks (the tallest).
    pub max_peaks: usize,
}

impl Default for PeakOptions {
    fn default() -> Self {
        Self {
            min_snr: 10.0,
            min_height_fraction: 0.0,
            min_prominence_snr: 5.0,
            include_negative: false,
            range_ppm: None,
            max_peaks: 1000,
        }
    }
}

/// One picked peak.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Peak {
    /// Chemical shift, ppm (parabolic interpolation).
    pub ppm: f64,
    /// Frequency relative to 0 ppm, Hz.
    pub hz: f64,
    /// Index of the maximum point.
    pub index: usize,
    /// Interpolated height (negative for negative peaks).
    pub height: f64,
    /// Full width at half height, Hz (`None` when overlapping lines hide the half-height point).
    pub width_hz: Option<f64>,
    /// Full width at half height, ppm.
    pub width_ppm: Option<f64>,
    /// |height| / noise SD (`None` when the noise SD is 0).
    pub snr: Option<f64>,
}

/// Peaks of `y` on `axis` (module docs), sorted by decreasing shift (left to right).
pub fn pick_peaks(y: &[f64], axis: &PpmAxis, opts: &PeakOptions) -> (Vec<Peak>, f64) {
    let sd = noise_sd(y);
    let n = y.len().min(axis.size);
    if n < 3 {
        return (Vec::new(), sd);
    }
    let (lo, hi) = match opts.range_ppm {
        Some((a, b)) => {
            let (i, j) = (axis.index_of(a), axis.index_of(b));
            let (i, j) = if i <= j { (i, j) } else { (j, i) };
            (
                i.floor().clamp(1.0, (n - 2) as f64) as usize,
                j.ceil().clamp(1.0, (n - 2) as f64) as usize,
            )
        }
        None => (1, n - 2),
    };
    let tallest = y[lo..=hi]
        .iter()
        .filter(|v| v.is_finite())
        .fold(0.0_f64, |m, v| m.max(v.abs()));
    let threshold = (opts.min_snr * sd).max(opts.min_height_fraction * tallest);
    let mut peaks = Vec::new();
    for sign in [1.0, -1.0] {
        if sign < 0.0 && !opts.include_negative {
            break;
        }
        for i in lo..=hi {
            let (a, b, c) = (sign * y[i - 1], sign * y[i], sign * y[i + 1]);
            if !(b > a && b >= c && b >= threshold && b.is_finite()) {
                continue;
            }
            if prominence(y, i, sign) < opts.min_prominence_snr * sd {
                continue;
            }
            // parabolic interpolation
            let den = a - 2.0 * b + c;
            let (off, h) = if den < 0.0 {
                let off = (0.5 * (a - c) / den).clamp(-0.5, 0.5);
                (off, b - 0.25 * (a - c) * off)
            } else {
                (0.0, b)
            };
            let width = half_width(y, i, sign, h);
            let ppm = axis.ppm(i as f64 + off);
            let hzpp = axis.hz_per_point();
            peaks.push(Peak {
                ppm,
                hz: ppm * axis.reference_frequency_mhz,
                index: i,
                height: sign * h,
                width_hz: width.map(|w| w * hzpp),
                width_ppm: width.map(|w| w * axis.step_ppm.abs()),
                snr: (sd > 0.0).then(|| h / sd),
            });
        }
    }
    if peaks.len() > opts.max_peaks {
        peaks.sort_by(|a, b| b.height.abs().total_cmp(&a.height.abs()));
        peaks.truncate(opts.max_peaks);
    }
    peaks.sort_by(|a, b| b.ppm.total_cmp(&a.ppm));
    (peaks, sd)
}

/// Height of point `i` (of `sign · y`) above the higher of the minima on either side, each
/// found by walking outwards until a higher point or the edge.
fn prominence(y: &[f64], i: usize, sign: f64) -> f64 {
    let h = sign * y[i];
    let mut left_min = h;
    for &v in y[..i].iter().rev() {
        let v = sign * v;
        if v > h {
            break;
        }
        left_min = left_min.min(v);
    }
    let mut right_min = h;
    for &v in &y[i + 1..] {
        let v = sign * v;
        if v > h {
            break;
        }
        right_min = right_min.min(v);
    }
    h - left_min.max(right_min)
}

/// Full width at half of `h` around point `i`, in points.
fn half_width(y: &[f64], i: usize, sign: f64, h: f64) -> Option<f64> {
    let half = h / 2.0;
    let v = |k: usize| sign * y[k];
    // left
    let mut k = i;
    let left = loop {
        if k == 0 {
            return None;
        }
        let (cur, nxt) = (v(k), v(k - 1));
        if nxt < half {
            // interpolate between k-1 (below) and k (above)
            break (k - 1) as f64 + (half - nxt) / (cur - nxt);
        }
        if nxt > cur {
            return None;
        }
        k -= 1;
    };
    let mut k = i;
    let right = loop {
        if k + 1 >= y.len() {
            return None;
        }
        let (cur, nxt) = (v(k), v(k + 1));
        if nxt < half {
            break k as f64 + (cur - half) / (cur - nxt);
        }
        if nxt > cur {
            return None;
        }
        k += 1;
    };
    Some(right - left)
}

/// One integral.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Integral {
    /// High-shift end of the region, ppm.
    pub from_ppm: f64,
    /// Low-shift end of the region, ppm.
    pub to_ppm: f64,
    /// Sum of the points × point spacing (intensity·ppm).
    pub value: f64,
    /// `value` rescaled so the reference region has the reference value (`None` when the
    /// reference integral is 0 or missing).
    pub normalized: Option<f64>,
    /// Points summed.
    pub points: usize,
}

/// Integrate `y` over `regions` (ppm pairs in either order). `reference` = (region index,
/// value) for the normalisation (default: region 0 → 1).
pub fn integrate(
    y: &[f64],
    axis: &PpmAxis,
    regions: &[(f64, f64)],
    reference: Option<(usize, f64)>,
) -> Vec<Integral> {
    let n = y.len().min(axis.size);
    let mut out: Vec<Integral> = regions
        .iter()
        .map(|&(a, b)| {
            let (hi, lo) = if a >= b { (a, b) } else { (b, a) };
            let (i, j) = (axis.index_of(hi), axis.index_of(lo));
            let (i, j) = if i <= j { (i, j) } else { (j, i) };
            let start = i.ceil().max(0.0) as usize;
            let end = (j.floor() as isize).min(n as isize - 1);
            let mut sum = 0.0;
            let mut points = 0;
            if end >= 0 {
                for v in y.iter().take(end as usize + 1).skip(start) {
                    if v.is_finite() {
                        sum += v;
                        points += 1;
                    }
                }
            }
            Integral {
                from_ppm: hi,
                to_ppm: lo,
                value: sum * axis.step_ppm.abs(),
                normalized: None,
                points,
            }
        })
        .collect();
    let (ri, rv) = reference.unwrap_or((0, 1.0));
    let base = out.get(ri).map_or(f64::NAN, |r| r.value);
    for r in &mut out {
        r.normalized = (base != 0.0 && base.is_finite()).then(|| r.value / base * rv);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axis(n: usize) -> PpmAxis {
        PpmAxis {
            first_ppm: 10.0,
            step_ppm: -10.0 / n as f64,
            size: n,
            reference_frequency_mhz: 400.0,
        }
    }

    fn lorentz(n: usize, lines: &[(f64, f64, f64)]) -> Vec<f64> {
        (0..n)
            .map(|k| {
                lines
                    .iter()
                    .map(|&(c, h, w)| h / (1.0 + ((k as f64 - c) / (w / 2.0)).powi(2)))
                    .sum::<f64>()
                    + 0.01 * crate::nmr::baseline::tests::noise(k)
            })
            .collect()
    }

    #[test]
    fn finds_lines_with_width_and_position() {
        let n = 8192;
        let ax = axis(n);
        let y = lorentz(n, &[(1000.3, 100.0, 8.0), (5000.0, 50.0, 4.0)]);
        let (p, sd) = pick_peaks(&y, &ax, &PeakOptions::default());
        assert!(sd > 0.0);
        assert_eq!(p.len(), 2, "{p:?}");
        assert!((p[0].ppm - ax.ppm(1000.3)).abs() < 0.05 * ax.step_ppm.abs());
        assert!((p[0].height - 100.0).abs() < 1.0);
        let w = p[0].width_ppm.unwrap() / ax.step_ppm.abs();
        assert!((w - 8.0).abs() < 0.3, "width {w}");
        assert!((p[1].width_hz.unwrap() - 4.0 * ax.hz_per_point()).abs() < 0.3 * ax.hz_per_point());
    }

    #[test]
    fn integrals_scale_with_area() {
        let n = 8192;
        let ax = axis(n);
        let y = lorentz(n, &[(1000.0, 100.0, 4.0), (5000.0, 100.0, 8.0)]);
        let r = integrate(
            &y,
            &ax,
            &[
                (ax.ppm(900.0), ax.ppm(1100.0)),
                (ax.ppm(4900.0), ax.ppm(5100.0)),
            ],
            None,
        );
        assert!((r[0].normalized.unwrap() - 1.0).abs() < 1e-12);
        assert!((r[1].normalized.unwrap() - 2.0).abs() < 0.05, "{r:?}");
        let r = integrate(&y, &ax, &[(ax.ppm(4900.0), ax.ppm(5100.0))], Some((0, 3.0)));
        assert!((r[0].normalized.unwrap() - 3.0).abs() < 1e-12);
        // regions off the axis and a reference out of range are harmless
        let r = integrate(&y, &ax, &[(50.0, 40.0)], Some((5, 1.0)));
        assert_eq!(r[0].points, 0);
        assert!(r[0].normalized.is_none());
    }

    #[test]
    fn negative_peaks_on_request() {
        let n = 4096;
        let ax = axis(n);
        let y = lorentz(n, &[(1000.0, 100.0, 4.0), (3000.0, -80.0, 4.0)]);
        let (p, _) = pick_peaks(&y, &ax, &PeakOptions::default());
        assert_eq!(p.len(), 1);
        let (p, _) = pick_peaks(
            &y,
            &ax,
            &PeakOptions {
                include_negative: true,
                ..PeakOptions::default()
            },
        );
        assert_eq!(p.len(), 2);
        assert!(p[1].height < 0.0);
    }

    #[test]
    fn tiny_or_flat_input() {
        let ax = axis(2);
        assert!(
            pick_peaks(&[1.0, 2.0], &ax, &PeakOptions::default())
                .0
                .is_empty()
        );
        let y = vec![0.0; 1000];
        let (p, sd) = pick_peaks(&y, &axis(1000), &PeakOptions::default());
        assert!(p.is_empty());
        assert!(sd.abs() < f64::EPSILON);
    }
}
