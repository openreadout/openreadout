//! Passive membrane properties from a current step (current clamp) and test-pulse metrics from
//! a voltage step (voltage clamp).
//!
//! Windows, for a step from sample `s0` to `s1`:
//! - baseline: the mean over the last 10 % of the time before the step (`[0.9·s0, s0)`), as eFEL
//!   defines `voltage_base`;
//! - steady state: the mean over the last 10 % of the step (eFEL `steady_state_voltage_stimend`);
//! - extreme: the minimum (hyperpolarising step) or maximum within the step.
//!
//! Current clamp: `input_resistance_mohm = (steady − baseline) / ΔI` (mV/pA × 1000);
//! `sag_ratio = (steady − min) / (baseline − min)` (eFEL `sag_ratio1`: the share of the peak
//! deflection that relaxes); `steady_fraction = (baseline − steady) / (baseline − min)` (eFEL
//! `sag_ratio2`); `tau_ms`: the time constant of `V(t) = C + A·e^{−t/τ}` fitted by least squares
//! from the step onset to the voltage extreme (at least 10 samples; `A` and `C` solved linearly
//! for each τ, τ found by golden-section search between 0.05 ms and 10 × the fit window).
//!
//! Voltage clamp (a step of `ΔV` mV): `holding_current_pa` = baseline current;
//! `peak_current_pa` = the extreme current in the first 2 ms of the step minus baseline;
//! `steady_current_pa` = steady state minus baseline; `total_resistance_mohm = ΔV / steady`;
//! `tau_ms`: single-exponential fit, decaying to the measured steady state, of the transient from
//! where it has fallen to 90 % of its height to where it first reaches the steady state (at most
//! 50 ms or half the step);
//! `access_resistance_mohm = ΔV / I0`, `I0` the fit extrapolated back to the transient's apex
//! (relative to baseline; the measured peak when the fit fails); `membrane_resistance_mohm` =
//! total − access; `capacitance_pf = Q/ΔV · ((Ra + Rm)/Rm)²`, `Q` the charge of the transient above
//! the steady state from the step onset to its first return to the steady state (exact for a
//! single-compartment cell behind a series resistance).

use serde::{Deserialize, Serialize};

fn mean(v: &[f64]) -> Option<f64> {
    let f: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    (!f.is_empty()).then(|| f.iter().sum::<f64>() / f.len() as f64)
}

/// Baseline and steady-state means of `y` around a step `[s0, s1)` (module docs).
pub fn base_and_steady(y: &[f64], s0: usize, s1: usize) -> (Option<f64>, Option<f64>) {
    let s1 = s1.min(y.len());
    if s0 >= s1 {
        return (None, None);
    }
    let b0 = (s0 as f64 * 0.9).floor() as usize;
    let base = if s0 > b0 { mean(&y[b0..s0]) } else { None };
    let len = s1 - s0;
    let t0 = s1 - (len / 10).max(1);
    (base, mean(&y[t0..s1]))
}

/// Least-squares fit of `y_k = C + A·e^{−k·dt/τ}`; returns `(τ, A, C, rms)` with τ in the units
/// of `dt`. `None` for fewer than 10 points or a non-finite fit.
pub fn fit_exponential(y: &[f64], dt: f64) -> Option<(f64, f64, f64, f64)> {
    let n = y.len();
    if n < 10 || !(dt > 0.0) || y.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let sse_at = |tau: f64| -> (f64, f64, f64) {
        // linear least squares for A, C with basis e_k = exp(-k dt / tau)
        let (mut se, mut see, mut sy, mut sey) = (0.0, 0.0, 0.0, 0.0);
        for (k, &v) in y.iter().enumerate() {
            let e = (-(k as f64) * dt / tau).exp();
            se += e;
            see += e * e;
            sy += v;
            sey += e * v;
        }
        let m = n as f64;
        let det = m * see - se * se;
        if det.abs() < 1e-300 {
            return (f64::INFINITY, 0.0, 0.0);
        }
        let a = (m * sey - se * sy) / det;
        let c = (sy - a * se) / m;
        let sse: f64 = y
            .iter()
            .enumerate()
            .map(|(k, &v)| {
                let r = v - c - a * (-(k as f64) * dt / tau).exp();
                r * r
            })
            .sum();
        (sse, a, c)
    };
    // golden-section search on log(tau)
    let (mut lo, mut hi) = ((0.05e-3_f64.min(dt)).ln(), (10.0 * n as f64 * dt).ln());
    let g = 0.5 * (5.0_f64.sqrt() - 1.0);
    let mut x1 = hi - g * (hi - lo);
    let mut x2 = lo + g * (hi - lo);
    let (mut f1, mut f2) = (sse_at(x1.exp()).0, sse_at(x2.exp()).0);
    for _ in 0..100 {
        if f1 < f2 {
            hi = x2;
            x2 = x1;
            f2 = f1;
            x1 = hi - g * (hi - lo);
            f1 = sse_at(x1.exp()).0;
        } else {
            lo = x1;
            x1 = x2;
            f1 = f2;
            x2 = lo + g * (hi - lo);
            f2 = sse_at(x2.exp()).0;
        }
        if (hi - lo).abs() < 1e-7 {
            break;
        }
    }
    let tau = f64::midpoint(lo, hi).exp();
    let (sse, a, c) = sse_at(tau);
    (sse.is_finite() && tau.is_finite()).then(|| (tau, a, c, (sse / n as f64).sqrt()))
}

