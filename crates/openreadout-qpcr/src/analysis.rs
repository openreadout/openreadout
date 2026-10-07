//! Our own qPCR analysis (documented in `docs/formats/qpcr.md` → "Analysis"): threshold-cycle
//! Cq with linear baseline subtraction, melt-curve −dF/dT, and least-squares helpers for
//! standard curves. Vendor-computed values are never overwritten; these results are reported
//! next to them.

/// Default baseline window (1-based cycles, inclusive), the common instrument default.
pub(crate) const DEFAULT_BASELINE: (u32, u32) = (3, 15);
/// Automatic threshold: this many standard deviations of the baseline residuals.
pub(crate) const AUTO_THRESHOLD_SDS: f64 = 10.0;

/// Empirical LC480 estimator, not a recovered implementation of Roche's algorithm.
/// Parameters and development-only agreement are recorded in the provenance log.
pub(crate) fn second_derivative_cq(cycles: &[f64], fluorescence: &[f64]) -> Option<CqCall> {
    if cycles.len() != fluorescence.len()
        || cycles.len() < 18
        || cycles.first().copied() != Some(1.0)
        || cycles.iter().chain(fluorescence).any(|v| !v.is_finite())
        || cycles.windows(2).any(|w| (w[1] - w[0] - 1.0).abs() > 1e-9)
    {
        return None;
    }
    let x = cycles.get(2..15)?;
    let y = fluorescence.get(2..15)?;
    let (intercept, slope, _) = linear_fit(x, y)?;
    let residuals: Vec<_> = x
        .iter()
        .zip(y)
        .map(|(x, y)| y - intercept - slope * x)
        .collect();
    let (_, sd, _) = mean_sd(&residuals);
    let sd = sd?;
    let level = y.iter().sum::<f64>() / y.len() as f64;
    let peak = fluorescence.iter().copied().reduce(f64::max)?;
    if !intercept.is_finite()
        || !slope.is_finite()
        || !sd.is_finite()
        || !level.is_finite()
        || !(50.0 * sd).is_finite()
    {
        return None;
    }
    let mut call = CqCall {
        cq: None,
        threshold: 50.0 * sd,
        threshold_source: "auto",
        baseline_start: 3,
        baseline_end: 15,
        baseline_intercept: intercept,
        baseline_slope: slope,
    };
    if peak - level <= call.threshold || peak <= level {
        return Some(call);
    }
    // Least-squares quadratic on offsets -4..4. Subtract the centre to avoid
    // cancellation of a large constant background. Twice the quadratic coefficient
    // is sum((3*x*x - 20)*y) / 462.
    let derivative: Vec<f64> = fluorescence
        .windows(9)
        .map(|w| {
            w.iter()
                .zip([28.0, 7.0, -8.0, -17.0, -20.0, -17.0, -8.0, 7.0, 28.0])
                .map(|(y, weight)| weight * (y - w[4]))
                .sum::<f64>()
                / 462.0
        })
        .collect();
    if derivative.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let (index, &height) = derivative
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))?;
    if height <= 0.0 {
        return Some(call);
    }
    let start = index.checked_sub(3)?;
    let window = derivative.get(start..index.checked_add(4)?)?;
    // Quadratic least squares on offsets -3..3: slope=sum(x*y)/28,
    // curvature coefficient=sum((x*x-4)*y)/84.
    let slope = window
        .iter()
        .zip(-3..=3)
        .map(|(y, x)| y * f64::from(x))
        .sum::<f64>()
        / 28.0;
    let curvature = window
        .iter()
        .zip(-3..=3)
        .map(|(y, x)| y * f64::from(x * x - 4))
        .sum::<f64>()
        / 84.0;
    if curvature >= 0.0 || !curvature.is_finite() || !slope.is_finite() {
        return None;
    }
    let offset = -slope / (2.0 * curvature);
    if !offset.is_finite() || offset.abs() > 3.0 {
        return None;
    }
    call.cq = Some(*cycles.get(index.checked_add(4)?)? + offset);
    Some(call)
}

