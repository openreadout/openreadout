//! Per-well kinetic metrics (enzyme-style reads) and growth-curve metrics (OD over hours).
//!
//! Definitions (book/src/guides/plate-analysis.md "Kinetics", "Growth curves"):
//!
//! - **window**: points per sliding window; default 5 or a tenth of the time points, whichever
//!   is larger.
//! - **max slope** (Vmax): the largest ordinary-least-squares slope over every run of `window`
//!   consecutive time points (all points when there are fewer); its time is the mean time of
//!   that window.
//! - **lag time**: where the max-slope line crosses the baseline, the lowest value up to the end
//!   of the max-slope window: t̄ − (ȳ − baseline)/slope, (t̄, ȳ) the window's centroid.
//! - **mean slope**: the OLS slope over all points.
//! - **AUC**: trapezoidal area under the values over the whole time range (value × s).
//! - **growth rate** µmax: the largest OLS slope of ln(value) against time (per hour) over every
//!   run of `window` consecutive points among the points above the threshold (default 5 % of the
//!   curve's maximum); doubling time ln 2 / µmax; exponential phase = that window; lag time where
//!   the µmax line crosses ln y0, y0 the mean of the first three values (at least the threshold).
//! - **logistic fit**: N(t) = K / (1 + ((K − N0)/N0) e^(−r t)) by least squares (the model
//!   growthcurver fits), t in hours: K, N0, r, t_mid = ln((K − N0)/N0)/r, doubling time ln 2/r.

use crate::fit;
use crate::stats;

/// Kinetic metrics of one well.
#[derive(Debug, Clone, PartialEq)]
pub struct Kinetic {
    /// Points used (finite values).
    pub n_points: usize,
    /// Window length used for the max slope (points).
    pub window: usize,
    /// Largest windowed slope, value units per second.
    pub max_slope_per_s: f64,
    /// r² of the max-slope window.
    pub max_slope_r_squared: f64,
    /// Mean time of the max-slope window (s).
    pub time_at_max_slope_s: f64,
    /// First and last time of the max-slope window (s).
    pub window_start_s: f64,
    /// Last time of the max-slope window (s).
    pub window_end_s: f64,
    /// Lag time (s), when the max slope is positive.
    pub lag_time_s: Option<f64>,
    /// OLS slope over all points, value units per second.
    pub mean_slope_per_s: f64,
    /// r² of the all-points line.
    pub mean_slope_r_squared: f64,
    /// Largest value.
    pub max_value: f64,
    /// Time of the largest value (s).
    pub time_to_max_s: f64,
    /// Smallest value.
    pub min_value: f64,
    /// First and last value.
    pub initial_value: f64,
    /// Last value.
    pub final_value: f64,
    /// Trapezoidal area under the values (value × s).
    pub auc: f64,
}

/// Finite (t, y) pairs sorted by time.
pub fn clean(t: &[f64], y: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let mut pts: Vec<(f64, f64)> = t
        .iter()
        .zip(y)
        .filter(|(a, b)| a.is_finite() && b.is_finite())
        .map(|(a, b)| (*a, *b))
        .collect();
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    pts.into_iter().unzip()
}

/// Max-slope window: (start index, slope, intercept, r²).
fn best_window(t: &[f64], y: &[f64], window: usize) -> Option<(usize, f64, f64, f64)> {
    let n = t.len();
    let h = window.min(n);
    if h < 2 {
        return None;
    }
    let mut best: Option<(usize, f64, f64, f64)> = None;
    for s in 0..=n - h {
        if let Some((slope, icpt, r2)) = stats::ols(&t[s..s + h], &y[s..s + h])
            && best.is_none_or(|b| slope > b.1)
        {
            best = Some((s, slope, icpt, r2));
        }
    }
    best
}

/// Default growth threshold as a fraction of each well's maximum: values at or below it are left
/// out of the log-scale fit (near the background, reader noise dominates ln(value)).
pub const GROWTH_THRESHOLD_FRACTION: f64 = 0.05;

/// Default window for `n` time points: 5 points or a tenth of the time course, whichever is
/// larger (so densely sampled reads are not dominated by noise).
pub fn default_window(n: usize) -> usize {
    5.max(n.div_ceil(10))
}

/// Kinetic metrics of a time series (t in seconds). `window`: points per sliding window
/// (default [`default_window`]). `None` below two finite points.
pub fn kinetic(t: &[f64], y: &[f64], window: Option<usize>) -> Option<Kinetic> {
    let (t, y) = clean(t, y);
    let n = t.len();
    if n < 2 {
        return None;
    }
    let h = window.unwrap_or_else(|| default_window(n)).clamp(2, n);
    let (s, slope, _, r2) = best_window(&t, &y, h)?;
    let tw = &t[s..s + h];
    let yw = &y[s..s + h];
    let tc = stats::mean(tw)?;
    let yc = stats::mean(yw)?;
    let baseline = y[..s + h].iter().copied().fold(f64::INFINITY, f64::min);
    let lag = (slope > 0.0).then(|| tc - (yc - baseline) / slope);
    let (mslope, _, mr2) = stats::ols(&t, &y)?;
    let (imax, max_value) =
        y.iter()
            .copied()
            .enumerate()
            .fold(
                (0, f64::NEG_INFINITY),
                |a, (i, v)| if v > a.1 { (i, v) } else { a },
            );
    Some(Kinetic {
        n_points: n,
        window: h,
        max_slope_per_s: slope,
        max_slope_r_squared: r2,
        time_at_max_slope_s: tc,
        window_start_s: tw[0],
        window_end_s: tw[h - 1],
        lag_time_s: lag,
        mean_slope_per_s: mslope,
        mean_slope_r_squared: mr2,
        max_value,
        time_to_max_s: t[imax],
        min_value: y.iter().copied().fold(f64::INFINITY, f64::min),
        initial_value: y[0],
        final_value: y[n - 1],
        auc: stats::trapezoid(&t, &y),
    })
}