/// Least-squares fit of `y_k = A·e^{−k·dt/τ}` (decay to zero); returns `(τ, A)`. `None` for
/// fewer than 4 points, a non-positive start or a non-finite fit.
pub fn fit_decay(y: &[f64], dt: f64) -> Option<(f64, f64)> {
    let n = y.len();
    if n < 4 || !(dt > 0.0) || y.iter().any(|v| !v.is_finite()) || !(y[0] > 0.0) {
        return None;
    }
    let at = |tau: f64| -> (f64, f64) {
        let (mut sye, mut see) = (0.0, 0.0);
        for (k, &v) in y.iter().enumerate() {
            let e = (-(k as f64) * dt / tau).exp();
            sye += v * e;
            see += e * e;
        }
        let a = if see > 0.0 { sye / see } else { 0.0 };
        let sse: f64 = y
            .iter()
            .enumerate()
            .map(|(k, &v)| (v - a * (-(k as f64) * dt / tau).exp()).powi(2))
            .sum();
        (sse, a)
    };
    let (mut lo, mut hi) = ((0.01 * dt).ln(), (100.0 * n as f64 * dt).ln());
    let g = 0.5 * (5.0_f64.sqrt() - 1.0);
    let mut x1 = hi - g * (hi - lo);
    let mut x2 = lo + g * (hi - lo);
    let (mut f1, mut f2) = (at(x1.exp()).0, at(x2.exp()).0);
    for _ in 0..200 {
        if f1 < f2 {
            hi = x2;
            x2 = x1;
            f2 = f1;
            x1 = hi - g * (hi - lo);
            f1 = at(x1.exp()).0;
        } else {
            lo = x1;
            x1 = x2;
            f1 = f2;
            x2 = lo + g * (hi - lo);
            f2 = at(x2.exp()).0;
        }
        if (hi - lo).abs() < 1e-9 {
            break;
        }
    }
    let tau = f64::midpoint(lo, hi).exp();
    let (sse, a) = at(tau);
    (sse.is_finite() && tau.is_finite() && a.is_finite()).then_some((tau, a))
}

/// Passive response to one current step.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StepResponse {
    /// Baseline voltage, mV.
    pub baseline_mv: Option<f64>,
    /// Steady-state voltage at the end of the step, mV.
    pub steady_state_mv: Option<f64>,
    /// Extreme voltage during the step (minimum for hyperpolarising steps), mV.
    pub extreme_mv: Option<f64>,
    /// (steady − baseline) / ΔI, MΩ.
    pub input_resistance_mohm: Option<f64>,
    /// Membrane time constant, ms.
    pub tau_ms: Option<f64>,
    /// (steady − min) / (baseline − min) (eFEL sag_ratio1), hyperpolarising steps.
    pub sag_ratio: Option<f64>,
    /// (baseline − steady) / (baseline − min) (eFEL sag_ratio2), hyperpolarising steps.
    pub steady_fraction: Option<f64>,
}

