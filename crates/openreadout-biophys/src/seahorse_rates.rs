//! Oxygen consumption, extracellular acidification and proton efflux rates of Seahorse XF assays,
//! computed from the stored sensor emissions with the published method (Gerencser et al.,
//! Anal. Chem. 2009, 81, 6868; its Supporting Information) and the constants each `.asyr` file
//! stores. Every formula here was checked against Wave's own numbers
//! (`docs/provenance/agilent-seahorse.md`, 2026-09-26): O2 and pH levels to 1e-12, ECAR to
//! 5e-6 mpH/min, OCR to 0.17 pmol/min (about 0.1 %).
//!
//! The functions are public (hidden from the docs) so the corpus tests can run them on the
//! emissions of a Wave export as well as on `.asyr` files.

/// First-derivative Savitzky-Golay kernel (7 points, quadratic), as the files store it.
pub(crate) const SG_FIRST: [f64; 7] = [
    3.0 / 28.0,
    2.0 / 28.0,
    1.0 / 28.0,
    0.0,
    -1.0 / 28.0,
    -2.0 / 28.0,
    -3.0 / 28.0,
];
/// Second-derivative Savitzky-Golay kernel (7 points, quadratic) as the files store it: divided
/// by the square of the time kernel it gives the local parabola's x² coefficient (half the
/// curvature), which is what the published spreadsheet and Wave use as d²[O2]/dt².
pub(crate) const SG_SECOND: [f64; 7] = [
    5.0 / 84.0,
    0.0,
    -3.0 / 84.0,
    -4.0 / 84.0,
    -3.0 / 84.0,
    0.0,
    5.0 / 84.0,
];
/// Smoothing Savitzky-Golay kernel (7 points, quadratic).
pub(crate) const SG_SMOOTH: [f64; 7] = [
    -2.0 / 21.0,
    3.0 / 21.0,
    6.0 / 21.0,
    7.0 / 21.0,
    6.0 / 21.0,
    3.0 / 21.0,
    -2.0 / 21.0,
];
/// Time constant (s) of the exponential that stands in for the O2 level while the probe is
/// raised between measurements (the paper's Supporting Information, eq. 19).
const FILL_TAU_S: f64 = 30.0;
/// Readings dropped at each end of a measurement before averaging the corrected OCR (the half
/// width of the kernels).
pub(crate) const OCR_EDGE: usize = 3;

/// The oxygen model of one assay: Stern-Volmer constants and the compartment model's time
/// constants, as stored in `O2DataModifiers`.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OxygenModel {
    /// Emission at zero oxygen (`FO`).
    pub f_zero: f64,
    /// Stern-Volmer constant, 1/mmHg (`Ksv`).
    pub ksv: f64,
    /// Ambient oxygen, mmHg (`CO`).
    pub ambient_mmhg: f64,
    /// Ambient oxygen, mM (`COb`).
    pub ambient_mm: f64,
    /// Atmosphere-to-chamber time constant, s (`TauAC`; 0 disables the path).
    pub tau_ac: f64,
    /// Atmosphere-to-wall time constant, s (`TauAW`; 0 disables the path).
    pub tau_aw: f64,
    /// Chamber-to-wall time constant, s (`TauW`).
    pub tau_w: f64,
    /// Wall-to-chamber time constant, s (`TauC`).
    pub tau_c: f64,
    /// Probe response time constant, s (`TauP`).
    pub tau_p: f64,
    /// Apparent chamber volume, µL (`ChamberVolume`).
    pub chamber_ul: f64,
}

fn rate(tau: f64) -> f64 {
    if tau > 0.0 { 1.0 / tau } else { 0.0 }
}

