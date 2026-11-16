//! `analyze nmr-peaks` on any NMR data set: [`analyze_peaks`] picks 1D peaks or 2D cross peaks
//! from what the data set holds.
//!
//! - A slice (`row:PPM`, `column:PPM`, a projection) of a 2D spectrum: 1D peaks on that slice.
//! - An explicit sweep: 1D peaks on that row (of a `ser` FID, an arrayed FID or a stored 2D
//!   spectrum).
//! - Otherwise, when the chosen trace is a stored 2D/3D spectrum (or the data set has one and no
//!   trace is named): cross peaks ([`crate::nmr::analyze_2d`]).
//! - A 2D FID with no stored spectrum, or `from: fid` on a 2D data set: the FID is transformed
//!   in every dimension ([`crate::nmr::fid2d::fid_matrix`], with the per-dimension choices of
//!   [`NmrPeaksRequest::nd`]) and its cross peaks picked as on a stored spectrum. A pseudo-2D
//!   series (DOSY, relaxation) gets 1D peaks of one row instead.
//! - Everything else: 1D peaks ([`crate::nmr::analyze`]).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use openreadout_core::model::{FileInfo, TraceInfo};
use openreadout_core::{Dataset, Error, Result};

use crate::nmr::peaks2d::PeakFlag;
use crate::nmr::slices2d::{SliceSpec, read_slice};
use crate::nmr::source::{SpectrumSource, is_fid};
use crate::nmr::source2d::{MatrixRequest, matrix_layout};
use crate::nmr::{
    ChosenSpectrum, Nmr2dReport, Nmr2dRequest, NmrReport, NmrRequest, Spectrum, analyze,
    analyze_2d, analyze_matrix, report_1d,
};

/// Output of `analyze nmr-peaks`: a 1D peak list, or the cross peaks of a 2D/3D spectrum
/// (tell them apart by `axis` against `axes`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)]
pub enum NmrPeaksReport {
    /// Peaks of a 1D spectrum (or of one row, column or projection of a 2D one).
    Spectrum(NmrReport),
    /// Cross peaks of a 2D or 3D spectrum.
    CrossPeaks(Nmr2dReport),
}

/// What [`analyze_peaks`] should do.
#[derive(Debug, Clone, Default)]
pub struct NmrPeaksRequest {
    /// The 1D analysis (also holds the trace, sweep and spectrum source).
    pub spectrum: NmrRequest,
    /// The sweep was asked for (a 1D analysis of that row even on a 2D spectrum).
    pub sweep_given: bool,
    /// Pick 1D peaks on this slice of a 2D spectrum.
    pub slice: Option<SliceSpec>,
    /// The 2D analysis. `peaks.ranges_ppm` holds the per-dimension ranges asked for.
    pub cross_peaks: Nmr2dRequest,
    /// A range given without a dimension: the 1D range, or the direct (last) dimension of a
    /// 2D spectrum.
    pub direct_range: Option<(f64, f64)>,
    /// Negative peaks asked for or refused (`None`: 1D leaves them out, 2D keeps them).
    pub negative: Option<bool>,
    /// How a multidimensional FID is processed (per dimension).
    pub nd: crate::nmr::fid2d::NdChoices,
}

/// Is `t` a multi-dimensional FID (a `ser` with indirect dimensions)?
pub fn is_fid_nd(t: &TraceInfo) -> bool {
    is_fid(t)
        && t.sweep_count > 1
        && t.extra
            .get("indirect_dimensions")
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty())
}

/// Where the 2D path takes its matrix from.
enum Route {
    Spectrum,
    Stored(Option<u32>),
    Fid(u32),
}

fn route(ds: &dyn Dataset, info: &FileInfo, req: &NmrPeaksRequest) -> Route {
    if req.sweep_given {
        return Route::Spectrum;
    }
    let vendor = ds.vendor_metadata().unwrap_or(Value::Null);
    let source = req.spectrum.source;
    if let Some(i) = req.spectrum.trace {
        return match info.traces.iter().find(|t| t.index == i) {
            Some(t) if matrix_layout(t, &vendor).is_some() => Route::Stored(Some(i)),
            Some(t) if is_fid_nd(t) && !crate::nmr::fid2d::is_series(ds, info, i) => Route::Fid(i),
            _ => Route::Spectrum,
        };
    }
    let matrix = info
        .traces
        .iter()
        .find(|t| matrix_layout(t, &vendor).is_some());
    // a pseudo-2D series is analysed row by row
    let fid_nd = info
        .traces
        .iter()
        .find(|t| is_fid_nd(t) && !crate::nmr::fid2d::is_series(ds, info, t.index));
    let spectrum_1d = info
        .traces
        .iter()
        .any(|t| !is_fid(t) && t.sweep_count <= 1 && t.extra.contains_key("axis"));
    match (source, matrix, fid_nd) {
        (SpectrumSource::Fid, _, Some(f)) => Route::Fid(f.index),
        (SpectrumSource::Fid, _, None) => Route::Spectrum,
        (_, Some(_), _) => Route::Stored(None),
        (SpectrumSource::Auto, None, Some(f)) if !spectrum_1d => Route::Fid(f.index),
        _ => Route::Spectrum,
    }
}

