//! EEGLAB datasets (`.set`, with the samples inside it or in a `.fdt` next to it): a MATLAB
//! MAT-file holding the `EEG` structure, either as one variable `EEG` or as one variable per
//! field. One trace whose channels are `EEG.chanlocs` and whose sweeps are the epochs; the
//! events form the `events` table. Notes and vocabulary: `docs/formats/eeglab.md`; provenance:
//! `docs/provenance/eeglab.md`.

pub mod mat;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::bytes::read_block;
use openreadout_core::model::{
    CheckReport, ColumnInfo, DetectConfidence, FileInfo, Finding, FormatDescriptor, LsEntry,
    Severity, SignalChannelInfo, Table, TableInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, Detection, FormatReader, PlaneIndex, has_extension};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use mat::{MatFile, MatValue, MatVariable};

/// Format id of EEGLAB datasets.
pub const EEGLAB_FORMAT_ID: &str = "eeglab";
/// Samples per channel decoded per `read_trace` call at most.
pub const MAX_EEGLAB_READ: u64 = 1 << 24;
/// Bytes read per `read_trace` call at most (one sample frame is always read).
pub const MAX_EEGLAB_READ_BYTES: u64 = 64 << 20;
/// Samples inside a compressed `.set` are inflated into memory once, up to this many bytes.
pub const MAX_EEGLAB_INFLATED: u64 = 2 << 30;

/// Where the samples are.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum EeglabData {
    /// A `.fdt` file of float32 samples, channels interleaved, epochs one after another.
    Fdt(PathBuf),
    /// The `data` array inside the `.set`: variable `variable`, stored as `stored` at `at` in
    /// its stream (values read at parse time are in `values`).
    Embedded {
        /// Index of the MAT variable holding the array.
        variable: usize,
        /// How the values are stored.
        stored: mat::MiType,
        /// Offset of the stored values in the variable's stream.
        at: u64,
        /// The values, when the array was small enough to read at parse time.
        values: Option<Vec<f64>>,
        /// The array class (`double`, `single`, …).
        class: String,
    },
}

/// One event of `EEG.event`.
#[derive(Debug, Clone, PartialEq)]
pub struct EeglabEvent {
    /// `type`, as text (numbers as EEGLAB shows them).
    pub kind: String,
    /// `latency` in samples counting from 1 (fractional after resampling).
    pub latency: f64,
    /// `duration` in samples (0 when absent, NaN when empty).
    pub duration: f64,
    /// `epoch` the event belongs to, counting from 1 (epoched data; 0 otherwise).
    pub epoch: f64,
}

/// The parsed dataset.
#[derive(Debug, Clone)]
pub struct EeglabSet {
    /// The MAT-file.
    pub mat: MatFile,
    /// The `EEG` fields came from one variable `EEG` (rather than one variable per field).
    pub single_variable: bool,
    /// `setname`.
    pub setname: String,
    /// `nbchan`.
    pub channels: u64,
    /// `pnts`: samples per epoch.
    pub points: u64,
    /// `trials`: epochs (1 for continuous data).
    pub trials: u64,
    /// `srate`, Hz.
    pub srate: f64,
    /// `xmin`: time of an epoch's first sample, seconds.
    pub xmin: f64,
    /// Channel labels (`chanlocs.labels`), empty when there are no channel locations.
    pub labels: Vec<String>,
    /// Channel types (`chanlocs.type`).
    pub types: Vec<String>,
    /// `ref`.
    pub reference: String,
    /// Where the samples are.
    pub data: EeglabData,
    /// Events.
    pub events: Vec<EeglabEvent>,
    /// ICA components (rows of `icaweights`; 0 when none).
    pub ica_components: u64,
    /// `etc.eeglabvers`, the EEGLAB version that saved the file, when recorded.
    pub eeglab_version: Option<String>,
    /// Problems found while parsing.
    pub findings: Vec<Finding>,
}

/// True when `head` starts with a MATLAB MAT-file header (versions 5.0 and 7.3).
pub fn looks_like_mat(head: &[u8]) -> bool {
    mat::mat_version(head).is_some()
}

fn number(v: Option<&MatValue>) -> Option<f64> {
    v.and_then(MatValue::as_f64).filter(|x| x.is_finite())
}

