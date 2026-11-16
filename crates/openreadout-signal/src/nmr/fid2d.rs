//! Multidimensional FIDs on the 2D surface: [`load_processed_nd`] turns a `ser` into a matrix,
//! and this module hands that matrix to everything that reads stored 2D spectra.
//!
//! - [`fid_matrix`]: the processed FID as a [`StoredMatrix`] (one `PpmAxis` and nucleus per
//!   dimension, slowest first), so `analyze nmr-peaks` picks cross peaks on it as on a stored
//!   spectrum. Pseudo-2D series (DOSY, relaxation, Spinsolve `data.2d`: F1 is the varied
//!   parameter, not a frequency) are refused here: their analysis is 1D peaks per row.
//! - [`ProcessedNdDataset`]: wraps a data set so that its multidimensional FID reads as the
//!   processed spectrum (`--process` on `trace`, `export`, `preview`, `plot`): a Fourier
//!   spectrum as a matrix trace (`extra.shape`, `extra.indirect_axes`, as a Bruker `2rr` reads),
//!   so contours, slices, projections and the matrix export work on it; a series as one 1-D
//!   spectrum per row with `extra.series` (what was varied, its values and unit).
//! - [`NdChoices`]: the per-dimension processing flags (`--lb f1=0.3`, MCP `dimensions`) as
//!   [`NdOptions`]. `--nus` is accepted for the reconstruction choice; the only mode today is
//!   the zero-filled grid `load_processed_nd` builds.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde_json::{Value, json};

use openreadout_core::model::{CheckReport, FileInfo, LsEntry, Table, Trace, TraceInfo};
use openreadout_core::provenance::ProvenanceMap;
use openreadout_core::{Dataset, Error, Plane, PlaneIndex, Result};

use crate::nmr::analysis::is_fid_nd;
use crate::nmr::baseline::BaselineMode;
use crate::nmr::indirect::{load_processed_nd, nd_parameters};
use crate::nmr::process::{Apodization, PhaseMode};
use crate::nmr::processnd::{DimensionOptions, Encoding, NdOptions, NdRecord, SpectrumNd};
use crate::nmr::source2d::{MatrixRequest, StoredMatrix, normalize_nucleus};
use crate::nmr::window::Window;

/// Processing choices for one dimension (all optional: the stored processing otherwise).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DimensionChoice {
    /// Transform size, complex points.
    pub size: Option<usize>,
    /// Window (exponential or Gaussian broadening, or none).
    pub apodization: Option<Apodization>,
    /// Phasing.
    pub phase: Option<PhaseMode>,
    /// Baseline correction.
    pub baseline: Option<BaselineMode>,
}

/// The NUS reconstruction modes accepted by `--nus` (MCP `nus`).
pub const NUS_MODES: &str = "default or zero-fill";

/// Processing choices for a multidimensional FID.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NdChoices {
    /// The direct dimension (the flags without a dimension).
    pub direct: DimensionChoice,
    /// Per dimension, `F1` = 0 (slowest first, the direct dimension last); these override
    /// `direct` for the direct dimension.
    pub by_dimension: BTreeMap<usize, DimensionChoice>,
    /// Ignore the digital-filter group delay.
    pub ignore_group_delay: bool,
    /// Non-uniform sampling: `default` or `zero-fill` (the zero-filled grid; reconstruction
    /// modes plug in here).
    pub nus: Option<String>,
}

impl NdChoices {
    /// Check the choices that do not depend on the data set.
    pub fn check(&self) -> Result<()> {
        match self.nus.as_deref().map(str::to_ascii_lowercase).as_deref() {
            None | Some("default" | "zero-fill" | "zero_fill" | "zerofill") => Ok(()),
            Some(other) => Err(Error::unsupported(
                "nmr-processing",
                format!("NUS reconstruction {other:?}"),
                format!(
                    "The NUS modes available are {NUS_MODES} (the measured rows on the full grid, zeros elsewhere); other reconstructions are not available yet."
                ),
            )),
        }
    }

