//! Our own qPCR analysis (documented in `docs/formats/qpcr.md` → "Analysis"): threshold-cycle
//! Cq with linear baseline subtraction, melt-curve −dF/dT, and least-squares helpers for
//! standard curves. Vendor-computed values are never overwritten; these results are reported
//! next to them.

/// Default baseline window (1-based cycles, inclusive), the common instrument default.
pub(crate) const DEFAULT_BASELINE: (u32, u32) = (3, 15);
/// Automatic threshold: this many standard deviations of the baseline residuals.
pub(crate) const AUTO_THRESHOLD_SDS: f64 = 10.0;

/// Result of our Cq computation on one curve.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CqCall {
    /// Fractional cycle where the baseline-corrected curve crosses the threshold.
    pub(crate) cq: Option<f64>,
    pub(crate) threshold: f64,
    /// `given` or `auto`.
    pub(crate) threshold_source: &'static str,
    pub(crate) baseline_start: u32,
    pub(crate) baseline_end: u32,
    /// Baseline line: `intercept + slope * cycle`.
    pub(crate) baseline_intercept: f64,
    pub(crate) baseline_slope: f64,
}

/// Ordinary least squares `y = a + b x`; returns (a, b, r²). `None` with fewer than 2 finite
/// points or no spread in x.
#[allow(clippy::many_single_char_names)] // the textbook least-squares names
pub(crate) fn linear_fit(x: &[f64], y: &[f64]) -> Option<(f64, f64, f64)> {
    let pts: Vec<(f64, f64)> = x
        .iter()
        .zip(y)
        .filter(|(a, b)| a.is_finite() && b.is_finite())
        .map(|(a, b)| (*a, *b))
        .collect();
    let n = pts.len() as f64;
    if pts.len() < 2 {
        return None;
    }
    let mx = pts.iter().map(|p| p.0).sum::<f64>() / n;
    let my = pts.iter().map(|p| p.1).sum::<f64>() / n;
    let sxx: f64 = pts.iter().map(|p| (p.0 - mx).powi(2)).sum();
    let sxy: f64 = pts.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
    let syy: f64 = pts.iter().map(|p| (p.1 - my).powi(2)).sum();
    if sxx <= 0.0 {
        return None;
    }
    let b = sxy / sxx;
    let a = my - b * mx;
    let r2 = if syy > 0.0 {
        (sxy * sxy) / (sxx * syy)
    } else {
        1.0
    };
    Some((a, b, r2))
}

/// How the baseline is found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Baseline {
    /// A least-squares line through these cycles (1-based, inclusive).
    Window(u32, u32),
    /// A known background line `intercept + slope * cycle` (RDML `bgFluor`, `bgFluorSlp`).
    Line(f64, f64),
    /// Cycles 3-15, then the window is ended three cycles before the first Cq estimate and the
    /// Cq recomputed (at most three times) so that early curves do not leak into their
    /// baseline.
    Auto,
}