/// O2 level (mmHg) of every well at one reading: `CO + (FO/Ksv)(1/F − 1/F_T)`, `F_T` the mean
/// corrected emission of the background wells at that reading. NaN when no background well has
/// a finite emission.
#[doc(hidden)]
#[must_use]
pub fn oxygen_levels(emission: &[f64], background: &[usize], m: &OxygenModel) -> Vec<f64> {
    let bg: Vec<f64> = background
        .iter()
        .filter_map(|&i| emission.get(i).copied())
        .filter(|v| v.is_finite())
        .collect();
    let reference = if bg.is_empty() || bg.len() != background.len() {
        f64::NAN
    } else {
        bg.iter().sum::<f64>() / bg.len() as f64
    };
    emission
        .iter()
        .map(|&f| m.ambient_mmhg + m.f_zero / m.ksv * (1.0 / f - 1.0 / reference))
        .collect()
}

/// pH of one well at one reading: `pH_cal + (F − F_cal) / (1000 (C3 F_cal + C4))`, `F_cal` the
/// well's calibration emission, `C3`, `C4` the pH gain equation's linear terms.
#[doc(hidden)]
#[must_use]
pub fn ph_level(emission: f64, calibration: f64, c3: f64, c4: f64, ph_cal: f64) -> f64 {
    ph_cal + (emission - calibration) / (1000.0 * (c3 * calibration + c4))
}

fn kernel(values: &[f64], at: usize, k: &[f64; 7]) -> Option<f64> {
    let lo = at.checked_sub(3)?;
    let win = values.get(lo..lo.checked_add(7)?)?;
    Some(win.iter().zip(k).map(|(v, c)| v * c).sum())
}

/// Integral over `[ta, tb]` of the quadratic through `(ta, a)`, `(tb, b)`, `(tc, c)` (the
/// Supporting Information's eq. 18).
fn quad_segment(ta: f64, tb: f64, tc: f64, a: f64, b: f64, c: f64) -> f64 {
    -(ta - tb)
        * (-c * (ta - tb).powi(2)
            + b * (ta + 2.0 * tb - 3.0 * tc) * (ta - tc)
            + a * (2.0 * ta + tb - 3.0 * tc) * (tb - tc))
        / (6.0 * (ta - tc) * (tb - tc))
}

/// Oxygen consumption rate (pmol/min) of one well in every measurement: the mean of the
/// paper's corrected OCR(t) (eqs. 13-14) over the measurement's readings without the first
/// and last three. `times` (s) and `levels` (mmHg) are per reading; `spans` are the
/// measurements' first and last reading (inclusive, increasing, not overlapping). The O2 level
/// between measurements, when the probe is raised and nothing is read, is filled with the
/// exponential of eq. 19 at the measurement's mean reading interval. A measurement with fewer
/// than seven readings, or any non-finite input it depends on, gives NaN.
#[doc(hidden)]
#[must_use]
pub fn oxygen_consumption(
    times: &[f64],
    levels: &[f64],
    spans: &[(usize, usize)],
    model: &OxygenModel,
) -> Vec<f64> {
    let nan = vec![f64::NAN; spans.len()];
    if times.len() != levels.len()
        || spans.is_empty()
        || spans.iter().any(|&(a, b)| a > b || b >= times.len())
        || spans.windows(2).any(|w| w[1].0 <= w[0].1)
    {
        return nan;
    }
    let Some(Course {
        times: ts,
        levels: ms,
        measured,
    }) = time_course(times, levels, spans)
    else {
        return nan;
    };
    let n = ts.len();
    let t0 = ts[0];
    let t: Vec<f64> = ts.iter().map(|x| x - t0).collect();
    let (k_p, k_ac, k_aw, k_w, k_c) = (
        rate(model.tau_p),
        rate(model.tau_ac),
        rate(model.tau_aw),
        rate(model.tau_w),
        rate(model.tau_c),
    );
    if k_p <= 0.0 {
        return nan;
    }
    let ambient = model.ambient_mmhg;
    let (d1, d2, smooth) = derivatives(&ms, &t);
    // wall oxygen (eq. 13), integrated step by step with the quadratic interpolation of the
    // integrand, each step scaled to its own end so the exponential never overflows
    let k = k_aw + k_w;
    let g: Vec<f64> = (0..n)
        .map(|i| k_aw * ambient + k_w * (smooth[i] + d1[i] / k_p))
        .collect();
    let mut wall = vec![0.0; n];
    wall[0] = ms[0];
    for i in 1..n {
        let (ta, tb) = (t[i - 1], t[i]);
        let decay = |x: f64| (k * (x - tb)).exp();
        let seg = if i + 1 < n {
            let tc = t[i + 1];
            quad_segment(ta, tb, tc, decay(ta) * g[i - 1], g[i], decay(tc) * g[i + 1])
        } else {
            f64::midpoint(decay(ta) * g[i - 1], g[i]) * (tb - ta)
        };
        wall[i] = decay(ta) * wall[i - 1] + seg;
    }
    let factor = model.chamber_ul * model.ambient_mm / model.ambient_mmhg * 60.0 * 1000.0;
    measured
        .iter()
        .map(|idx| {
            if idx.len() < 2 * OCR_EDGE + 1 {
                return f64::NAN;
            }
            let inner = &idx[OCR_EDGE..idx.len() - OCR_EDGE];
            let sum: f64 = inner
                .iter()
                .map(|&i| {
                    k_ac * ambient - (k_ac + k_c) * smooth[i] + k_c * wall[i]
                        - (k_ac + k_p + k_c) / k_p * d1[i]
                        - d2[i] / k_p
                })
                .sum();
            sum / inner.len() as f64 * factor
        })
        .collect()
}