/// Stored settings with local cubic interpolation in log fluorescence. Keep the
/// log-linear crossing when four regular, positive samples cannot support a
/// monotone cubic. The vendor's interpolation itself is not publicly specified.
pub(crate) fn stored_threshold_cq(
    cycles: &[f64],
    fluorescence: &[f64],
    threshold: f64,
    baseline: Baseline,
) -> Option<CqCall> {
    let mut call = threshold_cq(cycles, fluorescence, Some(threshold), baseline)?;
    let Some(cq) = call.cq else { return Some(call) };
    if !cq.is_finite() {
        return None;
    }
    let Some(left) = cycles.windows(2).position(|w| w[0] <= cq && cq <= w[1]) else {
        return Some(call);
    };
    let Some(start) = left.checked_sub(1) else {
        return Some(call);
    };
    let Some(end) = start.checked_add(4) else {
        return Some(call);
    };
    let Some(cycle) = cycles.get(start..end) else {
        return Some(call);
    };
    let Some(raw) = fluorescence.get(start..end) else {
        return Some(call);
    };
    let step = cycle[2] - cycle[1];
    if !step.is_finite()
        || step <= 0.0
        || cycle
            .windows(2)
            .any(|w| ((w[1] - w[0]) / step - 1.0).abs() > 1e-9)
    {
        return Some(call);
    }
    let mut log_values = [0.0; 4];
    for (i, (&cycle, &raw)) in cycle.iter().zip(raw).enumerate() {
        let corrected = raw - call.baseline_intercept - call.baseline_slope * cycle;
        if !corrected.is_finite() || corrected <= 0.0 {
            return Some(call);
        }
        log_values[i] = corrected.ln() - threshold.ln();
    }
    if log_values.windows(2).any(|w| w[1] <= w[0]) || log_values[1] > 0.0 || log_values[2] < 0.0 {
        return Some(call);
    }
    // Interpolate at offsets -1, 0, 1, 2 from the left crossing sample.
    let linear =
        (-2.0 * log_values[0] - 3.0 * log_values[1] + 6.0 * log_values[2] - log_values[3]) / 6.0;
    let quadratic = f64::midpoint(log_values[0], log_values[2]) - log_values[1];
    let cubic = (-log_values[0] + 3.0 * log_values[1] - 3.0 * log_values[2] + log_values[3]) / 6.0;
    let derivative = |offset: f64| linear + offset * (2.0 * quadratic + 3.0 * cubic * offset);
    let vertex = -quadratic / (3.0 * cubic);
    if derivative(0.0) <= 0.0
        || derivative(1.0) <= 0.0
        || (cubic > 0.0 && (0.0..1.0).contains(&vertex) && derivative(vertex) <= 0.0)
    {
        return Some(call);
    }
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..48 {
        let offset = f64::midpoint(lo, hi);
        if log_values[1] + offset * (linear + offset * (quadratic + offset * cubic)) < 0.0 {
            lo = offset;
        } else {
            hi = offset;
        }
    }
    call.cq = Some(cycle[1] + step * f64::midpoint(lo, hi));
    Some(call)
}

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
    if cycles.len() != fluorescence.len()
        || cycles.iter().chain(fluorescence).any(|v| !v.is_finite())
        || cycles.windows(2).any(|w| w[1] <= w[0])
    {
        return None;
    }
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
    if start.checked_add(2).is_none_or(|min_end| end < min_end) {
        // too short a window: the first quarter of the run (at least 3 cycles)
        start = 1;
        end = (last / 4).max(3).min(last.saturating_sub(1));
    }
    let (si, ei) = (start as usize - 1, end as usize);
    let (intercept, slope, _) = linear_fit(cycles.get(si..ei)?, fluorescence.get(si..ei)?)?;
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

    #[test]
    fn stored_threshold_interpolates_log_curvature() {
        let cycles: Vec<_> = (1..=8).map(f64::from).collect();
        let signal: Vec<_> = cycles.iter().map(|x| (x * x / 10.0).exp()).collect();
        let threshold = (2.5_f64.powi(2) / 10.0).exp();
        let call =
            stored_threshold_cq(&cycles, &signal, threshold, Baseline::Line(0.0, 0.0)).unwrap();
        assert!((call.cq.unwrap() - 2.5).abs() < 1e-12);
        // Without a full four-point neighborhood, the crossing remains log-linear.
        let short = stored_threshold_cq(
            &cycles[1..3],
            &signal[1..3],
            threshold,
            Baseline::Line(0.0, 0.0),
        )
        .unwrap();
        assert!((short.cq.unwrap() - 2.45).abs() < 1e-12);
    }

    #[test]
    fn second_derivative_is_invariant_to_fluorescence_units() {
        let cycles: Vec<_> = (1..=45).map(f64::from).collect();
        let signal: Vec<_> = cycles
            .iter()
            .map(|x| 100.0 + 1000.0 / (1.0 + (-0.5 * (x - 25.0)).exp()))
            .collect();
        let cq = second_derivative_cq(&cycles, &signal).unwrap().cq.unwrap();
        assert!((20.0..23.0).contains(&cq));
        for (gain, background) in [(0.001, 0.0), (2.0, 100_000.0)] {
            let scaled: Vec<_> = signal.iter().map(|y| gain * y + background).collect();
            let other = second_derivative_cq(&cycles, &scaled).unwrap().cq.unwrap();
            assert!((cq - other).abs() < 1e-8);
        }
        let flat = vec![100.0; cycles.len()];
        assert_eq!(second_derivative_cq(&cycles, &flat).unwrap().cq, None);
    }

    #[test]
    fn second_derivative_rejects_invalid_sampling_and_unresolved_peaks() {
        let cycles: Vec<_> = (1..=45).map(f64::from).collect();
        let late: Vec<_> = cycles.iter().map(|x| (x / 3.0).exp()).collect();
        // A peak outside the complete fitting window is unavailable, not negative.
        assert!(second_derivative_cq(&cycles, &late).is_none());
        assert!(second_derivative_cq(&cycles[..17], &late[..17]).is_none());
        assert!(second_derivative_cq(&cycles, &late[..44]).is_none());
        let mut bad = late.clone();
        bad[20] = f64::NAN;
        assert!(second_derivative_cq(&cycles, &bad).is_none());
        let mut uneven = cycles.clone();
        uneven[20] += 0.1;
        assert!(second_derivative_cq(&uneven, &late).is_none());
        let huge: Vec<_> = cycles.iter().map(|x| x * 1e305).collect();
        assert!(second_derivative_cq(&cycles, &huge).is_none());
    }

    #[test]
    fn malformed_curves_do_not_produce_cq() {
        for (cycles, fluorescence) in [
            (vec![1.0, 2.0], vec![1.0]),
            (vec![2.0, 1.0], vec![1.0, 2.0]),
            (vec![1.0, 1.0], vec![1.0, 2.0]),
            (vec![1.0, f64::NAN], vec![1.0, 2.0]),
            (vec![1.0, 2.0], vec![1.0, f64::INFINITY]),
        ] {
            assert!(
                threshold_cq(&cycles, &fluorescence, Some(1.5), Baseline::Line(0.0, 0.0)).is_none()
            );
        }
        let (c, f) = curve(30.0);
        // Extreme caller-supplied windows must not overflow or index outside the curve.
        let _ = threshold_cq(&c, &f, Some(10.0), Baseline::Window(u32::MAX, u32::MAX));
    }

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