/// Threshold-cycle Cq of one amplification curve.
///
/// 1. Baseline: see [`Baseline`]; the line (or constant) is subtracted from every cycle.
/// 2. Threshold: `threshold` when given (in the same units as the corrected curve, e.g. ΔRn),
///    else 10 × the standard deviation of the corrected baseline cycles.
/// 3. Cq: the first cycle after the baseline start where the corrected curve rises from below
///    the threshold to at or above it and stays there at the next cycle, linearly interpolated
///    between the two cycles. No crossing → `cq: None` (undetermined).
pub(crate) fn threshold_cq(
    cycles: &[f64],
    fluorescence: &[f64],
    threshold: Option<f64>,
    baseline: Baseline,
) -> Option<CqCall> {
    match baseline {
        Baseline::Window(s, e) => {
            threshold_cq_window(cycles, fluorescence, threshold, Some((s, e)))
        }
        Baseline::Line(v, slope) => {
            let n = fluorescence.len().min(cycles.len());
            if n < 2 || !v.is_finite() || !slope.is_finite() {
                return None;
            }
            let shifted: Vec<f64> = (0..n)
                .map(|i| fluorescence[i] - (v + slope * cycles[i]))
                .collect();
            let thr = threshold.filter(|t| t.is_finite() && *t > 0.0)?;
            let cq = crossing(&cycles[..n], &shifted, thr, 0);
            Some(CqCall {
                cq,
                threshold: thr,
                threshold_source: "given",
                baseline_start: 0,
                baseline_end: 0,
                baseline_intercept: v,
                baseline_slope: slope,
            })
        }
        Baseline::Auto => {
            let mut call = threshold_cq_window(cycles, fluorescence, threshold, None)?;
            // A curve that rises inside cycles 3-15 bends its own baseline and may never
            // cross: try windows that end earlier, keeping the first that ends before the rise.
            if call.cq.is_none() {
                for end in [12u32, 9, 7, 5, 4] {
                    let start = if end >= 7 { 3 } else { 1 };
                    if let Some(c) =
                        threshold_cq_window(cycles, fluorescence, threshold, Some((start, end)))
                        && c.cq.is_some_and(|q| q >= f64::from(end) + 2.0)
                    {
                        call = c;
                        break;
                    }
                }
            }
            for _ in 0..3 {
                let Some(cq) = call.cq else { break };
                let end = (cq.floor() as i64 - 3).max(0) as u32;
                // the window already ends before the rise
                if end >= call.baseline_end {
                    break;
                }
                let start = if end >= 5 { 3 } else { 1 };
                // the curve rises too early for a window before it
                if end < start + 2 {
                    break;
                }
                let next =
                    threshold_cq_window(cycles, fluorescence, threshold, Some((start, end)))?;
                if next.baseline_end == call.baseline_end {
                    break;
                }
                call = next;
            }
            Some(call)
        }
    }
}

/// First up-crossing of `thr` after index `from` that holds at the next point, interpolated.
fn crossing(cycles: &[f64], corrected: &[f64], thr: f64, from: usize) -> Option<f64> {
    let n = corrected.len().min(cycles.len());
    for i in (from + 1).max(1)..n {
        let (p, c) = (corrected[i - 1], corrected[i]);
        if !(p.is_finite() && c.is_finite()) {
            continue;
        }
        if p < thr && c >= thr && corrected.get(i + 1).is_none_or(|nx| *nx >= thr) {
            // Amplification is exponential between reads: interpolate log(fluorescence) when
            // both points are positive, linearly otherwise.
            let f = if p > 0.0 && thr > 0.0 {
                (thr.ln() - p.ln()) / (c.ln() - p.ln())
            } else {
                (thr - p) / (c - p)
            };
            return Some(cycles[i - 1] + f * (cycles[i] - cycles[i - 1]));
        }
    }
    None
}

fn threshold_cq_window(
    cycles: &[f64],
    fluorescence: &[f64],
    threshold: Option<f64>,
    baseline: Option<(u32, u32)>,
) -> Option<CqCall> {
    let n = fluorescence.len().min(cycles.len());
    if n < 4 {
        return None;
    }
    let (mut start, mut end) = baseline.unwrap_or(DEFAULT_BASELINE);
    let last = n as u32;
    if start < 1 {
        start = 1;
    }
    if end >= last {
        end = last.saturating_sub(1);
    }
    if end < start + 2 {
        // too short a window: the first quarter of the run (at least 3 cycles)
        start = 1;
        end = (last / 4).max(3).min(last.saturating_sub(1));
    }
    let (si, ei) = (start as usize - 1, end as usize);
    let (intercept, slope, _) = linear_fit(&cycles[si..ei], &fluorescence[si..ei])?;
    let corrected: Vec<f64> = (0..n)
        .map(|i| fluorescence[i] - (intercept + slope * cycles[i]))
        .collect();
    let (thr, src) = match threshold {
        Some(t) if t.is_finite() && t > 0.0 => (t, "given"),
        _ => {
            let w = &corrected[si..ei];
            let m = w.iter().sum::<f64>() / w.len() as f64;
            let sd =
                (w.iter().map(|v| (v - m).powi(2)).sum::<f64>() / (w.len() as f64 - 1.0)).sqrt();
            if !(sd.is_finite() && sd > 0.0) {
                return None;
            }
            (AUTO_THRESHOLD_SDS * sd, "auto")
        }
    };
    let mut cq = crossing(&cycles[..n], &corrected, thr, si);
    // An automatic threshold only measures noise: a curve that decays or drifts (photobleaching)
    // can cross it after the baseline line is removed. Keep such a crossing only when the stored
    // fluorescence itself rises above its baseline-window level by at least the threshold after
    // the crossing (an amplification rises; a decaying curve does not).
    if src == "auto"
        && let Some(q) = cq
    {
        let w = &fluorescence[si..ei];
        let level = w.iter().sum::<f64>() / w.len() as f64;
        let peak = (0..n)
            .filter(|&i| cycles[i] >= q)
            .map(|i| fluorescence[i])
            .filter(|v| v.is_finite())
            .fold(f64::NEG_INFINITY, f64::max);
        if peak - level < thr {
            cq = None;
        }
    }
    Some(CqCall {
        cq,
        threshold: thr,
        threshold_source: src,
        baseline_start: start,
        baseline_end: end,
        baseline_intercept: intercept,
        baseline_slope: slope,
    })
}