fn text(v: Option<&MatValue>) -> String {
    v.and_then(MatValue::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// A number as EEGLAB shows an event type that is numeric (`3`, `2.5`).
fn number_text(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}

/// The `EEG` field `name`: of the variable `EEG` or the variable of that name.
fn eeg_field<'a>(m: &'a MatFile, single: bool, name: &str) -> Option<&'a MatValue> {
    if single {
        m.variable("EEG")?.value.field(0, name)
    } else {
        m.variable(name).map(|v| &v.value)
    }
}

/// Interpret a parsed MAT-file as an EEGLAB dataset next to `set`.
pub fn read_set(fs: &Fs, set: &Path, mat: MatFile) -> Result<EeglabSet> {
    let single = mat
        .variable("EEG")
        .is_some_and(|v| matches!(v.value, MatValue::Struct { .. }));
    if mat.variable("ALLEEG").is_some() && !single {
        return Err(Error::unsupported(
            EEGLAB_FORMAT_ID,
            "a study file holding several datasets (ALLEEG)",
            "Save each dataset with pop_saveset and open those .set files.",
        ));
    }
    let get = |n: &str| eeg_field(&mat, single, n);
    let required = |n: &str| {
        number(get(n)).ok_or_else(|| {
            Error::corrupt(
                EEGLAB_FORMAT_ID,
                format!("the EEG structure has no numeric `{n}`; this MAT-file may not be an EEGLAB dataset"),
            )
        })
    };
    let srate = required("srate")?;
    let channels = required("nbchan")? as u64;
    let points = required("pnts")? as u64;
    let trials = number(get("trials")).unwrap_or(1.0).max(1.0) as u64;
    if !(srate > 0.0) || channels == 0 {
        return Err(Error::corrupt(
            EEGLAB_FORMAT_ID,
            format!("srate {srate} and nbchan {channels} are not usable"),
        ));
    }
    let mut findings = Vec::new();
    let mut labels = Vec::new();
    let mut types = Vec::new();
    if let Some(c) = get("chanlocs").filter(|c| matches!(c, MatValue::Struct { .. })) {
        for e in 0..c.len() {
            labels.push(text(c.field(e, "labels")));
            types.push(text(c.field(e, "type")));
        }
        if labels.len() as u64 != channels {
            findings.push(Finding::warning(
                "chanlocs_count",
                format!("chanlocs has {} entries for {channels} channels; channels are named ch0, ch1 …", labels.len()),
            ));
            labels.clear();
            types.clear();
        }
    }
    let total = channels
        .checked_mul(points)
        .and_then(|n| n.checked_mul(trials))
        .ok_or_else(|| Error::corrupt(EEGLAB_FORMAT_ID, "nbchan × pnts × trials overflows"))?;
    let data_var = if single {
        mat.variables.iter().position(|v| v.name == "EEG")
    } else {
        mat.variables.iter().position(|v| v.name == "data")
    };
    let data = match get("data") {
        Some(MatValue::Char { text: name, .. }) => {
            let datfile = text(get("datfile"));
            let named = if !datfile.is_empty() {
                datfile
            } else {
                name.trim().to_string()
            };
            let lower = named.to_ascii_lowercase();
            if lower.ends_with(".dat") {
                return Err(Error::unsupported(
                    EEGLAB_FORMAT_ID,
                    "samples in an old-style .dat file",
                    "Load the dataset in EEGLAB and save it again, which writes a .fdt file.",
                ));
            }
            EeglabData::Fdt(resolve(fs, set, &named))
        }
        Some(MatValue::Numeric { class, data, .. }) => {
            if data.count != total {
                return Err(Error::corrupt(
                    EEGLAB_FORMAT_ID,
                    format!(
                        "the data array holds {} values; nbchan × pnts × trials is {total}",
                        data.count
                    ),
                ));
            }
            EeglabData::Embedded {
                variable: data_var.unwrap_or(0),
                stored: data.stored,
                at: data.at,
                values: data.values.clone(),
                class: class_name(*class).into(),
            }
        }
        _ => {
            return Err(Error::corrupt(
                EEGLAB_FORMAT_ID,
                "the EEG structure has no `data` (neither a file name nor an array)",
            ));
        }
    };
    let mut events = Vec::new();
    if let Some(ev) = get("event").filter(|c| matches!(c, MatValue::Struct { .. })) {
        for e in 0..ev.len() {
            let kind = match ev.field(e, "type") {
                Some(v @ MatValue::Char { .. }) => text(Some(v)),
                Some(v) => v.as_f64().map(number_text).unwrap_or_default(),
                None => String::new(),
            };
            let Some(latency) = number(ev.field(e, "latency")) else {
                continue;
            };
            let duration = match ev.field(e, "duration") {
                None => 0.0,
                Some(v) if v.is_empty() => f64::NAN,
                Some(v) => v.as_f64().unwrap_or(f64::NAN),
            };
            events.push(EeglabEvent {
                kind,
                latency,
                duration,
                epoch: number(ev.field(e, "epoch")).unwrap_or(0.0),
            });
        }
        if (events.len() as u64) < ev.len() {
            findings.push(Finding::warning(
                "event_without_latency",
                format!(
                    "{} event(s) without a numeric latency were left out",
                    ev.len() - events.len() as u64
                ),
            ));
        }
    }
    let ica_components = match get("icaweights") {
        Some(MatValue::Numeric { dims, .. }) => dims.first().copied().unwrap_or(0),
        _ => 0,
    };
    let eeglab_version = get("etc")
        .and_then(|e| e.field(0, "eeglabvers"))
        .map(|v| text(Some(v)))
        .filter(|v| !v.is_empty());
    Ok(EeglabSet {
        setname: text(get("setname")),
        reference: text(get("ref")),
        single_variable: single,
        channels,
        points,
        trials,
        srate,
        xmin: number(get("xmin")).unwrap_or(0.0),
        labels,
        types,
        data,
        events,
        ica_components,
        eeglab_version,
        findings,
        mat,
    })
}

