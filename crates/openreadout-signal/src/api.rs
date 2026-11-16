//! JSON queries for `analyze nmr-peaks`, `analyze ephys-features` and `analyze spikes`, shared
//! by the MCP tool (`openreadout_nmr_peaks`, `openreadout_ephys_features`, `openreadout_spikes`) and batch
//! tables (`openreadout batch nmr-peaks …`, MCP `openreadout_batch`): the same field names
//! and defaults everywhere.

// `!(x > 0.0)` deliberately rejects NaN.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

use std::collections::BTreeMap;

use openreadout_core::{Error, Result};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::ephys::{ApSettings, CellRequest, DetectSettings, PeakSign, SpikesRequest};
use crate::nmr::{
    Apodization, BaselineMode, CrossPeakOptions, DimensionChoice, NdChoices, Nmr2dRequest,
    NmrPeaksReport, NmrPeaksRequest, NmrRequest, PeakOptions, PhaseMode, ProcessOptions,
    SpectrumSource,
};

/// A shift range: `[a, b]` ppm (a 1D spectrum, or the direct dimension of a 2D one), or one
/// range per dimension `{"f1": [a, b], "f2": [c, d]}`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum RangeArg {
    /// `[a, b]`.
    Pair((f64, f64)),
    /// `{"f1": [a, b], "f2": [c, d]}` (F1 is the slowest dimension).
    PerDimension(BTreeMap<String, (f64, f64)>),
}

/// An integration region: `[a, b]` ppm (1D), or a 2D box `{"f1": [a, b], "f2": [c, d]}` or
/// `[[a, b], [c, d]]` (one pair per dimension, F1 first).
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum IntegrateArg {
    /// `[a, b]`.
    Pair((f64, f64)),
    /// `[[a, b], [c, d]]`.
    Box(Vec<(f64, f64)>),
    /// `{"f1": [a, b], "f2": [c, d]}`.
    PerDimension(BTreeMap<String, (f64, f64)>),
}

/// Processing of one dimension of a multidimensional FID (each field as the top-level option of
/// the same name, which applies to the direct dimension).
#[derive(Debug, Default, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DimensionQuery {
    /// `default`, `stored`, `auto`, `magnitude`, `none` or `"P0,P1"` degrees.
    pub phase: Option<String>,
    /// Exponential line broadening, Hz (0 = no window).
    pub lb: Option<f64>,
    /// Gaussian broadening, Hz.
    pub gb: Option<f64>,
    /// Transform size, complex points.
    pub size: Option<usize>,
    /// `default`, `none` or `poly:N`.
    pub baseline: Option<String>,
}