/// Passive analysis of a current step of `delta_pa` from `s0` to `s1` on voltage `v` (mV).
pub fn current_step(v: &[f64], fs: f64, s0: usize, s1: usize, delta_pa: f64) -> StepResponse {
    let (base, steady) = base_and_steady(v, s0, s1);
    let s1c = s1.min(v.len());
    let mut r = StepResponse {
        baseline_mv: base,
        steady_state_mv: steady,
        ..StepResponse::default()
    };
    if s0 >= s1c {
        return r;
    }
    let seg = &v[s0..s1c];
    let hyper = delta_pa < 0.0;
    let ext = seg
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, x)| x.is_finite())
        .reduce(|a, b| {
            if (hyper && b.1 < a.1) || (!hyper && b.1 > a.1) {
                b
            } else {
                a
            }
        });
    r.extreme_mv = ext.map(|e| e.1);
    if let (Some(b), Some(s)) = (base, steady) {
        if delta_pa != 0.0 {
            r.input_resistance_mohm = Some((s - b) / delta_pa * 1000.0);
        }
        if hyper
            && let Some((_, m)) = ext
            && b - m > 0.0
        {
            r.sag_ratio = Some((s - m) / (b - m));
            r.steady_fraction = Some((b - s) / (b - m));
        }
    }
    if let Some((k, _)) = ext
        && k >= 10
    {
        r.tau_ms = fit_exponential(&seg[..=k], 1000.0 / fs).map(|f| f.0);
    }
    r
}

/// Voltage-clamp test-pulse metrics (module docs).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TestPulse {
    /// Voltage step, mV.
    pub delta_mv: f64,
    /// Current before the step, pA.
    pub holding_current_pa: Option<f64>,
    /// Transient peak minus baseline, pA.
    pub peak_current_pa: Option<f64>,
    /// Steady state minus baseline, pA.
    pub steady_current_pa: Option<f64>,
    /// ΔV / peak, MΩ.
    pub access_resistance_mohm: Option<f64>,
    /// ΔV / steady − access, MΩ.
    pub membrane_resistance_mohm: Option<f64>,
    /// ΔV / steady, MΩ.
    pub total_resistance_mohm: Option<f64>,
    /// Decay time constant of the transient, ms.
    pub tau_ms: Option<f64>,
    /// Q/ΔV · ((Ra + Rm)/Rm)² from the transient's charge, pF.
    pub capacitance_pf: Option<f64>,
}