fn class_name(c: u8) -> &'static str {
    match c {
        6 => "float64",
        7 => "float32",
        8 => "int8",
        9 => "uint8",
        10 => "int16",
        11 => "uint16",
        12 => "int32",
        13 => "uint32",
        14 => "int64",
        15 => "uint64",
        _ => "float64",
    }
}

/// The `.fdt` the dataset names, next to the `.set`: as named, without regard to case, else the
/// `.set`'s own stem with `.fdt` (EEGLAB and MNE do the same after a rename).
fn resolve(fs: &Fs, set: &Path, named: &str) -> PathBuf {
    let dir = set.parent().unwrap_or(Path::new(""));
    let name = Path::new(named)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !name.is_empty() {
        let p = dir.join(&name);
        if fs.is_file(&p) {
            return p;
        }
        if let Some(p) = fs.find_in_dir(dir, &name) {
            return p;
        }
    }
    set.with_extension("fdt")
}

impl EeglabSet {
    /// The MAT-file writer from the header text: `MATLAB <platform>`, `SciPy` (Python's
    /// `savemat`, as eeglabio and MNE use), or `Octave`.
    pub fn mat_writer(&self) -> String {
        let h = &self.mat.header_text;
        let l = h.to_ascii_lowercase();
        if l.contains("octave") {
            return "Octave".into();
        }
        // SciPy's savemat writes `MAT-file Platform:`; MATLAB writes `MAT-file, Platform:`
        if l.starts_with("matlab 5.0 mat-file platform:") {
            return "SciPy".into();
        }
        match h
            .split("Platform:")
            .nth(1)
            .and_then(|r| r.split(',').next())
        {
            Some(p) => format!("MATLAB {}", p.trim()),
            None => "MATLAB".into(),
        }
    }
    /// The MAT-file's variables hold compressed (`miCOMPRESSED`, version 7) data.
    pub fn compressed(&self) -> bool {
        self.mat.variables.iter().any(|v| v.compressed.is_some())
    }
}

/// The EEGLAB reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct EeglabReader;