/// NMR peaks: a 1D peak list and integrals, or the cross peaks of a 2D/3D spectrum (picked
/// automatically when the spectrum is one).
#[derive(Debug, Default, Clone, Deserialize, JsonSchema)]
pub struct NmrQuery {
    /// `auto` (default: the vendor's processed spectrum if present, else process the FID),
    /// `fid` (always process the FID), `processed` (stored spectrum only).
    pub from: Option<String>,
    /// Trace index (default: chosen by `from`).
    pub trace: Option<u32>,
    /// Row of a ser/arrayed FID or of a stored 2D spectrum: 1D peaks of that row.
    pub sweep: Option<u32>,
    /// 2D spectra: 1D peaks of a slice instead: `row:PPM` (the F2 trace at an F1 shift),
    /// `column:PPM`, or a projection `skyline:f1|f2`, `min:f1|f2`, `sum:f1|f2`.
    pub slice: Option<String>,
    /// `default` (stored phases, else automatic), `stored`, `auto`, `magnitude`, `none`, or
    /// `"P0,P1"` in degrees.
    pub phase: Option<String>,
    /// Exponential line broadening in Hz (0 = none; default stored, else 0.3 Hz 1H / 1 Hz other).
    pub lb: Option<f64>,
    /// Gaussian window instead of lb: Gaussian line width, Hz.
    pub gb: Option<f64>,
    /// Transform size in complex points (power of two).
    pub size: Option<usize>,
    /// Baseline: `default` (order-1 polynomial), `none`, `poly:N`. Stored spectra are used as
    /// stored unless this is given.
    pub baseline: Option<String>,
    /// Keep the digital-filter group delay (do not remove it).
    #[serde(default)]
    pub no_group_delay: bool,
    /// 2D/3D FIDs: processing per dimension, `{"f1": {"lb": 2, "size": 1024}}` (phase, lb, gb,
    /// size, baseline); the top-level options are the direct dimension's.
    #[serde(default)]
    pub dimensions: BTreeMap<String, DimensionQuery>,
    /// 2D FIDs sampled non-uniformly: `default` or `zero-fill` (the measured rows on the full
    /// grid).
    pub nus: Option<String>,
    /// Minimum peak height in noise SDs (default 10).
    pub min_snr: Option<f64>,
    /// Minimum prominence in noise SDs (default 5).
    pub min_prominence: Option<f64>,
    /// Minimum height as a fraction of the tallest point (default 0; e.g. 0.01).
    pub min_height_fraction: Option<f64>,
    /// Report negative peaks (default: false in 1D; true in 2D, for edited HSQC and
    /// phase-sensitive spectra).
    pub negative: Option<bool>,
    /// Only pick inside this range: `[a, b]` ppm, or per dimension `{"f1": [a, b], "f2": [c, d]}`
    /// (2D: also limits what is read; `[a, b]` alone is F2).
    pub range_ppm: Option<RangeArg>,
    /// Keep at most this many peaks (tallest; default 200).
    pub max_peaks: Option<usize>,
    /// Regions to integrate: `[[a, b], …]` ppm; 2D boxes `[{"f1": [a, b], "f2": [c, d]}, …]`.
    #[serde(default)]
    pub integrate: Vec<IntegrateArg>,
    /// 1D, without integrate: integrate every multiplet (peaks closer than 15 Hz, ±10 Hz).
    #[serde(default)]
    pub auto_integrate: bool,
    /// Normalise integrals so region `[index, value]` has that value (default `[0, 1]`).
    pub integral_reference: Option<(usize, f64)>,
    /// 2D: minimum height in units of the local (row and column) noise (default 6).
    pub min_local_snr: Option<f64>,
    /// 2D: smallest footprint in points; smaller cross peaks are spikes (default 3).
    pub min_footprint_points: Option<usize>,
    /// 2D: maxima closer than this in every dimension form one cross peak, Hz (default 15).
    pub group_gap_hz: Option<f64>,
    /// 2D: ... or closer than this many points, when that is more (default 1.5).
    pub group_gap_points: Option<f64>,
    /// 2D: footprint floor in local-noise units (default 3).
    pub footprint_snr: Option<f64>,
    /// 2D: a row or column whose noise is at least this many noise SDs is a ridge (default 3).
    pub ridge_factor: Option<f64>,
    /// 2D: flag cross peaks on the F1 line of one this many times taller (default 20).
    pub ridge_ratio: Option<f64>,
    /// 2D: keep maxima that stand out from the noise but not from their ridge (flagged).
    #[serde(default)]
    pub keep_ridge_peaks: bool,
    /// 2D homonuclear: diagonal width, Hz (default 15).
    pub diagonal_tolerance_hz: Option<f64>,
    /// 2D homonuclear: largest distance of a symmetric partner from the mirror image, Hz
    /// (default 25).
    pub symmetry_tolerance_hz: Option<f64>,
    /// 2D: also list every maximum of every cross peak.
    #[serde(default)]
    pub list_maxima: bool,
    /// 2D: refuse to read more points than this (default 2^28).
    pub max_points: Option<usize>,
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
    /// A spike's peak must rise this far above the lowest voltage since the previous spike, mV
    /// (default 15; 0 counts every crossing, as eFEL does).
    pub min_rise_mv: Option<f64>,
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

/// Dimension index of `f1`, `f2`, ... (F1 is the slowest dimension).
fn dimension(key: &str) -> Result<usize> {
    let k = key.trim().to_ascii_lowercase();
    k.strip_prefix('f')
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|n| (1..=crate::nmr::peaks2d::MAX_DIMENSIONS).contains(n))
        .map(|n| n - 1)
        .ok_or_else(|| usage(format!("dimension {key:?}: use f1, f2 (f3, f4 for nD)")))
}

fn finite_pair(p: (f64, f64), what: &str) -> Result<(f64, f64)> {
    if p.0.is_finite() && p.1.is_finite() {
        Ok(p)
    } else {
        Err(usage(format!(
            "{what}: a range is two finite numbers in ppm"
        )))
    }
}

fn per_dimension(m: &BTreeMap<String, (f64, f64)>, what: &str) -> Result<Vec<Option<(f64, f64)>>> {
    let mut out: Vec<Option<(f64, f64)>> = Vec::new();
    for (k, &p) in m {
        let d = dimension(k)?;
        if out.len() <= d {
            out.resize(d + 1, None);
        }
        out[d] = Some(finite_pair(p, what)?);
    }
    Ok(out)
}

impl NmrQuery {
    /// The 1D request this query describes (the 2D options are left out).
    pub fn request(&self) -> Result<NmrRequest> {
        Ok(self.peaks_request()?.spectrum)
    }