/// Growth metrics of one well.
#[derive(Debug, Clone, PartialEq)]
pub struct Growth {
    /// Points used (finite values).
    pub n_points: usize,
    /// Threshold below which points are left out of the log-scale fit.
    pub threshold: f64,
    /// Window length (points).
    pub window: usize,
    /// µmax, per hour, when an exponential phase was found.
    pub growth_rate_per_h: Option<f64>,
    /// ln 2 / µmax, hours.
    pub doubling_time_h: Option<f64>,
    /// r² of the log-scale fit in the exponential phase.
    pub exp_r_squared: Option<f64>,
    /// Exponential phase: first and last time (s).
    pub exp_phase_start_s: Option<f64>,
    /// Last time of the exponential phase (s).
    pub exp_phase_end_s: Option<f64>,
    /// Lag time on the log scale (s): where the µmax line crosses ln y0, y0 the mean of the first
    /// three values (at least the threshold).
    pub lag_time_s: Option<f64>,
    /// Largest value.
    pub max_value: f64,
    /// Time of the largest value (s).
    pub time_to_max_s: f64,
    /// Trapezoidal area under the values (value × h).
    pub auc_h: f64,
    /// Logistic fit, when it converged to a positive carrying capacity and rate.
    pub logistic: Option<LogisticGrowth>,
}

/// N(t) = K / (1 + ((K − N0)/N0) e^(−r t)) fitted to one well (t in hours from the first point).
#[derive(Debug, Clone, PartialEq)]
pub struct LogisticGrowth {
    /// Carrying capacity K.
    pub k: f64,
    /// Value at t = 0 (the first time point).
    pub n0: f64,
    /// Growth rate r, per hour.
    pub r_per_h: f64,
    /// Time of the inflection (s, on the file's clock).
    pub t_mid_s: f64,
    /// ln 2 / r, hours.
    pub doubling_time_h: f64,
    /// Residual standard deviation √(SSE/(n − 3)).
    pub sigma: f64,
    /// Whether the optimiser converged.
    pub converged: bool,
}

fn logistic_value(p: &[f64], t: f64, grad: Option<&mut [f64]>) -> f64 {
    let (k, n0, r) = (p[0], p[1], p[2]);
    let e = (-r * t).exp();
    let a = (k - n0) / n0;
    let d = 1.0 + a * e;
    let v = k / d;
    if let Some(g) = grad {
        let kd2 = k / (d * d);
        g[0] = 1.0 / d - kd2 * e / n0;
        g[1] = kd2 * e * k / (n0 * n0);
        g[2] = kd2 * a * t * e;
    }
    v
}

/// Fit the logistic growth model (t in hours from the first time point).
pub fn logistic_growth(t_h: &[f64], y: &[f64], r_guess: Option<f64>) -> Option<LogisticGrowth> {
    let n = t_h.len();
    if n < 4 {
        return None;
    }
    let ymax = y.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if ymax.is_nan() || ymax <= 0.0 {
        return None;
    }
    let span = t_h[n - 1] - t_h[0];
    let n0_guess = y[0].max(ymax * 1e-3);
    let r0 = r_guess.filter(|r| *r > 0.0).unwrap_or(4.0 / span.max(1e-9));
    let w = vec![1.0; n];
    let ok = |p: &[f64]| {
        p.iter().all(|v| v.is_finite())
            && p[0] > 0.0
            && p[1] > 0.0
            && p[2] > 0.0
            && p[1] < 1e3 * p[0].max(1e-300)
            && p[2] < 1e4
    };
    let mut best: Option<fit::LmOutcome> = None;
    for (kf, rf) in [(1.0, 1.0), (1.0, 0.5), (1.0, 2.0), (1.2, 1.0)] {
        let start = vec![ymax * kf, n0_guess, r0 * rf];
        let o = fit::levenberg_marquardt(&logistic_value, &ok, start, t_h, y, &w);
        if o.sse.is_finite() && best.as_ref().is_none_or(|b| o.sse < b.sse) {
            best = Some(o);
        }
    }
    let o = best?;
    let (k, n0, r) = (o.theta[0], o.theta[1], o.theta[2]);
    if !(k > 0.0 && n0 > 0.0 && r > 0.0) {
        return None;
    }
    let t_mid_h = ((k - n0).abs() / n0).ln() / r;
    Some(LogisticGrowth {
        k,
        n0,
        r_per_h: r,
        t_mid_s: t_mid_h * 3600.0,
        doubling_time_h: std::f64::consts::LN_2 / r,
        sigma: (o.sse / (n - 3) as f64).sqrt(),
        converged: o.converged,
    })
}