impl FormatReader for EeglabReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::EEGLAB)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: EEGLAB_FORMAT_ID.into(),
            name: "EEGLAB dataset (.set, .fdt)".into(),
            vendor: "Swartz Center for Computational Neuroscience (EEGLAB); also MNE-Python, eeglabio".into(),
            extensions: vec!["set".into(), "fdt".into()],
            family: "electrophysiology".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::EEGLAB.confidence,
            known_gaps: vec![
                "MAT-file version 7.3 (HDF5) datasets and old .dat sample files are refused".into(),
                "EEGLAB stores no unit; the values are as stored (µV by EEGLAB's convention)".into(),
                "ICA weights, channel positions and the history are kept in the vendor metadata, not decoded".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if has_extension(path, &["set"]) && looks_like_mat(head) {
            return Some(Detection {
                format_id: EEGLAB_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        None
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        let path = input.path();
        if let Some(d) = self.sniff(head, path) {
            return Some(d);
        }
        (has_extension(path, &["fdt"]) && input.fs().is_file(&path.with_extension("set"))).then(
            || Detection {
                format_id: EEGLAB_FORMAT_ID,
                confidence: DetectConfidence::Likely,
                note: Some("reads the dataset through the .set next to it".into()),
            },
        )
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(EeglabDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(EeglabDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// An opened EEGLAB dataset.
#[derive(Debug)]
pub struct EeglabDataset {
    path: PathBuf,
    fs: Fs,
    set_len: u64,
    set: EeglabSet,
    data_len: u64,
    /// Inflated samples of a compressed `.set`, kept after the first read.
    cache: Option<Vec<u8>>,
    kinds: Vec<String>,
}

impl EeglabDataset {
    /// Open a `.set` (or the `.fdt` next to it).
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`].
    pub fn open_input(input: &Input) -> Result<Self> {
        let fs = input.fs().clone();
        let given = input.path().to_path_buf();
        let path = if has_extension(&given, &["fdt"]) {
            given.with_extension("set")
        } else {
            given
        };
        let mut f = fs.open(&path).map_err(|e| Error::io(&path, e))?;
        let set_len = f.size().map_err(|e| Error::io(&path, e))?;
        let parsed = mat::parse_mat(&mut f, &path, set_len)?;
        let mut set = read_set(&fs, &path, parsed)?;
        let total = set.channels * set.points * set.trials;
        let data_len = match &set.data {
            EeglabData::Fdt(p) => {
                let g = fs.open(p).map_err(|e| {
                    Error::unsupported(
                        EEGLAB_FORMAT_ID,
                        "a dataset without its .fdt",
                        format!("The samples are in {}, which could not be opened ({e}); it must sit next to the .set.", p.display()),
                    )
                })?;
                let n = g.size().map_err(|e| Error::io(p, e))?;
                if n < total * 4 {
                    set.findings.push(Finding::error(
                        "truncated",
                        format!("the .fdt holds {n} bytes; nbchan × pnts × trials float32 samples need {}", total * 4),
                    ));
                } else if n > total * 4 {
                    set.findings.push(Finding::warning(
                        "extra_bytes",
                        format!("{} bytes after the samples in the .fdt", n - total * 4),
                    ));
                }
                n
            }
            EeglabData::Embedded { stored, .. } => total * mat::mi_size(*stored),
        };
        let mut kinds: Vec<String> = Vec::new();
        for e in &set.events {
            if !kinds.contains(&e.kind) {
                kinds.push(e.kind.clone());
            }
        }
        Ok(EeglabDataset {
            path,
            fs,
            set_len,
            set,
            data_len,
            cache: None,
            kinds,
        })
    }

    /// The parsed dataset.
    pub fn eeglab(&self) -> &EeglabSet {
        &self.set
    }

    /// Samples available per channel in each epoch (fewer than `pnts` in a cut-short `.fdt`).
    fn whole_epochs(&self) -> u64 {
        match &self.set.data {
            EeglabData::Fdt(_) => {
                let frame = self.set.channels * 4;
                (self.data_len / frame) / self.set.points.max(1)
            }
            EeglabData::Embedded { .. } => self.set.trials,
        }
    }

    /// Raw stored bytes of frames `[first, first + n)` (frame = one sample of every channel).
    fn frames(&mut self, first: u64, n: u64) -> Result<(Vec<u8>, mat::MiType)> {
        let nch = self.set.channels;
        match self.set.data.clone() {
            EeglabData::Fdt(p) => {
                let mut g = self.fs.open(&p).map_err(|e| Error::io(&p, e))?;
                let at = first * nch * 4;
                let b = read_block(&mut g, &p, at, n * nch * 4, self.data_len)?;
                if (b.len() as u64) < n * nch * 4 {
                    return Err(Error::corrupt_at(
                        EEGLAB_FORMAT_ID,
                        at,
                        "samples cut off by the end of the .fdt",
                    ));
                }
                Ok((b.bytes, mat::MI_SINGLE))
            }
            EeglabData::Embedded {
                variable,
                stored,
                at,
                values,
                ..
            } => {
                let size = mat::mi_size(stored);
                if let Some(v) = values {
                    // read at parse time: re-encode as little-endian f64
                    let a = (first * nch) as usize;
                    let b = ((first + n) * nch) as usize;
                    let out = v
                        .get(a..b)
                        .unwrap_or_default()
                        .iter()
                        .flat_map(|x| x.to_le_bytes())
                        .collect();
                    return Ok((out, 0));
                }
                let var: MatVariable = self.set.mat.variables[variable].clone();
                if var.compressed.is_some() {
                    if self.cache.is_none() {
                        if self.data_len > MAX_EEGLAB_INFLATED {
                            return Err(Error::unsupported(
                                EEGLAB_FORMAT_ID,
                                "samples over 2 GiB inside a compressed .set",
                                "Save the dataset with its samples in a separate .fdt file (pop_saveset 'savemode' 'twofiles').",
                            ));
                        }
                        let mut f = self
                            .fs
                            .open(&self.path)
                            .map_err(|e| Error::io(&self.path, e))?;
                        self.cache = Some(mat::read_stream(
                            &mut f,
                            &self.path,
                            self.set_len,
                            &var,
                            at,
                            self.data_len,
                        )?);
                    }
                    let c = self.cache.as_ref().expect("just filled");
                    let a = (first * nch * size) as usize;
                    let b = ((first + n) * nch * size) as usize;
                    return Ok((c.get(a..b).unwrap_or_default().to_vec(), stored));
                }
                let mut f = self
                    .fs
                    .open(&self.path)
                    .map_err(|e| Error::io(&self.path, e))?;
                let b = mat::read_stream(
                    &mut f,
                    &self.path,
                    self.set_len,
                    &var,
                    at + first * nch * size,
                    n * nch * size,
                )?;
                Ok((b, stored))
            }
        }
    }
}

impl Dataset for EeglabDataset {
    fn info(&self) -> Result<FileInfo> {
        let s = &self.set;
        let mut notes = vec![format!(
            "EEGLAB dataset ({} MAT-file{}): {} channel(s), {} epoch(s) of {} samples at {} Hz, samples {}",
            s.mat_writer(),
            if s.compressed() { ", compressed" } else { "" },
            s.channels,
            s.trials,
            s.points,
            s.srate,
            match &s.data {
                EeglabData::Fdt(p) => format!(
                    "in {}",
                    p.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ),
                EeglabData::Embedded { .. } => "inside the .set".into(),
            }
        )];
        notes.push(
            "EEGLAB records no unit: values are as stored (µV by EEGLAB's convention)".into(),
        );
        if !s.events.is_empty() {
            notes.push(format!(
                "{} event(s) in the `events` table (onset in seconds after the first sample of the first epoch)",
                s.events.len()
            ));
        }
        if s.findings.iter().any(|x| x.severity == Severity::Error) {
            notes.push("structural problems found (e.g. truncation); run `check`".into());
        }
        let epochs = self.whole_epochs().min(s.trials);
        let dtype = match &s.data {
            EeglabData::Fdt(_) => "float32".to_string(),
            EeglabData::Embedded { class, .. } => class.clone(),
        };
        let mut extra = BTreeMap::new();
        extra.insert("mat_version".into(), json!("5.0"));
        extra.insert("mat_writer".into(), json!(s.mat_writer()));
        extra.insert("application".into(), json!("EEGLAB"));
        if let Some(v) = &s.eeglab_version {
            extra.insert("application_version".into(), json!(v));
        }
        extra.insert(
            "sample_storage".into(),
            json!(match &s.data {
                EeglabData::Fdt(_) => "fdt",
                EeglabData::Embedded { .. } => "embedded",
            }),
        );
        if !s.setname.is_empty() {
            extra.insert("setname".into(), json!(s.setname));
        }
        if !s.reference.is_empty() {
            extra.insert("reference".into(), json!(s.reference));
        }
        if s.ica_components > 0 {
            extra.insert("ica_components".into(), json!(s.ica_components));
        }
        if s.trials > 1 {
            extra.insert("epoch_start_s".into(), json!(s.xmin));
        }
        let channels = (0..s.channels as usize)
            .map(|c| {
                let mut e = BTreeMap::new();
                if let Some(t) = s.types.get(c).filter(|t| !t.is_empty()) {
                    e.insert("type".into(), json!(t));
                }
                SignalChannelInfo {
                    index: c as u32,
                    name: s
                        .labels
                        .get(c)
                        .filter(|l| !l.is_empty())
                        .cloned()
                        .unwrap_or_else(|| format!("ch{c}")),
                    unit: None,
                    dtype: dtype.clone(),
                    scale: 1.0,
                    offset: 0.0,
                    extra: e,
                }
            })
            .collect();
        let traces = if epochs == 0 || s.points == 0 {
            notes.push("the dataset holds no whole epoch of samples".into());
            Vec::new()
        } else {
            vec![TraceInfo {
                index: 0,
                name: Some(if s.trials > 1 { "epochs" } else { "signals" }.into()),
                sample_rate_hz: s.srate,
                sample_count: s.points,
                sweep_count: u32::try_from(epochs).unwrap_or(u32::MAX),
                channels,
                start_s: Some(if s.trials > 1 { s.xmin } else { 0.0 }),
                extra,
            }]
        };
        let tables = if s.events.is_empty() {
            Vec::new()
        } else {
            let col = |i: u32, name: &str, dtype: &str, unit: Option<&str>| ColumnInfo {
                index: i,
                name: name.into(),
                label: None,
                dtype: dtype.into(),
                unit: unit.map(str::to_string),
                range: None,
                extra: BTreeMap::new(),
            };
            let mut kind = col(5, "type", "int32", None);
            kind.extra.insert("categories".into(), json!(self.kinds));
            vec![TableInfo {
                index: 0,
                name: Some("events".into()),
                row_count: s.events.len() as u64,
                columns: vec![
                    col(0, "onset_s", "float64", Some("s")),
                    col(1, "duration_s", "float64", Some("s")),
                    col(2, "latency", "float64", None),
                    col(3, "duration", "float64", None),
                    col(4, "epoch", "int32", None),
                    kind,
                ],
                extra: BTreeMap::new(),
            }]
        };
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.set_len,
            format: EeglabReader.descriptor(),
            format_version: Some("MAT 5.0".into()),
            images: Vec::new(),
            tables,
            spectra: Vec::new(),
            traces,
            plane_count: 0,
            notes,
        })
    }

    fn member_files(&self) -> Vec<PathBuf> {
        let mut v = vec![self.path.clone()];
        if let EeglabData::Fdt(p) = &self.set.data {
            v.push(p.clone());
        }
        v.into_iter().filter(|p| self.fs.is_file(p)).collect()
    }

    fn vendor_metadata(&self) -> Result<Value> {
        fn j(v: &MatValue, depth: usize) -> Value {
            if depth > 6 {
                return Value::Null;
            }
            match v {
                MatValue::Char { text, .. } => json!(text),
                MatValue::Numeric { dims, data, .. } => match &data.values {
                    Some(x) if x.len() == 1 => json!(x[0]),
                    Some(x) if x.len() <= 64 => json!(x),
                    _ => json!({"dims": dims, "values": data.count}),
                },
                MatValue::Cell { items, .. } => json!(
                    items
                        .iter()
                        .take(64)
                        .map(|i| j(i, depth + 1))
                        .collect::<Vec<_>>()
                ),
                MatValue::Struct { fields, values, .. } => {
                    let n = if fields.is_empty() {
                        0
                    } else {
                        values.len() / fields.len()
                    };
                    let one = |e: usize| {
                        let m: serde_json::Map<String, Value> = fields
                            .iter()
                            .enumerate()
                            .map(|(k, f)| (f.clone(), j(&values[e * fields.len() + k], depth + 1)))
                            .collect();
                        Value::Object(m)
                    };
                    if n == 1 {
                        one(0)
                    } else {
                        json!((0..n.min(64)).map(one).collect::<Vec<_>>())
                    }
                }
                MatValue::Other { class } => json!({"class": class}),
            }
        }
        let s = &self.set;
        let vars: serde_json::Map<String, Value> = s
            .mat
            .variables
            .iter()
            .filter(|v| v.name != "data")
            .map(|v| (v.name.clone(), j(&v.value, 0)))
            .collect();
        Ok(json!({"header": s.mat.header_text, "variables": vars}))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for k in [
            "traces[].sample_rate_hz",
            "traces[].sample_count",
            "traces[].sweep_count",
            "traces[].start_s",
            "traces[].channels[].name",
            "tables[].row_count",
        ] {
            p.insert(k.into(), Source::PriorArt);
        }
        p.insert("format_version".into(), Source::Inferred);
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let s = &self.set;
        let mut out: Vec<LsEntry> = s
            .mat
            .variables
            .iter()
            .map(|v| LsEntry {
                kind: "variable".into(),
                name: v.name.clone(),
                offset: v.compressed.map(|c| c.0),
                size: v.compressed.map(|c| c.1),
                image: None,
                details: json!({"compressed": v.compressed.is_some(), "elements": v.value.len()}),
            })
            .collect();
        if let EeglabData::Fdt(p) = &s.data {
            out.push(LsEntry {
                kind: "samples".into(),
                name: p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                offset: Some(0),
                size: Some(self.data_len),
                image: None,
                details: json!({"dtype": "float32"}),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            EEGLAB_FORMAT_ID,
            "image planes",
            "EEGLAB datasets hold sampled signals: use `openreadout trace`.",
        ))
    }

    fn read_trace(&mut self, index: u32, sweep: u32, first: u64, max: u64) -> Result<Trace> {
        let epochs = self.whole_epochs().min(self.set.trials);
        if index != 0 || epochs == 0 {
            return Err(Error::Usage(format!("trace index {index} out of range")));
        }
        if u64::from(sweep) >= epochs {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range ({epochs} epochs)"
            )));
        }
        let n = self.set.points;
        if first > n {
            return Err(Error::Usage(format!(
                "first sample {first} is past the end ({n} samples)"
            )));
        }
        let nch = self.set.channels;
        let count = max
            .min(n - first)
            .min(MAX_EEGLAB_READ)
            .min((MAX_EEGLAB_READ_BYTES / (nch * 8)).max(1));
        let start = u64::from(sweep) * n + first;
        let (bytes, stored) = self.frames(start, count)?;
        let vals = if stored == 0 {
            bytes
                .chunks_exact(8)
                .map(|b| f64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
                .collect::<Vec<_>>()
        } else if matches!(self.set.data, EeglabData::Fdt(_)) {
            bytes
                .chunks_exact(4)
                .map(|b| f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]])))
                .collect()
        } else {
            mat::decode_values(self.set.mat.big_endian, stored, &bytes)
        };
        let mut channels = vec![Vec::with_capacity(count as usize); nch as usize];
        for frame in vals.chunks_exact(nch as usize) {
            for (c, v) in frame.iter().enumerate() {
                channels[c].push(*v);
            }
        }
        Ok(Trace {
            trace: index,
            sweep,
            first_sample: first,
            channels,
        })
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        let s = &self.set;
        if index != 0 || s.events.is_empty() {
            return Err(Error::Usage(format!("table index {index} out of range")));
        }
        let a = (first_row as usize).min(s.events.len());
        let b = a
            .saturating_add(max_rows.min(usize::MAX as u64) as usize)
            .min(s.events.len());
        let rows = &s.events[a..b];
        let code = |k: &str| {
            self.kinds
                .iter()
                .position(|x| x == k)
                .map_or(f64::NAN, |i| i as f64)
        };
        Ok(Table {
            table: 0,
            first_row: a as u64,
            columns: vec![
                rows.iter().map(|e| (e.latency - 1.0) / s.srate).collect(),
                rows.iter().map(|e| e.duration / s.srate).collect(),
                rows.iter().map(|e| e.latency).collect(),
                rows.iter().map(|e| e.duration).collect(),
                rows.iter().map(|e| e.epoch).collect(),
                rows.iter().map(|e| code(&e.kind)).collect(),
            ],
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let s = &self.set;
        let mut r = CheckReport::new(self.path.display().to_string(), EEGLAB_FORMAT_ID);
        r.performed("MAT-file header and every variable parsed (compressed variables inflated)");
        r.performed("EEG fields srate, nbchan, pnts, trials, data, chanlocs and event read; the samples' size against nbchan × pnts × trials");
        for x in &s.findings {
            r.push(x.clone());
        }
        let total = (s.channels * s.points * s.trials) as f64;
        let outside = s
            .events
            .iter()
            .filter(|e| e.latency < 0.5 || e.latency > total / s.channels as f64 + 0.5)
            .count();
        if outside > 0 {
            r.push(Finding::warning(
                "event_outside_data",
                format!("{outside} event(s) lie outside the samples"),
            ));
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests;