    /// The request of `analyze nmr-peaks` this query describes, 1D and 2D.
    #[allow(clippy::too_many_lines)]
    pub fn peaks_request(&self) -> Result<NmrPeaksRequest> {
        let mut process = ProcessOptions::default();
        if let Some(p) = &self.phase {
            process.phase = phase_mode(p)?;
        }
        if let Some(b) = &self.baseline {
            process.baseline = baseline_mode(b)?;
        }
        if let Some(n) = self.size {
            if n < 2 {
                return Err(usage("size must be at least 2".into()));
            }
            process.size = Some(n);
        }
        for (name, v) in [("lb", self.lb), ("gb", self.gb)] {
            if v.is_some_and(|v| !v.is_finite()) {
                return Err(usage(format!("{name} must be a finite number of Hz")));
            }
        }
        process.apodization = match (self.lb, self.gb) {
            (Some(_), Some(_)) => {
                return Err(usage("lb and gb are two windows: give one".into()));
            }
            (_, Some(gb)) => Some(Apodization::Gaussian {
                line_broadening_hz: gb,
            }),
            (Some(0.0), None) => Some(Apodization::None),
            (Some(lb), None) => Some(Apodization::Exponential {
                line_broadening_hz: lb,
            }),
            (None, None) => None,
        };
        process.ignore_group_delay = self.no_group_delay;
        let source = match self.from.as_deref().unwrap_or("auto") {
            "auto" => SpectrumSource::Auto,
            "fid" => SpectrumSource::Fid,
            "processed" => SpectrumSource::Processed,
            other => return Err(usage(format!("from {other:?}: auto, fid or processed"))),
        };
        for (name, v) in [
            ("min_snr", self.min_snr),
            ("min_prominence", self.min_prominence),
            ("min_height_fraction", self.min_height_fraction),
            ("min_local_snr", self.min_local_snr),
            ("group_gap_hz", self.group_gap_hz),
            ("group_gap_points", self.group_gap_points),
            ("footprint_snr", self.footprint_snr),
            ("ridge_factor", self.ridge_factor),
            ("ridge_ratio", self.ridge_ratio),
            ("diagonal_tolerance_hz", self.diagonal_tolerance_hz),
            ("symmetry_tolerance_hz", self.symmetry_tolerance_hz),
        ] {
            if let Some(x) = v
                && !(x.is_finite() && x >= 0.0)
            {
                return Err(usage(format!("{name} must be a number >= 0")));
            }
        }
        let (direct_range, ranges) = match &self.range_ppm {
            None => (None, Vec::new()),
            Some(RangeArg::Pair(p)) => (Some(finite_pair(*p, "range_ppm")?), Vec::new()),
            Some(RangeArg::PerDimension(m)) => (None, per_dimension(m, "range_ppm")?),
        };
        let mut regions = Vec::new();
        let mut boxes = Vec::new();
        for g in &self.integrate {
            match g {
                IntegrateArg::Pair(p) => regions.push(finite_pair(*p, "integrate")?),
                IntegrateArg::Box(v) => boxes.push(
                    v.iter()
                        .map(|p| finite_pair(*p, "integrate"))
                        .collect::<Result<Vec<_>>>()?,
                ),
                IntegrateArg::PerDimension(m) => {
                    let v = per_dimension(m, "integrate")?;
                    if v.iter().any(Option::is_none) {
                        return Err(usage(
                            "integrate: a box needs a range for every dimension (f1 and f2)".into(),
                        ));
                    }
                    boxes.push(v.into_iter().flatten().collect());
                }
            }
        }
        if !regions.is_empty() && !boxes.is_empty() {
            return Err(usage(
                "integrate: give 1D regions [a, b] or 2D boxes, not both".into(),
            ));
        }
        let n_regions = regions.len().max(boxes.len());
        if let Some((i, v)) = self.integral_reference
            && (i >= n_regions || !v.is_finite())
        {
            return Err(usage(format!(
                "integral_reference [{i}, {v}]: region {i} does not exist or the value is not finite"
            )));
        }
        let slice = self
            .slice
            .as_deref()
            .map(crate::nmr::SliceSpec::parse)
            .transpose()?;
        if self.max_points == Some(0) {
            return Err(usage("max_points must be at least 1".into()));
        }
        let max_peaks = self.max_peaks.unwrap_or(200);
        let d = CrossPeakOptions::default();
        let cross = CrossPeakOptions {
            min_snr: self.min_snr.unwrap_or(d.min_snr),
            min_local_snr: self.min_local_snr.unwrap_or(d.min_local_snr),
            min_height_fraction: self.min_height_fraction.unwrap_or(d.min_height_fraction),
            min_prominence_snr: self.min_prominence.unwrap_or(d.min_prominence_snr),
            include_negative: self.negative.unwrap_or(d.include_negative),
            ranges_ppm: ranges,
            min_footprint_points: self.min_footprint_points.unwrap_or(d.min_footprint_points),
            max_peaks,
            group_gap_hz: self.group_gap_hz.unwrap_or(d.group_gap_hz),
            group_gap_points: self.group_gap_points.unwrap_or(d.group_gap_points),
            footprint_snr: self.footprint_snr.unwrap_or(d.footprint_snr),
            ridge_factor: self.ridge_factor.unwrap_or(d.ridge_factor),
            ridge_ratio: self.ridge_ratio.unwrap_or(d.ridge_ratio),
            keep_ridge_peaks: self.keep_ridge_peaks,
            diagonal_tolerance_hz: self
                .diagonal_tolerance_hz
                .unwrap_or(d.diagonal_tolerance_hz),
            symmetry_tolerance_hz: self
                .symmetry_tolerance_hz
                .unwrap_or(d.symmetry_tolerance_hz),
            list_maxima: self.list_maxima,
        };
        let choice = |phase: &Option<String>,
                      lb: Option<f64>,
                      gb: Option<f64>,
                      size: Option<usize>,
                      baseline: &Option<String>|
         -> Result<DimensionChoice> {
            if let Some(n) = size
                && n < 2
            {
                return Err(usage("size must be at least 2".into()));
            }
            Ok(DimensionChoice {
                size,
                apodization: match (lb, gb) {
                    (Some(_), Some(_)) => {
                        return Err(usage("lb and gb are two windows: give one".into()));
                    }
                    (_, Some(g)) if g.is_finite() => Some(Apodization::Gaussian {
                        line_broadening_hz: g,
                    }),
                    (Some(0.0), None) => Some(Apodization::None),
                    (Some(l), None) if l.is_finite() => Some(Apodization::Exponential {
                        line_broadening_hz: l,
                    }),
                    (None, None) => None,
                    _ => return Err(usage("lb and gb must be finite numbers of Hz".into())),
                },
                phase: phase.as_deref().map(phase_mode).transpose()?,
                baseline: baseline.as_deref().map(baseline_mode).transpose()?,
            })
        };
        let mut nd = NdChoices {
            direct: choice(&self.phase, self.lb, self.gb, self.size, &self.baseline)?,
            ignore_group_delay: self.no_group_delay,
            nus: self.nus.clone(),
            ..NdChoices::default()
        };
        for (k, d) in &self.dimensions {
            nd.by_dimension.insert(
                dimension(k)?,
                choice(&d.phase, d.lb, d.gb, d.size, &d.baseline)?,
            );
        }
        nd.check()?;
        Ok(NmrPeaksRequest {
            spectrum: NmrRequest {
                trace: self.trace,
                sweep: self.sweep.unwrap_or(0),
                source,
                process,
                baseline_on_stored: self.baseline.is_some(),
                peaks: PeakOptions {
                    min_snr: self.min_snr.unwrap_or(10.0),
                    min_prominence_snr: self.min_prominence.unwrap_or(5.0),
                    min_height_fraction: self.min_height_fraction.unwrap_or(0.0),
                    include_negative: self.negative.unwrap_or(false),
                    range_ppm: direct_range,
                    max_peaks,
                },
                integrals: regions,
                auto_integrals: self.auto_integrate,
                integral_reference: self.integral_reference,
            },
            sweep_given: self.sweep.is_some(),
            slice,
            cross_peaks: Nmr2dRequest {
                trace: self.trace,
                peaks: cross,
                integrals: boxes,
                integral_reference: self.integral_reference,
                max_points: self.max_points,
            },
            direct_range,
            negative: self.negative,
            nd,
        })
    }

    /// Run `analyze nmr-peaks` on `ds` (1D or 2D, from the data).
    pub fn run(
        &self,
        ds: &mut dyn openreadout_core::Dataset,
        info: &openreadout_core::model::FileInfo,
    ) -> Result<NmrPeaksReport> {
        crate::nmr::analyze_peaks(ds, info, &self.peaks_request()?)
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
        if let Some(v) = self.min_rise_mv {
            ap.min_rise_mv = v;
        }
        if !ap.peak_threshold_mv.is_finite()
            || !(ap.dvdt_threshold_v_per_s > 0.0)
            || ap.min_rise_mv.is_nan()
            || ap.min_rise_mv < 0.0
        {
            return Err(usage(
                "peak_threshold_mv must be finite, dvdt_threshold positive and min_rise_mv at least 0"
                    .into(),
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
