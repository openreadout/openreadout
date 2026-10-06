//! JSON queries for `analyze nmr-peaks`, `analyze ephys-features` and `analyze spikes`, shared
//! by the MCP tool (`openreadout_nmr_peaks`, `openreadout_ephys_features`, `openreadout_spikes`) and batch
//! tables (`openreadout batch nmr-peaks …`, MCP `openreadout_batch`): the same field names
//! and defaults everywhere.

// `!(x > 0.0)` deliberately rejects NaN.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

use openreadout_core::{Error, Result};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::ephys::{ApSettings, CellRequest, DetectSettings, PeakSign, SpikesRequest};
use crate::nmr::{
    Apodization, BaselineMode, NmrRequest, PeakOptions, PhaseMode, ProcessOptions, SpectrumSource,
};

/// Peak list and integrals of a 1-D NMR spectrum.
#[derive(Debug, Default, Clone, Deserialize, JsonSchema)]
pub struct NmrQuery {
    /// `auto` (default: the vendor's processed spectrum if present, else process the FID),
    /// `fid` (always process the FID), `processed` (stored spectrum only).
    pub from: Option<String>,
    /// Trace index (default: chosen by `from`).
    pub trace: Option<u32>,
    /// Row of a ser/arrayed FID (default 0).
    #[serde(default)]
    pub sweep: u32,
    /// `default` (stored phases, else automatic), `stored`, `auto`, `magnitude`, `none`, or
    /// `"P0,P1"` in degrees.
    pub phase: Option<String>,
    /// Exponential line broadening in Hz (0 = none; default stored, else 0.3 Hz 1H / 1 Hz other).
    pub lb: Option<f64>,
    /// Transform size in complex points (power of two).
    pub size: Option<usize>,
    /// Baseline: `default` (order-1 polynomial), `none`, `poly:N`. Stored spectra are used as
    /// stored unless this is given.
    pub baseline: Option<String>,
    /// Minimum peak height in noise SDs (default 10).
    pub min_snr: Option<f64>,
    /// Minimum prominence in noise SDs (default 5).
    pub min_prominence: Option<f64>,
    /// Minimum height as a fraction of the tallest point (default 0; e.g. 0.01).
    pub min_height_fraction: Option<f64>,
    /// Also report negative peaks (DEPT/APT).
    #[serde(default)]
    pub negative: bool,
    /// Only pick between two shifts, `[a, b]` ppm.
    pub range_ppm: Option<(f64, f64)>,
    /// Keep at most this many peaks (tallest; default 200).
    pub max_peaks: Option<usize>,
    /// Regions to integrate, `[[a, b], …]` in ppm.
    #[serde(default)]
    pub integrate: Vec<(f64, f64)>,
    /// Normalise integrals so region `[index, value]` has that value (default `[0, 1]`).
    pub integral_reference: Option<(usize, f64)>,
}

/// Per-spike rows `ephys-features` returns by default over MCP (`openreadout_ephys_features`): sweeps
/// and cell features answer most questions; spike counts are always complete.
pub const DEFAULT_MAX_SPIKES: usize = 25;

/// Patch-clamp features of a recording.
#[derive(Debug, Default, Clone, Deserialize, JsonSchema)]
pub struct EphysQuery {
    /// Trace index (default 0).
    #[serde(default)]
    pub trace: u32,
    /// Channel (default: the first voltage channel, else the first current channel).
    pub channel: Option<u32>,
    /// Sweeps to analyse (default all).
    pub sweeps: Option<Vec<u32>>,
    /// Spike voltage threshold, mV (default −20).
    pub peak_threshold_mv: Option<f64>,
    /// dV/dt onset criterion, V/s (default 10).
    pub dvdt_threshold: Option<f64>,
    /// Per-spike rows returned (default 25, the first spikes in sweep order; counts stay complete).
    pub max_spikes: Option<usize>,
}

/// Extracellular spike detection.
#[derive(Debug, Default, Clone, Deserialize, JsonSchema)]
pub struct SpikesQuery {
    /// Trace index (default 0; pick the broadband stream).
    #[serde(default)]
    pub trace: u32,
    /// Channels (default all).
    #[serde(default)]
    pub channels: Vec<u32>,
    /// Sweeps/segments (default all).
    pub sweeps: Option<Vec<u32>>,
    /// Band-pass `[low, high]` Hz (default [300, 6000]).
    pub band_hz: Option<(f64, f64)>,
    /// Threshold in MAD noise units (default 5).
    pub threshold: Option<f64>,
    /// `neg` (default), `pos` or `both`.
    pub sign: Option<String>,
    /// Only the first N seconds of each sweep.
    pub max_seconds: Option<f64>,
    /// Spike times per channel returned (default 200).
    pub max_times: Option<usize>,
}

fn usage(msg: String) -> Error {
    Error::Usage(msg)
}