/// The whole time course, measured readings and filled gaps.
struct Course {
    times: Vec<f64>,
    levels: Vec<f64>,
    /// The course positions of each measurement's readings.
    measured: Vec<Vec<usize>>,
}

/// The time course of `times` and `levels`; `None` when a gap cannot be filled.
fn time_course(times: &[f64], levels: &[f64], spans: &[(usize, usize)]) -> Option<Course> {
    let mut ts: Vec<f64> = Vec::new();
    let mut ms: Vec<f64> = Vec::new();
    let mut measured: Vec<Vec<usize>> = Vec::with_capacity(spans.len());
    for (k, &(first, last)) in spans.iter().enumerate() {
        let mut idx = Vec::with_capacity(last - first + 1);
        for i in first..=last {
            idx.push(ts.len());
            ts.push(times[i]);
            ms.push(levels[i]);
        }
        measured.push(idx);
        let Some(&(next, _)) = spans.get(k + 1) else {
            continue;
        };
        let gap = times[next] - times[last];
        let step = if last > first {
            (times[last] - times[first]) / (last - first) as f64
        } else {
            f64::NAN
        };
        if !(gap.is_finite() && step.is_finite() && gap > 0.0 && step > 0.0) {
            return None;
        }
        let fills = ((gap / step).round() - 1.0).max(0.0);
        if fills > 100_000.0 {
            return None;
        }
        let fills = fills as usize;
        let (a, b) = (levels[last], levels[next]);
        let dt = gap / (fills + 1) as f64;
        for j in 1..=fills {
            ts.push(times[last] + dt * j as f64);
            ms.push(b - (b - a) * (-((j - 1) as f64) * dt / FILL_TAU_S).exp());
        }
    }
    Some(Course {
        times: ts,
        levels: ms,
        measured,
    })
}

/// First and second derivatives and the smoothed level of `ms` against `t`; the first and last
/// three of the whole course have none.
fn derivatives(ms: &[f64], t: &[f64]) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let n = ms.len();
    let mut d1 = vec![0.0; n];
    let mut d2 = vec![0.0; n];
    let mut smooth = ms.to_vec();
    for i in 0..n {
        if let (Some(num1), Some(den), Some(num2), Some(sm)) = (
            kernel(ms, i, &SG_FIRST),
            kernel(t, i, &SG_FIRST),
            kernel(ms, i, &SG_SECOND),
            kernel(ms, i, &SG_SMOOTH),
        ) {
            d1[i] = num1 / den;
            d2[i] = num2 / (den * den);
            smooth[i] = sm;
        }
    }
    (d1, d2, smooth)
}