/// `analyze nmr-peaks` (module docs).
pub fn analyze_peaks(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &NmrPeaksRequest,
) -> Result<NmrPeaksReport> {
    if let Some(spec) = req.slice {
        if req.sweep_given {
            return Err(Error::Usage(
                "--slice and --sweep both pick the spectrum to analyse; give one".into(),
            ));
        }
        return slice_peaks(ds, info, req, spec).map(NmrPeaksReport::Spectrum);
    }
    match route(ds, info, req) {
        Route::Spectrum => {
            let one = one_d(req)?;
            let mut r = analyze(ds, info, &one)?;
            let rows = info
                .traces
                .iter()
                .find(|t| t.index == r.trace)
                .filter(|t| is_fid_nd(t))
                .map(|t| t.sweep_count);
            if let Some(n) = rows
                && !req.sweep_given
            {
                r.notes.push(format!(
                    "a pseudo-2D series of {n} rows: these are the peaks of row 0; pick another row with --sweep N (MCP sweep)"
                ));
            }
            Ok(NmrPeaksReport::Spectrum(r))
        }
        Route::Stored(trace) => {
            let two = two_d(ds, info, req, trace)?;
            analyze_2d(ds, info, &two).map(NmrPeaksReport::CrossPeaks)
        }
        Route::Fid(trace) => {
            let two = two_d(ds, info, req, Some(trace))?;
            let (m, record) = crate::nmr::fid2d::fid_matrix(
                ds,
                info,
                trace,
                &MatrixRequest {
                    trace: Some(trace),
                    ranges_ppm: two.peaks.ranges_ppm.clone(),
                    max_points: two
                        .max_points
                        .unwrap_or(crate::nmr::source2d::DEFAULT_MAX_POINTS),
                    ..MatrixRequest::default()
                },
                &req.nd,
            )?;
            let vendor = ds.vendor_metadata().unwrap_or(Value::Null);
            let lay = crate::nmr::source2d::MatrixLayout {
                shape: m.shape.clone(),
                axes: m.axes.clone(),
                nuclei: m.nuclei.clone(),
                sweep_stride: 1,
                notes: Vec::new(),
            };
            let t = info
                .traces
                .iter()
                .find(|t| t.index == trace)
                .cloned()
                .unwrap_or_default();
            let dimensions = crate::nmr::slices2d::dimension_axes(info, &t, &vendor, &lay);
            let mut r = analyze_matrix(info, m, "fid", &two)?;
            r.dimensions = dimensions;
            r.processing = Some(record);
            Ok(NmrPeaksReport::CrossPeaks(r))
        }
    }
}

/// The 1D request, refusing the 2D-only forms.
fn one_d(req: &NmrPeaksRequest) -> Result<NmrRequest> {
    if !req.cross_peaks.integrals.is_empty() {
        return Err(Error::Usage(
            "integration boxes with one range per dimension apply to 2D spectra; integrate a 1D spectrum (or a row) with A:B ranges".into(),
        ));
    }
    if req.cross_peaks.peaks.ranges_ppm.iter().any(Option::is_some) {
        return Err(Error::Usage(
            "ranges per dimension (f1=A:B, f2=C:D) apply to 2D spectra; a 1D spectrum (or a row) takes one A:B range".into(),
        ));
    }
    let mut one = req.spectrum.clone();
    one.peaks.range_ppm = req.direct_range;
    one.peaks.include_negative = req.negative.unwrap_or(false);
    Ok(one)
}