/// Test-pulse analysis of current `i` (pA) for a step of `delta_mv` from `s0` to `s1`.
pub fn test_pulse(i: &[f64], fs: f64, s0: usize, s1: usize, delta_mv: f64) -> TestPulse {
    let (base, steady) = base_and_steady(i, s0, s1);
    let mut r = TestPulse {
        delta_mv,
        holding_current_pa: base,
        ..TestPulse::default()
    };
    let s1c = s1.min(i.len());
    if s0 >= s1c || delta_mv == 0.0 {
        return r;
    }
    let (Some(b), Some(ss)) = (base, steady) else {
        return r;
    };
    let win = ((0.002 * fs) as usize).max(2).min(s1c - s0);
    let up = delta_mv > 0.0;
    let (k, pk) = i[s0..s0 + win]
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, x)| x.is_finite())
        .fold(
            (0, if up { f64::NEG_INFINITY } else { f64::INFINITY }),
            |acc, (k, x)| {
                if (up && x > acc.1) || (!up && x < acc.1) {
                    (k, x)
                } else {
                    acc
                }
            },
        );
    if !pk.is_finite() {
        return r;
    }
    let peak = pk - b;
    let st = ss - b;
    r.peak_current_pa = Some(peak);
    r.steady_current_pa = Some(st);
    let rt = (st != 0.0).then(|| delta_mv / st * 1000.0);
    r.total_resistance_mohm = rt;
    // transient decay above the steady state, from where it has fallen to 90 % of its height to
    // where it first reaches the steady state (at most 50 ms or half the step)
    let k = s0 + k;
    let height = pk - ss;
    let sign = height.signum();
    let u = (k..s1c).find(|&q| (i[q] - ss) * sign < 0.9 * height.abs());
    let cap = (s0 + (0.05 * fs) as usize).min(s0 + (s1c - s0) / 2);
    let end = u.map(|u| (u..cap).find(|&q| (i[q] - ss) * sign <= 0.0).unwrap_or(cap));
    let fit = u.zip(end).and_then(|(u, e)| {
        let y: Vec<f64> = i.get(u..e)?.iter().map(|v| (v - ss) * sign).collect();
        fit_decay(&y, 1000.0 / fs).map(|(tau, a)| (u, tau, a))
    });
    // charge of the transient above the steady state, from the step onset to where it first
    // returns to the steady state (pA·ms = fC)
    let charge = end.and_then(|e| {
        let q: f64 = i.get(s0..e)?.iter().map(|v| v - ss).sum::<f64>() * 1000.0 / fs;
        q.is_finite().then_some(q)
    });
    let mut ra = (peak != 0.0).then(|| delta_mv / peak * 1000.0);
    if let Some((u, tau, a)) = fit {
        r.tau_ms = Some(tau);
        // the fit extrapolated back to the transient's apex, relative to the baseline
        let back = (u - k) as f64 * 1000.0 / fs;
        let i0 = sign * a * (back / tau).exp() + ss - b;
        if i0 != 0.0 && i0.is_finite() && i0.signum() == delta_mv.signum() {
            ra = Some(delta_mv / i0 * 1000.0);
        }
    }
    r.access_resistance_mohm = ra;
    let rm = match (ra, rt) {
        (Some(a), Some(t)) => Some(t - a),
        _ => None,
    };
    r.membrane_resistance_mohm = rm;
    if let (Some(q), Some(ra), Some(rm)) = (charge, ra, rm)
        && ra > 0.0
        && rm > 0.0
        && q / delta_mv > 0.0
    {
        // series-resistance circuit: the capacitor takes Q·(Ra+Rm)/Rm of charge (the
        // transient above steady state plus the membrane current it has not yet reached) and
        // ends at ΔV·Rm/(Ra+Rm); fC/mV = pF
        let k = (ra + rm) / rm;
        r.capacitance_pf = Some(q / delta_mv * k * k);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exponential_fit_recovers_tau() {
        let dt = 0.05; // ms
        let y: Vec<f64> = (0..2000)
            .map(|k| -70.0 - 10.0 * (1.0 - (-(k as f64) * dt / 20.0).exp()))
            .collect();
        let (tau, a, c, rms) = fit_exponential(&y, dt).unwrap();
        assert!((tau - 20.0).abs() < 1e-3, "{tau}");
        assert!((c - -80.0).abs() < 1e-3 && (a - 10.0).abs() < 1e-3);
        assert!(rms < 1e-6);
        assert!(fit_exponential(&y[..5], dt).is_none());
    }

    #[test]
    fn rc_cell_current_step() {
        // R = 200 MΩ, τ = 20 ms, −50 pA → −10 mV, with 20 % sag
        let fs = 20000.0;
        let n = 20000;
        let (s0, s1) = (4000, 14000);
        let v: Vec<f64> = (0..n)
            .map(|k| {
                if k < s0 || k >= s1 {
                    -70.0
                } else {
                    let t = (k - s0) as f64 / fs * 1000.0;
                    -70.0 - 10.0 * (1.0 - (-t / 20.0).exp())
                }
            })
            .collect();
        let r = current_step(&v, fs, s0, s1, -50.0);
        assert!((r.input_resistance_mohm.unwrap() - 200.0).abs() < 0.5);
        assert!((r.tau_ms.unwrap() - 20.0).abs() < 0.5, "{:?}", r.tau_ms);
        assert!(r.sag_ratio.unwrap().abs() < 1e-3);
        let empty = current_step(&v, fs, 5, 5, -50.0);
        assert!(empty.input_resistance_mohm.is_none());
    }

    #[test]
    fn test_pulse_of_an_rc_circuit() {
        // Ra 10 MΩ, Rm 190 MΩ, Cm 100 pF, step +10 mV
        let fs = 50000.0;
        let (ra, rm, cm) = (10.0, 190.0, 100.0);
        let tau = ra * rm / (ra + rm) * cm / 1000.0; // ms
        let (s0, s1) = (5000, 25000);
        let i: Vec<f64> = (0..30000)
            .map(|k| {
                if k < s0 || k >= s1 {
                    -20.0
                } else {
                    let t = (k - s0) as f64 / fs * 1000.0;
                    let ss = 10.0 / (ra + rm) * 1000.0;
                    let pk = 10.0 / ra * 1000.0;
                    -20.0 + ss + (pk - ss) * (-t / tau).exp()
                }
            })
            .collect();
        let r = test_pulse(&i, fs, s0, s1, 10.0);
        assert!((r.holding_current_pa.unwrap() - -20.0).abs() < 1e-9);
        assert!((r.access_resistance_mohm.unwrap() - ra).abs() < 0.01);
        assert!((r.membrane_resistance_mohm.unwrap() - rm).abs() < 0.5);
        assert!((r.tau_ms.unwrap() - tau).abs() < 0.01);
        assert!(
            (r.capacitance_pf.unwrap() - cm).abs() < 2.0,
            "{:?}",
            r.capacitance_pf
        );
    }
}