/// −dF/dT of a melt curve by central differences (one-sided at the ends); temperatures that do
/// not increase give NaN.
pub(crate) fn melt_derivative(t: &[f64], f: &[f64]) -> Vec<f64> {
    let n = t.len().min(f.len());
    (0..n)
        .map(|i| {
            let (lo, hi) = (i.saturating_sub(1), (i + 1).min(n - 1));
            let dt = t[hi] - t[lo];
            if hi == lo || dt.is_nan() || dt <= 0.0 {
                return f64::NAN;
            }
            -(f[hi] - f[lo]) / dt
        })
        .collect()
}

/// Mean and sample standard deviation of the finite values.
pub(crate) fn mean_sd(v: &[f64]) -> (Option<f64>, Option<f64>, usize) {
    let f: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    if f.is_empty() {
        return (None, None, 0);
    }
    let n = f.len() as f64;
    let m = f.iter().sum::<f64>() / n;
    let sd =
        (f.len() > 1).then(|| (f.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (n - 1.0)).sqrt());
    (Some(m), sd, f.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A logistic amplification curve on a sloped baseline.
    fn curve(mid: f64) -> (Vec<f64>, Vec<f64>) {
        let c: Vec<f64> = (1..=40).map(f64::from).collect();
        let f = c
            .iter()
            .map(|x| 1000.0 + 2.0 * x + 50_000.0 / (1.0 + (-(x - mid) / 1.5).exp()))
            .collect();
        (c, f)
    }

    #[test]
    fn cq_of_a_logistic_curve() {
        let (c, f) = curve(30.0);
        let call = threshold_cq(&c, &f, Some(25_000.0), Baseline::Window(3, 15)).unwrap();
        // the logistic reaches half its height at the midpoint
        assert!((call.cq.unwrap() - 30.0).abs() < 0.05, "{call:?}");
        // the curve's foot leaks slightly into cycles 3-15
        assert!((call.baseline_slope - 2.0).abs() < 0.2, "{call:?}");
        let earlier = threshold_cq(&c, &f, Some(1000.0), Baseline::Auto)
            .unwrap()
            .cq
            .unwrap();
        assert!(earlier < 30.0);
        let auto = threshold_cq(&c, &f, None, Baseline::Auto).unwrap();
        assert_eq!(auto.threshold_source, "auto");
        // no amplification: flat noise-free line → no SD, no auto threshold
        let flat = vec![100.0; 40];
        assert!(threshold_cq(&c, &flat, None, Baseline::Auto).is_none());
        let never = threshold_cq(&c, &flat, Some(10.0), Baseline::Auto).unwrap();
        assert_eq!(never.cq, None);
        assert!(threshold_cq(&c[..3], &f[..3], None, Baseline::Auto).is_none());
    }

    #[test]
    fn fit_and_derivative() {
        let (a, b, r2) = linear_fit(&[1.0, 2.0, 3.0], &[3.0, 5.0, 7.0]).unwrap();
        assert!((a - 1.0).abs() < 1e-12 && (b - 2.0).abs() < 1e-12 && (r2 - 1.0).abs() < 1e-12);
        assert!(linear_fit(&[1.0], &[1.0]).is_none());
        let d = melt_derivative(&[60.0, 61.0, 62.0], &[10.0, 8.0, 4.0]);
        assert_eq!(d, vec![2.0, 3.0, 4.0]);
        assert_eq!(
            mean_sd(&[1.0, 3.0, f64::NAN]),
            (Some(2.0), Some(2f64.sqrt()), 2)
        );
    }
}