/// Parse a phase mode: `default`, `stored`, `auto`, `magnitude`, `none` or `"P0[,P1]"` degrees.
pub fn phase_mode(s: &str) -> Result<PhaseMode> {
    Ok(match s.trim().to_ascii_lowercase().as_str() {
        "default" => PhaseMode::Default,
        "stored" => PhaseMode::Stored,
        "auto" => PhaseMode::Auto,
        "magnitude" => PhaseMode::Magnitude,
        "none" => PhaseMode::None,
        other => {
            let v: Vec<f64> = other
                .split(',')
                .filter_map(|p| p.trim().parse().ok())
                .collect();
            match v.as_slice() {
                [a] if a.is_finite() => PhaseMode::Manual {
                    phase0_deg: *a,
                    phase1_deg: 0.0,
                },
                [a, b] if a.is_finite() && b.is_finite() => PhaseMode::Manual {
                    phase0_deg: *a,
                    phase1_deg: *b,
                },
                _ => {
                    return Err(usage(format!(
                        "phase {s:?}: default, stored, auto, magnitude, none or \"P0,P1\""
                    )));
                }
            }
        }
    })
}

/// Parse a baseline mode: `default`, `none` or `poly:N` (N ≤ 8).
pub fn baseline_mode(s: &str) -> Result<BaselineMode> {
    let t = s.trim().to_ascii_lowercase();
    match t.as_str() {
        "default" => Ok(BaselineMode::Default),
        "none" => Ok(BaselineMode::None),
        _ => t
            .strip_prefix("poly:")
            .and_then(|n| n.parse::<u32>().ok())
            .filter(|n| *n <= 8)
            .map(|order| BaselineMode::Polynomial { order })
            .ok_or_else(|| usage(format!("baseline {s:?}: default, none or poly:N (N <= 8)"))),
    }
}

impl NmrQuery {
    /// The request this query describes.
    pub fn request(&self) -> Result<NmrRequest> {
        let mut process = ProcessOptions::default();
        if let Some(p) = &self.phase {
            process.phase = phase_mode(p)?;
        }
        if let Some(b) = &self.baseline {
            process.baseline = baseline_mode(b)?;
        }
        process.size = self.size.filter(|n| *n >= 2);
        if let Some(lb) = self.lb {
            if !lb.is_finite() {
                return Err(usage("lb must be finite".into()));
            }
            process.apodization = Some(if lb == 0.0 {
                Apodization::None
            } else {
                Apodization::Exponential {
                    line_broadening_hz: lb,
                }
            });
        }
        let source = match self.from.as_deref().unwrap_or("auto") {
            "auto" => SpectrumSource::Auto,
            "fid" => SpectrumSource::Fid,
            "processed" => SpectrumSource::Processed,
            other => return Err(usage(format!("from {other:?}: auto, fid or processed"))),
        };
        Ok(NmrRequest {
            trace: self.trace,
            sweep: self.sweep,
            source,
            process,
            baseline_on_stored: self.baseline.is_some(),
            peaks: PeakOptions {
                min_snr: self.min_snr.unwrap_or(10.0),
                min_prominence_snr: self.min_prominence.unwrap_or(5.0),
                min_height_fraction: self.min_height_fraction.unwrap_or(0.0),
                include_negative: self.negative,
                range_ppm: self.range_ppm,
                max_peaks: self.max_peaks.unwrap_or(200),
            },
            integrals: self.integrate.clone(),
            integral_reference: self.integral_reference,
        })
    }
}

impl EphysQuery {
    /// The request this query describes.
    pub fn request(&self) -> Result<CellRequest> {
        let mut ap = ApSettings::default();
        if let Some(v) = self.peak_threshold_mv {
            ap.peak_threshold_mv = v;
        }
        if let Some(v) = self.dvdt_threshold {
            ap.dvdt_threshold_v_per_s = v;
        }
        if !ap.peak_threshold_mv.is_finite() || !(ap.dvdt_threshold_v_per_s > 0.0) {
            return Err(usage(
                "peak_threshold_mv must be finite and dvdt_threshold positive".into(),
            ));
        }
        Ok(CellRequest {
            trace: self.trace,
            channel: self.channel,
            sweeps: self.sweeps.clone(),
            ap,
            max_spikes: self.max_spikes.unwrap_or(DEFAULT_MAX_SPIKES),
        })
    }
}

impl SpikesQuery {
    /// The request this query describes.
    pub fn request(&self) -> Result<SpikesRequest> {
        let mut s = DetectSettings::default();
        if let Some((lo, hi)) = self.band_hz {
            if !(lo > 0.0 && hi > lo) {
                return Err(usage(
                    "band_hz must be [low, high] with 0 < low < high".into(),
                ));
            }
            s.low_hz = lo;
            s.high_hz = hi;
        }
        if let Some(t) = self.threshold {
            if !(t > 0.0) {
                return Err(usage("threshold must be positive".into()));
            }
            s.threshold = t;
        }
        s.sign = match self.sign.as_deref().unwrap_or("neg") {
            "neg" => PeakSign::Neg,
            "pos" => PeakSign::Pos,
            "both" => PeakSign::Both,
            other => return Err(usage(format!("sign {other:?}: neg, pos or both"))),
        };
        Ok(SpikesRequest {
            trace: self.trace,
            channels: self.channels.clone(),
            sweeps: self.sweeps.clone(),
            settings: s,
            max_seconds: self.max_seconds.filter(|v| *v > 0.0),
            max_times: self.max_times.unwrap_or(200),
        })
    }
}
