//! Extracellular threshold-crossing spike detection.
//!
//! Per channel: band-pass the signal ([`crate::ephys::filter`], Butterworth, default 300–6000 Hz,
//! order 5, zero phase), estimate the noise as `median(|x|) / 0.6745` of the filtered signal
//! (Quiroga, Nadasdy & Ben-Shaul 2004), and report a spike at every sample that
//!
//! - exceeds `threshold × noise` in the chosen direction (`neg`: below −threshold·noise, the
//!   default; `pos`; `both`: either, by absolute value), and
//! - is the extreme (strictly more extreme than every sample within `exclude_ms` before it, and
//!   at least as extreme as every sample within `exclude_ms` after it).
//!
//! This is the local-extremum rule SpikeInterface documents for `detect_peaks` with
//! `method="by_channel"` (defaults `detect_threshold=5`, `peak_sign="neg"`,
//! `exclude_sweep_ms=0.1`); results are compared with it in the corpus tests.

use serde::{Deserialize, Serialize};

use crate::ephys::filter::{bandpass_sos, filtfilt};

/// Which deflections count.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PeakSign {
    /// Negative-going (the usual extracellular spike).
    #[default]
    Neg,
    /// Positive-going.
    Pos,
    /// Either.
    Both,
}

/// Detection settings.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DetectSettings {
    /// Band-pass low edge, Hz.
    pub low_hz: f64,
    /// Band-pass high edge, Hz (clamped to 0.45 × the sampling rate).
    pub high_hz: f64,
    /// Butterworth order.
    pub order: usize,
    /// Threshold in noise units.
    pub threshold: f64,
    /// Direction.
    pub sign: PeakSign,
    /// Exclusion half-window, ms.
    pub exclude_ms: f64,
}

impl Default for DetectSettings {
    fn default() -> Self {
        Self {
            low_hz: 300.0,
            high_hz: 6000.0,
            order: 5,
            threshold: 5.0,
            sign: PeakSign::Neg,
            exclude_ms: 0.1,
        }
    }
}

/// `median(|x|) / 0.6745`.
pub fn mad_noise(x: &[f64]) -> f64 {
    let mut a: Vec<f64> = x
        .iter()
        .filter(|v| v.is_finite())
        .map(|v| v.abs())
        .collect();
    if a.is_empty() {
        return 0.0;
    }
    let m = a.len() / 2;
    a.select_nth_unstable_by(m, f64::total_cmp);
    a[m] / 0.6745
}

/// Filter `x` (sampled at `fs`) per `s`; `None` when the band does not fit the sampling rate.
pub fn bandpass(x: &[f64], fs: f64, s: &DetectSettings) -> Option<Vec<f64>> {
    let high = s.high_hz.min(0.45 * fs);
    let sos = bandpass_sos(s.order, s.low_hz, high, fs)?;
    Some(filtfilt(&sos, x))
}

/// Sample indices of detected spikes in the filtered signal `y` with noise `noise`.
pub fn detect_peaks(y: &[f64], fs: f64, noise: f64, s: &DetectSettings) -> Vec<usize> {
    let n = y.len();
    let w = ((s.exclude_ms / 1000.0) * fs).round().max(0.0) as usize;
    let thr = s.threshold * noise;
    if !(thr > 0.0) || n == 0 {
        return Vec::new();
    }
    let score = |v: f64| match s.sign {
        PeakSign::Neg => -v,
        PeakSign::Pos => v,
        PeakSign::Both => v.abs(),
    };
    let mut out = Vec::new();
    for i in 0..n {
        let v = score(y[i]);
        if !(v > thr) {
            continue;
        }
        let lo = i.saturating_sub(w);
        let hi = (i + w).min(n - 1);
        let before = y[lo..i].iter().all(|&u| score(u) < v);
        let after = y[i + 1..=hi].iter().all(|&u| score(u) <= v);
        if before && after {
            out.push(i);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_injected_spikes() {
        let fs = 30000.0;
        let n = 60000;
        let mut x: Vec<f64> = (0..n)
            .map(|i| 20.0 * crate::nmr::baseline::tests::noise(i) + 50.0)
            .collect();
        let at = [3000usize, 10000, 20000, 20100, 45000];
        for &t in &at {
            for k in 0..30 {
                let ph = k as f64 / 30.0 * std::f64::consts::PI;
                x[t + k] -= 100.0 * ph.sin();
            }
        }
        let s = DetectSettings {
            exclude_ms: 1.0,
            ..DetectSettings::default()
        };
        let y = bandpass(&x, fs, &s).unwrap();
        let noise = mad_noise(&y);
        let p = detect_peaks(&y, fs, noise, &s);
        assert_eq!(p.len(), at.len(), "{p:?}");
        for (a, b) in p.iter().zip(&at) {
            assert!((*a as i64 - (*b as i64 + 15)).abs() < 10, "{a} vs {b}");
        }
        assert!(detect_peaks(&y, fs, 0.0, &s).is_empty());
        assert!(mad_noise(&[]).abs() < f64::EPSILON);
    }
}
