//! NMR: FID processing, peak picking and integration.
//!
//! [`analysis::analyze_peaks`] is what `openreadout analyze nmr-peaks` and the
//! `openreadout_nmr_peaks` MCP tool run: it picks 1D peaks ([`analyze`]: the vendor's
//! processed spectrum, or the FID processed here) or, on a 2D/3D spectrum, cross peaks
//! ([`analyze_2d`]). [`ProcessedNmrDataset`] wraps any NMR dataset so that its FID traces read
//! as processed spectra (`trace --process`, `export --process`, `preview --process`);
//! [`slices2d::NmrMatrixDataset`] adds the rows, columns and projections of a 2D spectrum as
//! traces (`--slice`).

pub mod analysis;
pub mod baseline;
mod dataset;
pub mod peaks;
pub mod peaks2d;
pub mod phase;
pub mod process;
pub mod slices2d;
pub mod source;
pub mod source2d;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use openreadout_core::model::FileInfo;
use openreadout_core::{Dataset, Result};

pub use analysis::{NmrPeaksReport, NmrPeaksRequest, analyze_peaks};
pub use baseline::BaselineMode;
pub use dataset::ProcessedNmrDataset;
pub use peaks::{Integral, Peak, PeakOptions, integrate, noise_sd, pick_peaks};
pub use peaks2d::{
    CrossPeak, CrossPeakOptions, CrossPeakPick, Maximum, PeakFlag, PeakSign, SpectrumContext,
    VolumeIntegral, annotate, integrate_regions, matrix_noise_sd, pick_cross_peaks,
};
pub use process::{
    Apodization, FidParameters, PhaseMode, PpmAxis, ProcessOptions, ProcessingRecord, Spectrum,
    StoredProcessing, process_fid,
};
pub use slices2d::{
    DimensionAxis, NmrMatrixDataset, PooledRequest, PooledView, ProjectionMethod, SliceSpec,
    read_pooled, read_slice,
};
pub use source::SpectrumSource;
pub use source2d::{MatrixRequest, StoredMatrix, load_matrix};

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
    /// Without `integrals`: integrate every multiplet, a region around each group of peaks
    /// closer than [`MULTIPLET_GAP_HZ`].
    pub auto_integrals: bool,
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
    /// The slice of a 2D spectrum the peaks were picked on (`row:PPM`, `column:PPM`,
    /// `skyline:f2`, ...), when one was asked for; `trace` is the 2D spectrum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slice: Option<String>,
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
        let (mut real, axis) = source::load_spectrum_row(ds, info, index, req.sweep)?;
        let mut notes = Vec::new();
        if req.baseline_on_stored
            && let Some(step) = baseline::correct(&mut real, req.process.baseline)
        {
            notes.push(format!("stored spectrum corrected: {step}"));
        }
        if t.sweep_count > 1 {
            notes.push(format!(
                "row {} of {} of the stored spectrum (its direct-dimension spectrum at one indirect point)",
                req.sweep, t.sweep_count
            ));
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

/// Peaks closer than this belong to one multiplet (`auto_integrals`), Hz.
pub const MULTIPLET_GAP_HZ: f64 = 15.0;
/// Margin of a multiplet's integration region beyond its outer peaks, Hz.
pub const MULTIPLET_MARGIN_HZ: f64 = 10.0;

/// One integration region per multiplet: positive peaks closer than [`MULTIPLET_GAP_HZ`] form
/// a group, and its region reaches [`MULTIPLET_MARGIN_HZ`] beyond its outer peaks (at least
/// the peaks' half widths). Regions are in falling ppm, as peaks are listed.
pub fn multiplet_regions(peaks: &[Peak], axis: &PpmAxis) -> Vec<(f64, f64)> {
    let mhz = axis.reference_frequency_mhz;
    if !(mhz.is_finite() && mhz > 0.0) {
        return Vec::new();
    }
    let mut pos: Vec<&Peak> = peaks.iter().filter(|p| p.height > 0.0).collect();
    pos.sort_by(|a, b| b.ppm.total_cmp(&a.ppm));
    let gap = MULTIPLET_GAP_HZ / mhz;
    let margin = |p: &Peak| (MULTIPLET_MARGIN_HZ / mhz).max(p.width_ppm.unwrap_or(0.0));
    let mut out: Vec<(f64, f64)> = Vec::new();
    let mut group: Vec<&Peak> = Vec::new();
    let flush = |g: &mut Vec<&Peak>, out: &mut Vec<(f64, f64)>| {
        if let (Some(hi), Some(lo)) = (g.first(), g.last()) {
            out.push((hi.ppm + margin(hi), lo.ppm - margin(lo)));
        }
        g.clear();
    };
    for p in pos {
        if group.last().is_some_and(|l| l.ppm - p.ppm > gap) {
            flush(&mut group, &mut out);
        }
        group.push(p);
    }
    flush(&mut group, &mut out);
    // regions that overlap after the margins are merged
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for r in out {
        match merged.last_mut() {
            Some(m) if r.0 >= m.1 => m.1 = r.1.min(m.1),
            _ => merged.push(r),
        }
    }
    merged
}

/// Pick peaks and integrate (module docs).
pub fn analyze(ds: &mut dyn Dataset, info: &FileInfo, req: &NmrRequest) -> Result<NmrReport> {
    let chosen = load(ds, info, req)?;
    Ok(report_1d(info, chosen, req))
}

/// Pick peaks and integrate on a spectrum already chosen (or cut from a 2D spectrum).
pub fn report_1d(info: &FileInfo, chosen: ChosenSpectrum, req: &NmrRequest) -> NmrReport {
    let s = &chosen.spectrum;
    let (peaks, sd) = pick_peaks(&s.real, &s.axis, &req.peaks);
    let largest = peaks
        .iter()
        .filter(|p| p.height > 0.0)
        .max_by(|a, b| a.height.total_cmp(&b.height))
        .cloned();
    let regions = if req.integrals.is_empty() && req.auto_integrals {
        multiplet_regions(&peaks, &s.axis)
    } else {
        req.integrals.clone()
    };
    let integrals = integrate(&s.real, &s.axis, &regions, req.integral_reference);
    let trace_name = info
        .traces
        .iter()
        .find(|t| t.index == chosen.trace)
        .and_then(|t| t.name.clone());
    NmrReport {
        path: info.path.clone(),
        format: info.format.id.clone(),
        source: chosen.source.into(),
        trace: chosen.trace,
        trace_name,
        sweep: req.sweep,
        slice: None,
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
    }
}

/// What [`analyze_2d`] should do.
#[derive(Debug, Clone, Default)]
pub struct Nmr2dRequest {
    /// Trace to use (default: the first stored 2D/3D spectrum).
    pub trace: Option<u32>,
    /// Peak-picking thresholds; `ranges_ppm` also limits what is read.
    pub peaks: CrossPeakOptions,
    /// Integration boxes, one ppm pair per dimension (slowest first).
    pub integrals: Vec<Vec<(f64, f64)>>,
    /// Normalisation of the integrals: (box index, value).
    pub integral_reference: Option<(usize, f64)>,
    /// Refuse to read more points than this (default [`source2d::DEFAULT_MAX_POINTS`]).
    pub max_points: Option<usize>,
}

/// Output of 2D/3D peak picking on a stored spectrum.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Nmr2dReport {
    /// The input path.
    pub path: String,
    /// Format id.
    pub format: String,
    /// `processed` (a spectrum the vendor software stored).
    pub source: String,
    /// Trace used (see `info` → `traces[]`).
    pub trace: u32,
    /// Its name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_name: Option<String>,
    /// The pulse program of the data set's acquisition, when the reader reports one
    /// (`hsqcedetgpsisp2.3`, `cosygpppqf`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pulse_program: Option<String>,
    /// Nucleus per dimension, slowest first (F1, F2), when known.
    pub nuclei: Vec<Option<String>>,
    /// What each dimension measures, slowest first: a chemical shift in ppm for most; the F1
    /// axis of a J-resolved spectrum is a coupling (shown in Hz), and a DOSY F1 axis has the
    /// unit the file names. `axes` are in ppm either way.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dimensions: Vec<slices2d::DimensionAxis>,
    /// Points per dimension of the whole spectrum.
    pub shape: Vec<usize>,
    /// Chemical-shift axis per dimension of the whole spectrum, slowest first.
    pub axes: Vec<PpmAxis>,
    /// The two dimensions that observe the same nucleus (diagonal and symmetry annotations).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homonuclear_dimensions: Option<(usize, usize)>,
    /// Noise standard deviation the thresholds and `snr` use (same units as `height`): the
    /// larger of `run_noise_sd` and `maxima_noise_sd`.
    pub noise_sd: f64,
    /// Noise SD from runs along the direct dimension.
    pub run_noise_sd: f64,
    /// Noise SD implied by the heights of the local maxima (heavy-tailed noise).
    pub maxima_noise_sd: f64,
    /// Thresholds used.
    pub peak_options: CrossPeakOptions,
    /// Number of cross peaks found.
    pub peak_count: usize,
    /// Number of maxima in them.
    pub maxima_count: usize,
    /// Maxima left out because they do not stand out from the noise of their row or column.
    pub ridge_suppressed: usize,
    /// Cross peaks left out for a footprint smaller than `min_footprint_points`.
    pub small_footprint_suppressed: usize,
    /// The tallest positive cross peak (the tallest negative one when there is none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_peak: Option<CrossPeak>,
    /// Cross peaks, high direct-dimension shift first; `index` is the apex point in the whole
    /// spectrum.
    pub peaks: Vec<CrossPeak>,
    /// Every maximum (with `list_maxima`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub maxima: Vec<Maximum>,
    /// Volume integrals of the requested boxes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub integrals: Vec<VolumeIntegral>,
    /// Remarks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Pick cross peaks and integrate boxes on a stored 2D/3D spectrum (module docs of
/// [`peaks2d`] and [`source2d`]).
pub fn analyze_2d(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &Nmr2dRequest,
) -> Result<Nmr2dReport> {
    // read the picking ranges and every integration box
    let mut ranges = req.peaks.ranges_ppm.clone();
    if !ranges.is_empty() {
        for b in &req.integrals {
            for (d, &(a, z)) in b.iter().enumerate() {
                if ranges.len() <= d {
                    ranges.resize(d + 1, None);
                }
                if let Some(r) = &mut ranges[d] {
                    let (lo, hi) = (r.0.min(r.1).min(a.min(z)), r.0.max(r.1).max(a.max(z)));
                    *r = (hi, lo);
                }
            }
        }
    }
    let m = load_matrix(
        ds,
        info,
        &MatrixRequest {
            trace: req.trace,
            ranges_ppm: ranges,
            max_points: req.max_points.unwrap_or(source2d::DEFAULT_MAX_POINTS),
            ..MatrixRequest::default()
        },
    )?;
    let vendor = ds.vendor_metadata().unwrap_or(serde_json::Value::Null);
    let dimensions = info
        .traces
        .iter()
        .find(|t| t.index == m.trace)
        .and_then(|t| {
            source2d::matrix_layout(t, &vendor)
                .map(|lay| slices2d::dimension_axes(info, t, &vendor, &lay))
        })
        .unwrap_or_default();
    let mut r = analyze_matrix(info, m, "processed", req)?;
    r.dimensions = dimensions;
    Ok(r)
}

/// Pick cross peaks and integrate boxes on a matrix already read: a stored spectrum
/// ([`load_matrix`]) or one transformed from the FID (`source` names which, as in the report).
pub fn analyze_matrix(
    info: &FileInfo,
    m: StoredMatrix,
    source: &str,
    req: &Nmr2dRequest,
) -> Result<Nmr2dReport> {
    let mut pick = pick_cross_peaks(&m.data, &m.shape, &m.axes, &req.peaks)?;
    let ctx = SpectrumContext {
        nuclei: m.nuclei.clone(),
        carrier_points: m.carrier_points.clone(),
    };
    let pair = annotate(&mut pick.peaks, &m.axes, &ctx, &req.peaks);
    let integrals = integrate_regions(
        &m.data,
        &m.shape,
        &m.axes,
        &req.integrals,
        req.integral_reference,
    )?;
    let shift = |idx: &mut Vec<usize>| {
        for (i, o) in idx.iter_mut().zip(&m.offsets) {
            *i += o;
        }
    };
    for p in &mut pick.peaks {
        shift(&mut p.index);
    }
    for x in &mut pick.maxima {
        shift(&mut x.index);
    }
    let main_peak = pick
        .peaks
        .iter()
        .filter(|p| p.height > 0.0)
        .max_by(|a, b| a.height.total_cmp(&b.height))
        .or_else(|| {
            pick.peaks
                .iter()
                .max_by(|a, b| a.height.abs().total_cmp(&b.height.abs()))
        })
        .cloned();
    let axes = m
        .axes
        .iter()
        .zip(&m.offsets)
        .zip(&m.full_shape)
        .map(|((a, &o), &n)| PpmAxis {
            first_ppm: a.ppm(-(o as f64)),
            size: n,
            ..*a
        })
        .collect();
    let trace_name = info
        .traces
        .iter()
        .find(|t| t.index == m.trace)
        .and_then(|t| t.name.clone());
    let mut notes = m.notes;
    notes.append(&mut pick.notes);
    Ok(Nmr2dReport {
        path: info.path.clone(),
        format: info.format.id.clone(),
        source: source.into(),
        trace: m.trace,
        trace_name,
        pulse_program: slices2d::pulse_program(info),
        nuclei: m.nuclei,
        dimensions: Vec::new(),
        shape: m.full_shape,
        axes,
        homonuclear_dimensions: pair,
        noise_sd: pick.noise_sd,
        run_noise_sd: pick.run_noise_sd,
        maxima_noise_sd: pick.maxima_noise_sd,
        peak_options: req.peaks.clone(),
        peak_count: pick.peaks.len(),
        maxima_count: pick.maxima_count,
        ridge_suppressed: pick.ridge_suppressed,
        small_footprint_suppressed: pick.small_footprint_suppressed,
        main_peak,
        peaks: pick.peaks,
        maxima: pick.maxima,
        integrals,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peak(ppm: f64) -> Peak {
        Peak {
            ppm,
            hz: ppm * 400.0,
            index: 0,
            height: 1.0,
            width_hz: None,
            width_ppm: None,
            snr: None,
        }
    }

    #[test]
    fn multiplets_group_peaks_closer_than_15_hz() {
        let axis = PpmAxis {
            reference_frequency_mhz: 400.0,
            ..PpmAxis::default()
        };
        // a doublet 7 Hz apart at 7.26, a singlet at 2.10
        let p = [peak(7.26), peak(7.26 - 7.0 / 400.0), peak(2.10)];
        let r = multiplet_regions(&p, &axis);
        assert_eq!(r.len(), 2);
        assert!((r[0].0 - (7.26 + 10.0 / 400.0)).abs() < 1e-9);
        assert!((r[1].1 - (2.10 - 10.0 / 400.0)).abs() < 1e-9);
    }
}
