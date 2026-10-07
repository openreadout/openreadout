//! NMR: FID processing, peak picking and integration.
//!
//! [`analyze`] is what `openreadout analyze nmr-peaks` and the `openreadout_nmr_peaks` MCP tool
//! run: choose a spectrum (the vendor's processed one, or the FID processed here), pick peaks,
//! integrate regions. [`ProcessedNmrDataset`] wraps any NMR dataset so that its FID traces read
//! as processed spectra (`trace --process`, `export --process`, `preview --process`).

pub mod baseline;
mod dataset;
pub mod peaks;
pub mod phase;
pub mod process;
pub mod source;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use openreadout_core::model::FileInfo;
use openreadout_core::{Dataset, Result};

pub use baseline::BaselineMode;
pub use dataset::ProcessedNmrDataset;
pub use peaks::{Integral, Peak, PeakOptions, integrate, noise_sd, pick_peaks};
pub use process::{
    Apodization, FidParameters, PhaseMode, PpmAxis, ProcessOptions, ProcessingRecord, Spectrum,
    StoredProcessing, process_fid,
};
pub use source::SpectrumSource;

/// What [`analyze`] should do.
#[derive(Debug, Clone, Default)]
pub struct NmrRequest {
    /// Trace to use (default: chosen by `source`).
    pub trace: Option<u32>,
    /// Sweep (row) of the trace: a `ser` row or an arrayed FID.
    pub sweep: u32,
    /// Stored spectrum or FID.
    pub source: SpectrumSource,
    /// FID processing options (ignored for stored spectra, except `baseline` when
    /// `baseline_on_stored`).
    pub process: ProcessOptions,
    /// Also baseline-correct a stored spectrum (default: use it as stored).
    pub baseline_on_stored: bool,
    /// Peak-picking thresholds.
    pub peaks: PeakOptions,
    /// Integration regions, ppm pairs.
    pub integrals: Vec<(f64, f64)>,
    /// Normalisation of the integrals: (region index, value).
    pub integral_reference: Option<(usize, f64)>,
}

/// Output of `analyze nmr-peaks`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct NmrReport {
    /// The input path.
    pub path: String,
    /// Format id.
    pub format: String,
    /// `processed` (a spectrum the vendor software stored) or `fid` (processed here).
    pub source: String,
    /// Trace used (see `info` → `traces[]`).
    pub trace: u32,
    /// Its name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_name: Option<String>,
    /// Sweep (row) used.
    pub sweep: u32,
    /// Observed nucleus, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nucleus: Option<String>,
    /// How the FID was processed (`source` = `fid` only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub processing: Option<ProcessingRecord>,
    /// The chemical-shift axis of the spectrum.
    pub axis: PpmAxis,
    /// Robust noise standard deviation (same units as `height`).
    pub noise_sd: f64,
    /// Thresholds used.
    pub peak_options: PeakOptions,
    /// Number of peaks found.
    pub peak_count: usize,
    /// The tallest positive peak (named like `analyze peaks` → `main_peak` for chromatograms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_peak: Option<Peak>,
    /// Peaks, high shift first.
    pub peaks: Vec<Peak>,
    /// Integrals of the requested regions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub integrals: Vec<Integral>,
    /// Remarks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// A spectrum ready for analysis, with where it came from.
#[derive(Debug, Clone)]
pub struct ChosenSpectrum {
    /// The spectrum (imaginary part may be empty).
    pub spectrum: Spectrum,
    /// `processed` or `fid`.
    pub source: &'static str,
    /// Trace index.
    pub trace: u32,
    /// Processing record for FIDs.
    pub processing: Option<ProcessingRecord>,
    /// Nucleus if known.
    pub nucleus: Option<String>,
    /// Remarks.
    pub notes: Vec<String>,
}

/// Load (and, for an FID, process) the spectrum `req` asks for.
pub fn load(ds: &mut dyn Dataset, info: &FileInfo, req: &NmrRequest) -> Result<ChosenSpectrum> {
    let index = source::choose_trace(info, req.trace, req.source)?;
    let t = info
        .traces
        .iter()
        .find(|t| t.index == index)
        .cloned()
        .unwrap_or_default();
    let nucleus = t
        .extra
        .get("nucleus")
        .and_then(serde_json::Value::as_str)
        .map(|s| s.trim_start_matches('^').to_string());
    if source::is_fid(&t) {
        let (params, stored, mut notes) = source::fid_parameters(ds, info, index)?;
        let fid = source::read_fid(ds, info, index, req.sweep)?;
        let (spectrum, rec) = process_fid(&fid, &params, &stored, &req.process)?;
        notes.extend(rec.notes.iter().cloned());
        Ok(ChosenSpectrum {
            spectrum,
            source: "fid",
            trace: index,
            processing: Some(rec),
            nucleus,
            notes,
        })
    } else {
        let (mut real, axis) = source::load_spectrum(ds, info, index)?;
        let mut notes = Vec::new();
        if req.baseline_on_stored
            && let Some(step) = baseline::correct(&mut real, req.process.baseline)
        {
            notes.push(format!("stored spectrum corrected: {step}"));
        }
        if req.sweep != 0 {
            notes.push("stored spectra: the first row is used".into());
        }
        Ok(ChosenSpectrum {
            spectrum: Spectrum {
                real,
                imag: Vec::new(),
                axis,
            },
            source: "processed",
            trace: index,
            processing: None,
            nucleus,
            notes,
        })
    }
}

/// Pick peaks and integrate (module docs).
pub fn analyze(ds: &mut dyn Dataset, info: &FileInfo, req: &NmrRequest) -> Result<NmrReport> {
    let chosen = load(ds, info, req)?;
    let s = &chosen.spectrum;
    let (peaks, sd) = pick_peaks(&s.real, &s.axis, &req.peaks);
    let largest = peaks
        .iter()
        .filter(|p| p.height > 0.0)
        .max_by(|a, b| a.height.total_cmp(&b.height))
        .cloned();
    let integrals = integrate(&s.real, &s.axis, &req.integrals, req.integral_reference);
    let trace_name = info
        .traces
        .iter()
        .find(|t| t.index == chosen.trace)
        .and_then(|t| t.name.clone());
    Ok(NmrReport {
        path: info.path.clone(),
        format: info.format.id.clone(),
        source: chosen.source.into(),
        trace: chosen.trace,
        trace_name,
        sweep: req.sweep,
        nucleus: chosen.nucleus,
        processing: chosen.processing,
        axis: s.axis,
        noise_sd: sd,
        peak_options: req.peaks.clone(),
        peak_count: peaks.len(),
        main_peak: largest,
        peaks,
        integrals,
        notes: chosen.notes,
    })
}