    /// The [`NdOptions`] for a data set with `nd` dimensions.
    pub fn options(&self, nd: usize) -> NdOptions {
        let apply = |o: &mut DimensionOptions, c: &DimensionChoice| {
            if let Some(n) = c.size {
                o.size = Some(n);
            }
            if let Some(a) = c.apodization {
                o.window = Some(match a {
                    Apodization::None => Window::None,
                    Apodization::Exponential { line_broadening_hz } => {
                        Window::Exponential { line_broadening_hz }
                    }
                    Apodization::Gaussian { line_broadening_hz } => {
                        Window::Gaussian { line_broadening_hz }
                    }
                });
            }
            if let Some(p) = c.phase {
                o.phase = p;
            }
            if let Some(b) = c.baseline {
                o.baseline = b;
            }
        };
        let mut opts = NdOptions {
            ignore_group_delay: self.ignore_group_delay,
            ..NdOptions::default()
        };
        apply(&mut opts.direct, &self.direct);
        let nd = nd.max(1);
        opts.indirect = vec![DimensionOptions::default(); nd - 1];
        for (&d, c) in &self.by_dimension {
            if d + 1 == nd {
                apply(&mut opts.direct, c);
            } else if d + 1 < nd {
                // indirect dimensions are listed fastest first: F(nd-1) is entry 0
                let k = nd - 2 - d;
                apply(&mut opts.indirect[k], c);
            }
        }
        opts
    }
}

/// Is this data set's FID `index` a pseudo-2D series (F1 not transformed)?
pub fn is_series(ds: &dyn Dataset, info: &FileInfo, index: u32) -> bool {
    nd_parameters(ds, info, index).is_ok_and(|p| {
        p.indirect
            .iter()
            .any(|d| d.series.is_some() || d.encoding == Encoding::Series)
    })
}

fn nuclei_of(s: &SpectrumNd) -> Vec<Option<String>> {
    s.record
        .dimensions
        .iter()
        .map(|d| d.nucleus.as_deref().and_then(normalize_nucleus))
        .collect()
}

/// The multidimensional FID `trace` processed with `choices`, as a matrix the 2D picker takes
/// (module docs), with what was done. Refuses pseudo-2D series and matrices larger than
/// `req.max_points`.
pub fn fid_matrix(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    trace: u32,
    req: &MatrixRequest,
    choices: &NdChoices,
) -> Result<(StoredMatrix, NdRecord)> {
    choices.check()?;
    if is_series(ds, info, trace) {
        return Err(Error::unsupported(
            "nmr-processing",
            format!("cross peaks of the pseudo-2D series in trace {trace}"),
            "F1 of this data set is a varied parameter (diffusion gradient, delay), not a frequency: pick 1D peaks of one row with `--sweep N` (MCP sweep), or look at the rows with `preview --process`.",
        ));
    }
    let p = nd_parameters(ds, info, trace)?;
    let opts = choices.options(p.indirect.len() + 1);
    let s = load_processed_nd(ds, info, trace, &opts)?;
    let total = s
        .shape
        .iter()
        .try_fold(1usize, |a, &b| a.checked_mul(b))
        .unwrap_or(usize::MAX);
    if total > req.max_points {
        return Err(crate::nmr::slices2d::too_many(total, req.max_points));
    }
    let dims = s.shape.len();
    let carrier_points = (0..dims)
        .map(|d| (d + 1 < dims).then(|| (s.shape[d] / 2) as f64))
        .collect();
    let nuclei = nuclei_of(&s);
    let record = s.record.clone();
    Ok((
        StoredMatrix {
            data: s.real,
            shape: s.shape.clone(),
            axes: s.axes,
            nuclei,
            full_shape: s.shape,
            offsets: vec![0; dims],
            carrier_points,
            trace,
            notes: record.notes.clone(),
        },
        record,
    ))
}

/// One processed FID of [`ProcessedNdDataset`].
struct Processed {
    spectrum: SpectrumNd,
    info: TraceInfo,
}

/// Wraps a data set so that its multidimensional FIDs read as processed spectra (module docs).
/// Processing happens on the first `info` or read (it takes the whole `ser`), once.
pub struct ProcessedNdDataset {
    inner: Box<dyn Dataset>,
    choices: NdChoices,
    fids: Vec<u32>,
    done: Mutex<BTreeMap<u32, Processed>>,
}

impl std::fmt::Debug for ProcessedNdDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessedNdDataset")
            .field("fids", &self.fids)
            .field("choices", &self.choices)
            .finish_non_exhaustive()
    }
}

/// Does `info` have a multidimensional FID?
pub fn has_fid_nd(info: &FileInfo) -> bool {
    info.traces.iter().any(is_fid_nd)
}

impl ProcessedNdDataset {
    /// Wrap `inner`; an error (exit 6) when it has no multidimensional FID.
    pub fn new(inner: Box<dyn Dataset>, choices: NdChoices) -> Result<Self> {
        choices.check()?;
        let info = inner.info()?;
        let fids: Vec<u32> = info
            .traces
            .iter()
            .filter(|t| is_fid_nd(t))
            .map(|t| t.index)
            .collect();
        if fids.is_empty() {
            return Err(Error::unsupported(
                "nmr-processing",
                "multidimensional processing of a data set without a 2D FID",
                "This data set has no FID with indirect dimensions; 1D FIDs are processed with `--process` alone.",
            ));
        }
        Ok(Self {
            inner,
            choices,
            fids,
            done: Mutex::new(BTreeMap::new()),
        })
    }

