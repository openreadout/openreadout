//! Action-potential detection and per-spike features on a membrane-voltage trace (mV).
//!
//! **Detection.** A spike is an upward crossing of `peak_threshold_mv` (default −20 mV); it ends
//! at the next downward crossing (or the end of the window). Its peak is the largest sample in
//! between. This is the spike definition eFEL documents for `Spikecount` (voltage `Threshold`
//! −20 mV), so counts are comparable.
//!
//! **Threshold (onset).** `dV/dt` is the forward difference in V/s (= mV/ms). Searching forward
//! from the previous spike's after-hyperpolarisation minimum (or the window start) to the peak,
//! the onset is the first sample from which `dV/dt ≥ dvdt_threshold` (default 10 V/s, eFEL's
//! `DerivativeThreshold`) holds for 5 consecutive samples; when no such run exists, the sample
//! before the voltage crossing is used and `threshold_from_dvdt` is false.
//!
//! **Per-spike features** (times in s from the sweep start, durations in ms):
//! `amplitude_mv` = peak − threshold voltage; `half_width_ms`: width at the voltage half way
//! between threshold and peak (linear interpolation on both flanks); `rise_time_ms`: onset to
//! peak; `decay_time_ms`: peak to the first return to the threshold voltage; `ahp_mv`: the
//! minimum between the peak and the next spike's onset (last spike: the end of the analysis
//! window); `ahp_depth_mv` = threshold − `ahp_mv`; `upstroke_v_per_s` / `downstroke_v_per_s`:
//! the largest rising and falling `dV/dt` between onset and peak and between peak and AHP;
//! `isi_ms`: time since the previous peak.

use serde::{Deserialize, Serialize};

/// Detection settings.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ApSettings {
    /// Voltage a spike must cross upward, mV.
    pub peak_threshold_mv: f64,
    /// `dV/dt` defining the onset, V/s.
    pub dvdt_threshold_v_per_s: f64,
    /// Consecutive samples the onset criterion must hold.
    pub dvdt_window_samples: usize,
}

impl Default for ApSettings {
    fn default() -> Self {
        Self {
            peak_threshold_mv: -20.0,
            dvdt_threshold_v_per_s: 10.0,
            dvdt_window_samples: 5,
        }
    }
}

/// One action potential.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Spike {
    /// Sweep index.
    pub sweep: u32,
    /// Spike number within the sweep (0-based).
    pub index: u32,
    /// Sample index of the peak within the sweep.
    pub peak_sample: u64,
    /// Peak time from the sweep start, s.
    pub peak_time_s: f64,
    /// Peak voltage, mV.
    pub peak_mv: f64,
    /// Onset time, s.
    pub threshold_time_s: f64,
    /// Onset voltage, mV.
    pub threshold_mv: f64,
    /// True when the onset met the dV/dt criterion.
    pub threshold_from_dvdt: bool,
    /// Peak − onset voltage, mV.
    pub amplitude_mv: f64,
    /// Width at half amplitude, ms.
    pub half_width_ms: Option<f64>,
    /// Onset to peak, ms.
    pub rise_time_ms: f64,
    /// Peak to the return to the onset voltage, ms.
    pub decay_time_ms: Option<f64>,
    /// After-hyperpolarisation minimum, mV.
    pub ahp_mv: Option<f64>,
    /// Onset − AHP minimum, mV.
    pub ahp_depth_mv: Option<f64>,
    /// Largest rising dV/dt, V/s.
    pub upstroke_v_per_s: f64,
    /// Largest falling dV/dt (negative), V/s.
    pub downstroke_v_per_s: Option<f64>,
    /// Time since the previous spike's peak, ms.
    pub isi_ms: Option<f64>,
}