/// Growth metrics of a time series (t in seconds, values already background-corrected).
/// `threshold`: values at or below it are left out of the log-scale fit (default 5 % of the
/// maximum).
pub fn growth(
    t: &[f64],
    y: &[f64],
    window: Option<usize>,
    threshold: Option<f64>,
) -> Option<Growth> {
    let (t, y) = clean(t, y);
    let n = t.len();
    if n < 2 {
        return None;
    }
    let (imax, max_value) =
        y.iter()
            .copied()
            .enumerate()
            .fold(
                (0, f64::NEG_INFINITY),
                |a, (i, v)| if v > a.1 { (i, v) } else { a },
            );
    let thr = threshold.unwrap_or(GROWTH_THRESHOLD_FRACTION * max_value.max(0.0));
    let t_h: Vec<f64> = t.iter().map(|v| v / 3600.0).collect();
    // log-scale sliding window over the points above the threshold, kept in time order
    let (lt, ly): (Vec<f64>, Vec<f64>) = t_h
        .iter()
        .zip(&y)
        .filter(|(_, v)| **v > thr && **v > 0.0)
        .map(|(a, b)| (*a, b.ln()))
        .unzip();
    let h = window.unwrap_or_else(|| default_window(n)).max(2);
    let mut out = Growth {
        n_points: n,
        threshold: thr,
        window: h,
        growth_rate_per_h: None,
        doubling_time_h: None,
        exp_r_squared: None,
        exp_phase_start_s: None,
        exp_phase_end_s: None,
        lag_time_s: None,
        max_value,
        time_to_max_s: t[imax],
        auc_h: stats::trapezoid(&t_h, &y),
        logistic: None,
    };
    if lt.len() >= h
        && let Some((s, mu, _, r2)) = best_window(&lt, &ly, h)
        && mu > 0.0
    {
        out.growth_rate_per_h = Some(mu);
        out.doubling_time_h = Some(std::f64::consts::LN_2 / mu);
        out.exp_r_squared = Some(r2);
        out.exp_phase_start_s = Some(lt[s] * 3600.0);
        out.exp_phase_end_s = Some(lt[s + h - 1] * 3600.0);
        let tc = stats::mean(&lt[s..s + h])?;
        let yc = stats::mean(&ly[s..s + h])?;
        // baseline: the mean of the first three values, at least the threshold
        let y0 = stats::mean(&y[..n.min(3)])?.max(thr).max(f64::MIN_POSITIVE);
        out.lag_time_s = Some((tc - (yc - y0.ln()) / mu) * 3600.0);
    }
    // logistic fit on hours from the first point
    let t0 = t_h[0];
    let rel: Vec<f64> = t_h.iter().map(|v| v - t0).collect();
    out.logistic = logistic_growth(&rel, &y, out.growth_rate_per_h).map(|mut l| {
        l.t_mid_s += t0 * 3600.0;
        l
    });
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_kinetics() {
        let t: Vec<f64> = (0..10).map(|i| f64::from(i) * 60.0).collect();
        // flat for 3 points, then rising 0.01/s
        let y: Vec<f64> = t
            .iter()
            .map(|&s| {
                if s < 180.0 {
                    0.1
                } else {
                    0.1 + 0.01 * (s - 180.0)
                }
            })
            .collect();
        let k = kinetic(&t, &y, Some(3)).unwrap();
        assert!((k.max_slope_per_s - 0.01).abs() < 1e-12);
        assert!((k.lag_time_s.unwrap() - 180.0).abs() < 1e-9, "{k:?}");
        assert!((k.max_value - y[9]).abs() < 1e-12);
        assert_eq!(k.time_to_max_s, 540.0);
        assert!(kinetic(&[0.0], &[1.0], Some(3)).is_none());
    }

    #[test]
    fn exponential_growth() {
        // N = K / (1 + ((K − N0)/N0) e^(−r t)), r = 0.8/h
        let (k, n0, r) = (1.2, 0.01, 0.8);
        let t: Vec<f64> = (0..97).map(|i| f64::from(i) * 900.0).collect();
        let y: Vec<f64> = t
            .iter()
            .map(|&s| k / (1.0 + ((k - n0) / n0) * (-r * s / 3600.0).exp()))
            .collect();
        let g = growth(&t, &y, Some(5), None).unwrap();
        let l = g.logistic.unwrap();
        assert!(
            (l.r_per_h - r).abs() < 1e-6 && (l.k - k).abs() < 1e-6,
            "{l:?}"
        );
        // above the 5 % threshold d ln N/dt = r (1 − N/K) ≤ 0.95 r: µmax a little below r
        let mu = g.growth_rate_per_h.unwrap();
        assert!(mu < r && mu > 0.85 * r, "{mu}");
    }
}
