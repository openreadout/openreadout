//! A dataset wrapper that presents FID traces as processed spectra.

use std::collections::HashMap;

use serde_json::{Value, json};

use openreadout_core::model::{
    CheckReport, FileInfo, LsEntry, SignalChannelInfo, Table, Trace, TraceInfo,
};
use openreadout_core::provenance::ProvenanceMap;
use openreadout_core::{Dataset, Error, Plane, PlaneIndex, Result};

use crate::fft::next_pow2;
use crate::nmr::process::{
    FidParameters, MAX_TRANSFORM_SIZE, PhaseMode, ProcessOptions, ProcessingRecord,
    StoredProcessing, ppm_axis, process_fid,
};
use crate::nmr::source;

/// A processed spectrum: real part, imaginary part, and how it was made.
type Processed = (Vec<f64>, Vec<f64>, ProcessingRecord);

struct FidTrace {
    params: FidParameters,
    stored: StoredProcessing,
    size: usize,
}

/// Wraps an NMR dataset: every complex FID trace (Bruker, Varian, JEOL) reads as its processed
/// spectrum (same trace index; one spectrum per sweep; channels `real` and `imag`, or
/// `magnitude`), computed on first read with the given [`ProcessOptions`]. `info` stays
/// header-only: the axis and size come from the parameters. Everything else is delegated.
pub struct ProcessedNmrDataset {
    inner: Box<dyn Dataset>,
    opts: ProcessOptions,
    fids: HashMap<u32, FidTrace>,
    cache: HashMap<(u32, u32), Processed>,
}

impl std::fmt::Debug for ProcessedNmrDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessedNmrDataset")
            .field("opts", &self.opts)
            .field("fid_traces", &self.fids.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl ProcessedNmrDataset {
    /// Wrap `inner`. Errors (exit 6) when it has no FID that can be processed.
    pub fn new(inner: Box<dyn Dataset>, opts: ProcessOptions) -> Result<Self> {
        let info = inner.info()?;
        let mut fids = HashMap::new();
        let mut last_err = None;
        for t in info.traces.iter().filter(|t| source::is_fid(t)) {
            match source::fid_parameters(inner.as_ref(), &info, t.index) {
                Ok((params, stored, _)) => {
                    let want = opts
                        .size
                        .or(stored.size)
                        .unwrap_or_else(|| (t.sample_count as usize).saturating_mul(2));
                    let size = next_pow2(want.max(2))
                        .unwrap_or(MAX_TRANSFORM_SIZE)
                        .min(MAX_TRANSFORM_SIZE);
                    fids.insert(
                        t.index,
                        FidTrace {
                            params,
                            stored,
                            size,
                        },
                    );
                }
                Err(e) => last_err = Some(e),
            }
        }
        if fids.is_empty() {
            return Err(last_err.unwrap_or_else(|| {
                Error::unsupported(
                    "nmr-processing",
                    format!("--process on a {} file (no NMR FID)", info.format.name),
                    "`--process` turns NMR FIDs (Bruker, Varian/Agilent, JEOL) into spectra; drop it for other files.",
                )
            }));
        }
        Ok(Self {
            inner,
            opts,
            fids,
            cache: HashMap::new(),
        })
    }

    /// The processing record of a trace/sweep already read.
    pub fn record(&self, trace: u32, sweep: u32) -> Option<&ProcessingRecord> {
        self.cache.get(&(trace, sweep)).map(|c| &c.2)
    }

    fn magnitude(&self) -> bool {
        self.opts.phase == PhaseMode::Magnitude
    }

    fn describe(&self, t: &mut TraceInfo, f: &FidTrace) {
        let axis = ppm_axis(&f.params, f.size);
        let names: &[&str] = if self.magnitude() {
            &["magnitude"]
        } else {
            &["real", "imag"]
        };
        t.sample_rate_hz = 0.0;
        t.sample_count = f.size as u64;
        t.start_s = None;
        t.channels = names
            .iter()
            .enumerate()
            .map(|(i, n)| SignalChannelInfo {
                index: i as u32,
                name: (*n).into(),
                unit: None,
                dtype: "float64".into(),
                scale: 1.0,
                offset: 0.0,
                extra: Default::default(),
            })
            .collect();
        let nucleus = t.extra.get("nucleus").cloned();
        let file = t.extra.get("file").cloned();
        t.extra.clear();
        t.extra.insert("kind".into(), json!("processed_spectrum"));
        t.extra
            .insert("processed_from".into(), file.unwrap_or(json!("fid")));
        if let Some(n) = nucleus {
            t.extra.insert("nucleus".into(), n);
        }
        t.extra.insert(
            "axis".into(),
            json!({
                "quantity": "chemical_shift",
                "unit": "ppm",
                "first": axis.first_ppm,
                "step": axis.step_ppm,
                "last": axis.last_ppm(),
                "size": axis.size,
                "spectrometer_frequency_mhz": axis.reference_frequency_mhz,
            }),
        );
        t.extra.insert(
            "processing_options".into(),
            serde_json::to_value(&self.opts).unwrap_or(Value::Null),
        );
        t.extra.insert(
            "fid_parameters".into(),
            serde_json::to_value(&f.params).unwrap_or(Value::Null),
        );
        t.extra.insert(
            "stored_processing".into(),
            serde_json::to_value(&f.stored).unwrap_or(Value::Null),
        );
    }
}

impl Dataset for ProcessedNmrDataset {
    fn info(&self) -> Result<FileInfo> {
        let mut info = self.inner.info()?;
        for t in &mut info.traces {
            if let Some(f) = self.fids.get(&t.index) {
                self.describe(t, f);
            }
        }
        info.notes.push(
            "FID traces are shown as spectra processed by OpenReadout (--process); see traces[].extra.processing_options".into(),
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
        if !self.fids.contains_key(&index) {
            return self
                .inner
                .read_trace(index, sweep, first_sample, max_samples);
        }
        if !self.cache.contains_key(&(index, sweep)) {
            let info = self.inner.info()?;
            let fid = source::read_fid(self.inner.as_mut(), &info, index, sweep)?;
            let f = &self.fids[&index];
            let opts = ProcessOptions {
                size: Some(f.size),
                ..self.opts.clone()
            };
            let (spec, rec) = process_fid(&fid, &f.params, &f.stored, &opts)?;
            self.cache
                .insert((index, sweep), (spec.real, spec.imag, rec));
        }
        let (re, im, _) = &self.cache[&(index, sweep)];
        let n = re.len() as u64;
        let a = first_sample.min(n) as usize;
        let b = first_sample.saturating_add(max_samples).min(n) as usize;
        let mut channels = vec![re[a..b].to_vec()];
        if !self.magnitude() {
            channels.push(if im.len() == re.len() {
                im[a..b].to_vec()
            } else {
                vec![0.0; b - a]
            });
        }
        Ok(Trace {
            trace: index,
            sweep,
            first_sample: a as u64,
            channels,
        })
    }
    fn experiment(&self) -> Option<openreadout_core::Experiment> {
        self.inner.experiment()
    }
}