/// Detect spikes in `v[a..b)` (module docs). `fs` in Hz.
pub fn detect(v: &[f64], fs: f64, a: usize, b: usize, sweep: u32, s: &ApSettings) -> Vec<Spike> {
    let b = b.min(v.len());
    if b < a + 3 || !(fs > 0.0) {
        return Vec::new();
    }
    let dt_ms = 1000.0 / fs;
    let dvdt = |i: usize| (v[i + 1] - v[i]) / dt_ms; // mV/ms = V/s, valid for i + 1 < len
    let thr = s.peak_threshold_mv;
    // crossings and peaks
    let mut events: Vec<(usize, usize, usize)> = Vec::new(); // (crossing, peak, end)
    let mut i = a + 1;
    while i < b {
        if v[i - 1] < thr && v[i] >= thr {
            let c = i;
            let mut j = i;
            let mut pk = i;
            while j < b && v[j] >= thr {
                if v[j] > v[pk] {
                    pk = j;
                }
                j += 1;
            }
            events.push((c, pk, j));
            i = j + 1;
        } else {
            i += 1;
        }
    }
    let mut out: Vec<Spike> = Vec::with_capacity(events.len());
    let mut search_from = a;
    for (k, &(c, pk, end)) in events.iter().enumerate() {
        // onset
        let w = s.dvdt_window_samples.max(1);
        let mut onset = None;
        let mut j = search_from;
        while j < pk {
            if j + w < v.len() && (j..j + w).all(|q| dvdt(q) >= s.dvdt_threshold_v_per_s) {
                onset = Some(j);
                break;
            }
            j += 1;
        }
        let (on, from_dvdt) = match onset {
            Some(o) => (o, true),
            None => (c.saturating_sub(1).max(a), false),
        };
        let v_on = v[on];
        let v_pk = v[pk];
        // AHP window: to the next spike's crossing, or the window end
        let next = events.get(k + 1).map_or(b, |e| e.0);
        let (mut ahp_i, mut ahp_v) = (None, f64::INFINITY);
        for (q, &x) in v.iter().enumerate().take(next).skip(pk) {
            if x < ahp_v {
                ahp_v = x;
                ahp_i = Some(q);
            }
        }
        let ahp = ahp_i.map(|_| ahp_v);
        // half width
        let half = f64::midpoint(v_on, v_pk);
        let rise_cross = (on..pk)
            .find(|&q| v[q] < half && v[q + 1] >= half)
            .map(|q| q as f64 + (half - v[q]) / (v[q + 1] - v[q]));
        let fall_cross = (pk..next.min(v.len() - 1))
            .find(|&q| v[q] >= half && v[q + 1] < half)
            .map(|q| q as f64 + (v[q] - half) / (v[q] - v[q + 1]));
        let half_width_ms = match (rise_cross, fall_cross) {
            (Some(r), Some(f)) => Some((f - r) * dt_ms),
            _ => None,
        };
        let decay = (pk..next.min(v.len() - 1))
            .find(|&q| v[q] > v_on && v[q + 1] <= v_on)
            .map(|q| (q as f64 + (v[q] - v_on) / (v[q] - v[q + 1]) - pk as f64) * dt_ms);
        let up = (on..pk.min(v.len() - 1))
            .map(dvdt)
            .fold(f64::NEG_INFINITY, f64::max);
        let down_end = ahp_i.unwrap_or(end).min(v.len() - 1);
        let down = (pk..down_end).map(dvdt).fold(f64::INFINITY, f64::min);
        let prev_peak = out.last().map(|p: &Spike| p.peak_sample);
        out.push(Spike {
            sweep,
            index: k as u32,
            peak_sample: pk as u64,
            peak_time_s: pk as f64 / fs,
            peak_mv: v_pk,
            threshold_time_s: on as f64 / fs,
            threshold_mv: v_on,
            threshold_from_dvdt: from_dvdt,
            amplitude_mv: v_pk - v_on,
            half_width_ms,
            rise_time_ms: (pk - on) as f64 * dt_ms,
            decay_time_ms: decay,
            ahp_mv: ahp,
            ahp_depth_mv: ahp.map(|x| v_on - x),
            upstroke_v_per_s: if up.is_finite() { up } else { 0.0 },
            downstroke_v_per_s: down.is_finite().then_some(down),
            isi_ms: prev_peak.map(|p| (pk as f64 - p as f64) * dt_ms),
        });
        search_from = ahp_i.unwrap_or(end);
    }
    out
}

/// Mean over consecutive inter-spike intervals of `(ISI_{n+1} − ISI_n) / (ISI_{n+1} + ISI_n)`
/// (positive = slowing down). `None` with fewer than 3 spikes.
pub fn adaptation_index(isis: &[f64]) -> Option<f64> {
    if isis.len() < 2 {
        return None;
    }
    let v: Vec<f64> = isis
        .windows(2)
        .filter(|w| w[0] + w[1] > 0.0)
        .map(|w| (w[1] - w[0]) / (w[1] + w[0]))
        .collect();
    (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A crude spike train: rest −70 mV, spikes at `at` (samples) rising 1 mV/0.05 ms… shape.
    pub(crate) fn train(n: usize, at: &[usize]) -> Vec<f64> {
        let mut v = vec![-70.0; n];
        for &t in at {
            // 1 ms linear rise from -50 to +30, 1.5 ms fall to -75, 3 ms recovery
            for k in 0..200 {
                v[t - 200 + k] = -70.0 + 0.1 * k as f64; // slow depolarisation (2 V/s) to -50
            }
            for k in 0..20 {
                v[t + k] = -50.0 + 4.0 * k as f64;
            }
            for k in 0..30 {
                v[t + 20 + k] = 30.0 - 3.5 * k as f64;
            }
            for k in 0..60 {
                v[t + 50 + k] = -75.0 + 5.0 * k as f64 / 60.0;
            }
        }
        v
    }

    #[test]
    fn counts_and_features() {
        let fs = 20000.0;
        let v = train(20000, &[2000, 6000, 12000]);
        let s = detect(&v, fs, 0, v.len(), 3, &ApSettings::default());
        assert_eq!(s.len(), 3);
        let a = &s[0];
        assert_eq!(a.sweep, 3);
        assert!(a.threshold_from_dvdt);
        assert!((a.peak_mv - 30.0).abs() < 1e-9);
        assert!((a.threshold_mv - -50.0).abs() < 1.0, "{}", a.threshold_mv);
        assert!(a.half_width_ms.unwrap() > 0.5 && a.half_width_ms.unwrap() < 3.0);
        assert!((a.ahp_mv.unwrap() - -75.0).abs() < 1e-9);
        assert!((s[1].isi_ms.unwrap() - 200.0).abs() < 1e-9);
        assert!((s[2].isi_ms.unwrap() - 300.0).abs() < 1e-9);
        let ai = adaptation_index(&[200.0, 300.0]).unwrap();
        assert!((ai - 0.2).abs() < 1e-12);
        assert!(adaptation_index(&[1.0]).is_none());
    }

    #[test]
    fn nothing_on_flat_or_short_input() {
        let s = ApSettings::default();
        assert!(detect(&[-70.0; 1000], 20000.0, 0, 1000, 0, &s).is_empty());
        assert!(detect(&[0.0; 2], 20000.0, 0, 2, 0, &s).is_empty());
        assert!(detect(&[f64::NAN; 100], 20000.0, 0, 100, 0, &s).is_empty());
        assert!(detect(&[-70.0; 100], 0.0, 0, 100, 0, &s).is_empty());
    }
}