/// Least-squares slope of `values` against `times` (NaN with fewer than two finite points).
#[doc(hidden)]
#[must_use]
pub fn slope(times: &[f64], values: &[f64]) -> f64 {
    let pts: Vec<(f64, f64)> = times
        .iter()
        .zip(values)
        .filter(|(t, v)| t.is_finite() && v.is_finite())
        .map(|(t, v)| (*t, *v))
        .collect();
    if pts.len() < 2 {
        return f64::NAN;
    }
    let n = pts.len() as f64;
    let mt = pts.iter().map(|p| p.0).sum::<f64>() / n;
    let mv = pts.iter().map(|p| p.1).sum::<f64>() / n;
    let sxx: f64 = pts.iter().map(|p| (p.0 - mt).powi(2)).sum();
    let sxy: f64 = pts.iter().map(|p| (p.0 - mt) * (p.1 - mv)).sum();
    if sxx > 0.0 { sxy / sxx } else { f64::NAN }
}

/// Acidification of one well in one measurement before background correction (mpH/min):
/// −1000 × the slope of its pH against time in minutes over the readings from `offset` on.
#[doc(hidden)]
#[must_use]
pub fn acidification(times_s: &[f64], ph: &[f64], span: (usize, usize), offset: usize) -> f64 {
    let (first, last) = span;
    let Some(start) = first.checked_add(offset) else {
        return f64::NAN;
    };
    if start > last || last >= times_s.len() || last >= ph.len() {
        return f64::NAN;
    }
    let t: Vec<f64> = times_s[start..=last].iter().map(|x| x / 60.0).collect();
    -1000.0 * slope(&t, &ph[start..=last])
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]
    use super::*;

    fn model() -> OxygenModel {
        OxygenModel {
            f_zero: 52_508.243_864_537_74,
            ksv: 0.0211,
            ambient_mmhg: 151.690_017,
            ambient_mm: 0.214,
            tau_ac: 746.0,
            tau_aw: 0.0,
            tau_w: 296.0,
            tau_c: 246.0,
            tau_p: 60.9,
            chamber_ul: 9.15,
        }
    }

    #[test]
    fn kernels_are_the_published_ones() {
        assert!((SG_FIRST[0] - 0.107_142_857_142_857).abs() < 1e-15);
        assert!((SG_SECOND[0] - 0.059_523_809_523_809_5).abs() < 1e-15);
        assert!((SG_SMOOTH[3] - 0.333_333_333_333_333).abs() < 1e-15);
        // a line differentiates to its slope, a parabola twice to its curvature
        let x: Vec<f64> = (0..7).map(f64::from).collect();
        let line: Vec<f64> = x.iter().map(|v| 2.0 * v + 1.0).collect();
        let para: Vec<f64> = x.iter().map(|v| v * v).collect();
        let d = kernel(&line, 3, &SG_FIRST).unwrap() / kernel(&x, 3, &SG_FIRST).unwrap();
        assert!((d - 2.0).abs() < 1e-12);
        // the published "second derivative" is the local parabola's x² coefficient, half the
        // curvature; the Supporting Information and Wave use it as it is (doubling it moves OCR
        // 6-15 % away from Wave's)
        let dd = kernel(&para, 3, &SG_SECOND).unwrap() / kernel(&x, 3, &SG_FIRST).unwrap().powi(2);
        assert!((dd - 1.0).abs() < 1e-12);
        assert!((kernel(&para, 3, &SG_SMOOTH).unwrap() - 9.0).abs() < 1.0);
        assert_eq!(kernel(&x, 2, &SG_FIRST), None);
        assert_eq!(kernel(&x, 4, &SG_FIRST), None);
    }

    #[test]
    fn quadratic_segment_integrates_a_parabola_exactly() {
        // ∫_1^2 x² dx = 7/3
        let v = quad_segment(1.0, 2.0, 3.5, 1.0, 4.0, 12.25);
        assert!((v - 7.0 / 3.0).abs() < 1e-12);
    }

    #[test]
    fn levels_are_ambient_for_the_background() {
        let m = model();
        let l = oxygen_levels(&[12_000.0, 12_500.0, 13_000.0], &[1], &m);
        assert!((l[1] - m.ambient_mmhg).abs() < 1e-9);
        assert!(l[0] > l[1] && l[2] < l[1]);
        assert!(
            oxygen_levels(&[1.0, 2.0], &[5], &m)
                .iter()
                .all(|v| v.is_nan())
        );
        assert!((ph_level(30_000.0, 30_000.0, 0.000_405, 1.01, 7.4) - 7.4).abs() < 1e-15);
        // Wave's pH of well A02 at the first reading of the seahtrue export
        let p = ph_level(28_931.581_665_590_7, 29_700.0, 0.000_405, 1.01, 7.4);
        assert!((p - 7.341_065_434_336_06).abs() < 1e-9, "{p}");
    }

    #[test]
    fn constant_oxygen_consumes_nothing() {
        let times: Vec<f64> = (0..36)
            .map(|i| f64::from(i) * 15.7 + f64::from(i / 12) * 200.0)
            .collect();
        let levels = vec![151.690_017; 36];
        let r = oxygen_consumption(&times, &levels, &[(0, 11), (12, 23), (24, 35)], &model());
        assert_eq!(r.len(), 3);
        // the quadratic interpolation of the wall integral is not exact for an exponential:
        // a few 1e-3 pmol/min on a constant level
        assert!(r.iter().all(|v| v.abs() < 0.01), "{r:?}");
    }

    #[test]
    fn falling_oxygen_is_consumption_and_bad_spans_give_nan() {
        let times: Vec<f64> = (0..24).map(|i| f64::from(i) * 15.7).collect();
        let levels: Vec<f64> = times.iter().map(|t| 150.0 - 0.1 * t).collect();
        let r = oxygen_consumption(&times, &levels, &[(0, 11), (12, 23)], &model());
        assert!(r.iter().all(|v| *v > 0.0), "{r:?}");
        let m = model();
        assert!(oxygen_consumption(&times, &levels, &[(0, 30)], &m)[0].is_nan());
        assert!(oxygen_consumption(&times, &levels, &[(5, 2)], &m)[0].is_nan());
        assert!(
            oxygen_consumption(&times, &levels, &[(0, 11), (11, 23)], &m)
                .iter()
                .all(|v| v.is_nan())
        );
        assert!(oxygen_consumption(&times, &levels, &[(0, 4)], &m)[0].is_nan());
        assert!(oxygen_consumption(&times, &levels[..3], &[(0, 2)], &m)[0].is_nan());
    }

    #[test]
    fn slopes_and_acidification() {
        let t = [0.0, 60.0, 120.0, 180.0, 240.0];
        let ph = [7.4, 7.39, 7.38, 7.37, 7.36];
        assert!((acidification(&t, &ph, (0, 4), 0) - 10.0).abs() < 1e-9);
        assert!((acidification(&t, &ph, (0, 4), 3) - 10.0).abs() < 1e-9);
        assert!(acidification(&t, &ph, (0, 4), 4).is_nan());
        assert!(acidification(&t, &ph, (0, 9), 0).is_nan());
        assert!(slope(&[1.0], &[2.0]).is_nan());
        assert!(slope(&[1.0, 1.0], &[2.0, 3.0]).is_nan());
    }
}