    /// Process every multidimensional FID now (needs `&mut self`).
    pub fn process_all(&mut self) -> Result<()> {
        let info = self.inner.info()?;
        for &i in &self.fids.clone() {
            let has = self
                .done
                .lock()
                .map_err(|_| Error::Other("processing state poisoned".into()))?
                .contains_key(&i);
            if has {
                continue;
            }
            let p = nd_parameters(self.inner.as_ref(), &info, i)?;
            let opts = self.choices.options(p.indirect.len() + 1);
            let s = load_processed_nd(self.inner.as_mut(), &info, i, &opts)?;
            let t = info
                .traces
                .iter()
                .find(|t| t.index == i)
                .cloned()
                .unwrap_or_default();
            let r = Processed {
                info: describe(t, &s),
                spectrum: s,
            };
            self.done
                .lock()
                .map_err(|_| Error::Other("processing state poisoned".into()))?
                .insert(i, r);
        }
        Ok(())
    }

    /// What was done to FID `index`, once processed.
    pub fn record(&self, index: u32) -> Option<NdRecord> {
        let done = self.done.lock().ok()?;
        done.get(&index).map(|p| p.spectrum.record.clone())
    }
}

/// The trace a processed FID reads as.
fn describe(mut t: TraceInfo, s: &SpectrumNd) -> TraceInfo {
    let dims = s.shape.len();
    let axis_json = |a: &crate::nmr::PpmAxis, quantity: &str| {
        json!({
            "quantity": quantity,
            "unit": "ppm",
            "first": a.first_ppm,
            "step": a.step_ppm,
            "last": a.last_ppm(),
            "size": a.size,
            "spectrometer_frequency_mhz": a.reference_frequency_mhz,
        })
    };
    let nuclei = nuclei_of(s);
    let file = t.extra.get("file").cloned();
    let direct_nucleus = nuclei.last().cloned().flatten();
    let rows: usize = s.shape[..dims - 1].iter().product();
    t.sample_rate_hz = 0.0;
    t.sample_count = s.shape[dims - 1] as u64;
    t.sweep_count = u32::try_from(rows).unwrap_or(u32::MAX);
    t.start_s = None;
    t.channels = vec![openreadout_core::model::SignalChannelInfo {
        index: 0,
        name: "real".into(),
        unit: None,
        dtype: "float64".into(),
        scale: 1.0,
        offset: 0.0,
        extra: Default::default(),
    }];
    let pulse = t.extra.get("pulse_program").cloned();
    t.extra.clear();
    t.extra.insert("kind".into(), json!("processed_spectrum"));
    t.extra
        .insert("processed_from".into(), file.unwrap_or(json!("ser")));
    if let Some(p) = pulse {
        // kept, so the experiment is named as on the stored spectrum
        t.extra.insert("pulse_program_of_fid".into(), p);
    }
    if let Some(n) = &direct_nucleus {
        t.extra.insert("nucleus".into(), json!(n));
    }
    t.extra.insert(
        "axis".into(),
        axis_json(&s.axes[dims - 1], "chemical_shift"),
    );
    t.extra.insert(
        "processing_nd".into(),
        serde_json::to_value(&s.record).unwrap_or(Value::Null),
    );
    match &s.series {
        Some(series) => {
            // a stack of 1D spectra, one per row
            t.extra.insert(
                "series".into(),
                serde_json::to_value(series).unwrap_or(Value::Null),
            );
        }
        None => {
            t.extra.insert("shape".into(), json!(s.shape));
            // indirect axes fastest first, as the Bruker reader lists them
            let ind: Vec<Value> = (0..dims - 1)
                .rev()
                .map(|d| axis_json(&s.axes[d], "chemical_shift"))
                .collect();
            t.extra.insert("indirect_axes".into(), json!(ind));
            if dims == 2
                && let Some(n) = nuclei.first().cloned().flatten()
            {
                t.extra.insert("sweep_axis".into(), json!({ "nucleus": n }));
            }
        }
    }
    t
}