/// The 2D request for `trace`: the bare range goes to the direct dimension.
fn two_d(
    ds: &dyn Dataset,
    info: &FileInfo,
    req: &NmrPeaksRequest,
    trace: Option<u32>,
) -> Result<Nmr2dRequest> {
    if !req.spectrum.integrals.is_empty() || req.spectrum.auto_integrals {
        return Err(Error::Usage(
            "this is a 2D spectrum: give integration boxes with one range per dimension (`--integrate f1=A:B,f2=C:D`; MCP integrate [{\"f1\": [a, b], \"f2\": [c, d]}]), or pick 1D peaks of one row with `--sweep N` or `--slice row:PPM`".into(),
        ));
    }
    let mut two = req.cross_peaks.clone();
    two.trace = trace;
    if let Some(n) = req.negative {
        two.peaks.include_negative = n;
    }
    if let Some(r) = req.direct_range {
        let vendor = ds.vendor_metadata().unwrap_or(Value::Null);
        let dims = info
            .traces
            .iter()
            .filter(|t| trace.is_none_or(|i| t.index == i))
            .find_map(|t| matrix_layout(t, &vendor).map(|l| l.shape.len()))
            .unwrap_or(2);
        if two.peaks.ranges_ppm.len() < dims {
            two.peaks.ranges_ppm.resize(dims, None);
        }
        if two.peaks.ranges_ppm[dims - 1].is_some() {
            return Err(Error::Usage(format!(
                "the direct dimension (F{dims}) has two ranges: give it once"
            )));
        }
        two.peaks.ranges_ppm[dims - 1] = Some(r);
    }
    Ok(two)
}

fn slice_peaks(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &NmrPeaksRequest,
    spec: SliceSpec,
) -> Result<NmrReport> {
    let s = read_slice(ds, info, req.spectrum.trace, spec)?;
    let one = one_d(req)?;
    let chosen = ChosenSpectrum {
        spectrum: Spectrum {
            real: s.values,
            imag: Vec::new(),
            axis: s.axis,
        },
        source: "processed",
        trace: s.source_trace,
        processing: None,
        nucleus: s.dimension.nucleus.clone(),
        notes: vec![format!("peaks of the {}", s.name)],
    };
    let mut r = report_1d(info, chosen, &one);
    r.sweep = 0;
    r.slice = Some(spec.text());
    Ok(r)
}

fn flag_name(f: PeakFlag) -> &'static str {
    match f {
        PeakFlag::Diagonal => "diagonal",
        PeakFlag::PartnerMissing => "partner_missing",
        PeakFlag::Ridge => "ridge",
        PeakFlag::Axial => "axial",
    }
}

fn opt(v: Option<f64>) -> Value {
    v.map_or(Value::Null, |x| json!(x))
}

impl Nmr2dReport {
    /// One flat row per cross peak, for tables (`-o peaks.csv`, `batch nmr-peaks`): `peak`
    /// (index in `peaks`), the position per dimension (`f1_ppm`, `f2_ppm`; a J-resolved F1 is
    /// `f1_hz`), `height`, `sign`, `snr`, `local_snr`, the widths per dimension in Hz,
    /// `volume`, `absolute_volume`, `maxima`, `footprint_points`, `flags` (joined with `;`),
    /// `symmetric_partner` and `partner_height_ratio`.
    pub fn cross_peak_rows(&self) -> Vec<Map<String, Value>> {
        let name = |d: usize| -> (String, f64) {
            match self.dimensions.get(d) {
                Some(a) if !a.is_shift() => (format!("f{}_{}", d + 1, a.slug()), a.scale),
                _ => (format!("f{}_ppm", d + 1), 1.0),
            }
        };
        self.peaks
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mut r = Map::new();
                r.insert("peak".into(), json!(i));
                for (d, &v) in p.ppm.iter().enumerate() {
                    let (k, scale) = name(d);
                    r.insert(k, json!(v * scale));
                }
                r.insert("height".into(), json!(p.height));
                r.insert(
                    "sign".into(),
                    serde_json::to_value(p.sign).unwrap_or(Value::Null),
                );
                r.insert("snr".into(), opt(p.snr));
                r.insert("local_snr".into(), opt(p.local_snr));
                for (d, w) in p.width_hz.iter().enumerate() {
                    r.insert(format!("f{}_width_hz", d + 1), opt(*w));
                }
                r.insert("volume".into(), json!(p.volume));
                r.insert("absolute_volume".into(), json!(p.absolute_volume));
                r.insert("maxima".into(), json!(p.maxima));
                r.insert("footprint_points".into(), json!(p.footprint_points));
                r.insert(
                    "flags".into(),
                    json!(
                        p.flags
                            .iter()
                            .map(|f| flag_name(*f))
                            .collect::<Vec<_>>()
                            .join(";")
                    ),
                );
                r.insert(
                    "symmetric_partner".into(),
                    p.symmetric_partner.map_or(Value::Null, |x| json!(x)),
                );
                r.insert("partner_height_ratio".into(), opt(p.partner_height_ratio));
                r
            })
            .collect()
    }

    /// One flat row per integration box: `region`, `f1_from_ppm`, `f1_to_ppm`, ...,
    /// `value`, `normalized`, `points`.
    pub fn integral_rows(&self) -> Vec<Map<String, Value>> {
        self.integrals
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let mut r = Map::new();
                r.insert("region".into(), json!(i));
                for (d, (a, b)) in g.ranges_ppm.iter().enumerate() {
                    r.insert(format!("f{}_from_ppm", d + 1), json!(a));
                    r.insert(format!("f{}_to_ppm", d + 1), json!(b));
                }
                r.insert("value".into(), json!(g.value));
                r.insert("normalized".into(), opt(g.normalized));
                r.insert("points".into(), json!(g.points));
                r
            })
            .collect()
    }
}