impl Dataset for ProcessedNdDataset {
    fn info(&self) -> Result<FileInfo> {
        let mut info = self.inner.info()?;
        let done = self
            .done
            .lock()
            .map_err(|_| Error::Other("processing state poisoned".into()))?;
        for t in &mut info.traces {
            if let Some(p) = done.get(&t.index) {
                *t = p.info.clone();
            }
        }
        info.notes.push(
            "multidimensional FIDs are shown as spectra processed by OpenReadout (--process); see traces[].extra.processing_nd".into(),
        );
        Ok(info)
    }
    fn vendor_metadata(&self) -> Result<Value> {
        self.inner.vendor_metadata()
    }
    fn provenance(&self) -> ProvenanceMap {
        self.inner.provenance()
    }
    fn assurance_observations(&self) -> openreadout_core::assurance::Observations {
        self.inner.assurance_observations()
    }
    fn file_assurance(&self) -> Option<openreadout_core::assurance::Assurance> {
        self.inner.file_assurance()
    }
    fn is_strict(&self) -> bool {
        self.inner.is_strict()
    }
    fn entries(&self) -> Result<Vec<LsEntry>> {
        self.inner.entries()
    }
    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        self.inner.read_plane(image, index)
    }
    fn check(&mut self) -> Result<CheckReport> {
        self.inner.check()
    }
    fn check_headers(&mut self) -> Result<CheckReport> {
        self.inner.check_headers()
    }
    fn member_files(&self) -> Vec<std::path::PathBuf> {
        self.inner.member_files()
    }
    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        self.inner.read_table(index, first_row, max_rows)
    }
    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        if !self.fids.contains(&index) {
            return self
                .inner
                .read_trace(index, sweep, first_sample, max_samples);
        }
        self.process_all()?;
        let done = self
            .done
            .lock()
            .map_err(|_| Error::Other("processing state poisoned".into()))?;
        let Some(p) = done.get(&index) else {
            return Err(Error::Other(format!("FID {index} was not processed")));
        };
        let s = &p.spectrum;
        let n = *s.shape.last().unwrap_or(&0);
        let rows = s.real.len() / n.max(1);
        let r = sweep as usize;
        if r >= rows {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range: the processed spectrum has {rows} rows"
            )));
        }
        let row = &s.real[r * n..(r + 1) * n];
        let a = (first_sample as usize).min(n);
        let b = (first_sample.saturating_add(max_samples) as usize).min(n);
        Ok(Trace {
            trace: index,
            sweep,
            first_sample: a as u64,
            channels: vec![row[a..b].to_vec()],
        })
    }
    fn experiment(&self) -> Option<openreadout_core::Experiment> {
        self.inner.experiment()
    }
}

/// Wrap `inner` for `--process` on a multidimensional FID and process it now, so that `info`
/// describes the spectrum.
pub fn process_nd_dataset(inner: Box<dyn Dataset>, choices: NdChoices) -> Result<Box<dyn Dataset>> {
    let mut w = ProcessedNdDataset::new(inner, choices)?;
    w.process_all()?;
    Ok(Box::new(w))
}

/// `--process` (MCP `process`) on any NMR data set: a multidimensional FID is transformed in
/// every dimension with `nd`; otherwise each 1D FID is processed row by row with `one`.
pub fn process_any(
    ds: Box<dyn Dataset>,
    nd: NdChoices,
    one: crate::nmr::ProcessOptions,
) -> Result<Box<dyn Dataset>> {
    if has_fid_nd(&ds.info()?) {
        return process_nd_dataset(ds, nd);
    }
    if !nd.by_dimension.is_empty() {
        return Err(Error::Usage(
            "options for one dimension (fN=) apply to 2D FIDs; this data set has 1D FIDs only"
                .into(),
        ));
    }
    Ok(Box::new(crate::nmr::ProcessedNmrDataset::new(ds, one)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_map_dimensions_to_the_transform_order() {
        let mut c = NdChoices {
            direct: DimensionChoice {
                size: Some(1024),
                ..DimensionChoice::default()
            },
            ..NdChoices::default()
        };
        c.by_dimension.insert(
            0,
            DimensionChoice {
                apodization: Some(Apodization::Exponential {
                    line_broadening_hz: 2.0,
                }),
                ..DimensionChoice::default()
            },
        );
        c.by_dimension.insert(
            2,
            DimensionChoice {
                size: Some(4096),
                ..DimensionChoice::default()
            },
        );
        // 3D: F1 is the slowest, the last of the indirect list; F3 the direct dimension
        let o = c.options(3);
        assert_eq!(o.direct.size, Some(4096));
        assert_eq!(o.indirect.len(), 2);
        assert_eq!(
            o.indirect[1].window,
            Some(Window::Exponential {
                line_broadening_hz: 2.0
            })
        );
        assert_eq!(o.indirect[0], DimensionOptions::default());
        // 2D: F1 is the only indirect dimension
        let o = c.options(2);
        assert!(o.indirect[0].window.is_some());
        assert_eq!(o.direct.size, Some(1024));
        assert!(c.check().is_ok());
        let bad = NdChoices {
            nus: Some("ist".into()),
            ..NdChoices::default()
        };
        assert_eq!(bad.check().unwrap_err().exit_code(), 6);
    }
}