impl NmrReport {
    /// One flat row per peak: `peak`, `ppm`, `hz`, `height`, `width_hz`, `width_ppm`, `snr`.
    pub fn peak_rows(&self) -> Vec<Map<String, Value>> {
        self.peaks
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mut r = Map::new();
                r.insert("peak".into(), json!(i));
                if let Ok(Value::Object(m)) = serde_json::to_value(p) {
                    r.extend(m);
                }
                r
            })
            .collect()
    }

    /// One flat row per region: `region`, `from_ppm`, `to_ppm`, `value`, `normalized`, `points`.
    pub fn integral_rows(&self) -> Vec<Map<String, Value>> {
        self.integrals
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let mut r = Map::new();
                r.insert("region".into(), json!(i));
                if let Ok(Value::Object(m)) = serde_json::to_value(g) {
                    r.extend(m);
                }
                r
            })
            .collect()
    }
}

impl NmrPeaksReport {
    /// The peak (or cross-peak) rows, then the integral rows, of either kind of report.
    pub fn rows(&self) -> (Vec<Map<String, Value>>, Vec<Map<String, Value>>) {
        match self {
            NmrPeaksReport::Spectrum(r) => (r.peak_rows(), r.integral_rows()),
            NmrPeaksReport::CrossPeaks(r) => (r.cross_peak_rows(), r.integral_rows()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nmr::peaks2d::{CrossPeak, PeakSign};

    #[test]
    fn cross_peak_rows_name_each_dimension() {
        let r = Nmr2dReport {
            dimensions: vec![
                crate::nmr::DimensionAxis {
                    quantity: "coupling".into(),
                    unit: "Hz".into(),
                    nucleus: Some("1H".into()),
                    scale: 500.0,
                },
                crate::nmr::DimensionAxis {
                    quantity: "chemical_shift".into(),
                    unit: "ppm".into(),
                    nucleus: Some("1H".into()),
                    scale: 1.0,
                },
            ],
            peaks: vec![CrossPeak {
                ppm: vec![0.01, 7.26],
                hz: vec![5.0, 3630.0],
                apex_ppm: vec![0.01, 7.26],
                index: vec![3, 100],
                height: -2.0,
                sign: PeakSign::Negative,
                snr: Some(20.0),
                local_snr: Some(12.0),
                width_hz: vec![Some(2.0), None],
                width_ppm: vec![Some(0.004), None],
                maxima: 1,
                volume: -0.5,
                absolute_volume: 0.5,
                footprint_points: 9,
                footprint_ppm: vec![(0.02, 0.0), (7.27, 7.25)],
                flags: vec![PeakFlag::Diagonal, PeakFlag::Ridge],
                symmetric_partner: None,
                partner_height_ratio: None,
            }],
            ..Nmr2dReport::default()
        };
        let rows = r.cross_peak_rows();
        assert_eq!(rows[0]["f1_hz"], json!(5.0));
        assert_eq!(rows[0]["f2_ppm"], json!(7.26));
        assert_eq!(rows[0]["f2_width_hz"], Value::Null);
        assert_eq!(rows[0]["flags"], json!("diagonal;ridge"));
        assert_eq!(rows[0]["sign"], json!("negative"));
    }
}
