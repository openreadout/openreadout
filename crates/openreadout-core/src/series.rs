//! A ready-made [`Dataset`] for readers whose files are small sets of sampled columns: an EPR
//! spectrum, a diffraction scan, a potentiostat or battery-cycler record, a thermal-analysis run.
//! Such a reader parses the whole file at open into a [`SeriesFile`] (traces of named channels,
//! tables, experiment facts, the vendor tree, findings) and wraps it in a [`SeriesDataset`], which
//! serves `info`, `trace`, `table`, `check`, `info --view structure` and `info --view full` from
//! memory.
//!
//! Conventions (the same as the other readers'):
//!
//! - a trace's `extra.axis` is `{quantity, unit, first, step, size}` for an evenly spaced
//!   abscissa, or `{quantity, unit, irregular: true, channel: k}` when channel `k` holds it;
//! - a trace may hold several sweeps of the same channels (a 2D EPR data set: one field sweep
//!   per angle; one diffraction scan per range); sweeps may differ in length
//!   (`extra.sweep_sample_counts`);
//! - a table's text column is a category code: the values are indices into the column's
//!   `extra.categories`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::assurance::Observations;
use crate::error::{Error, Result};
use crate::experiment::Experiment;
use crate::model::{
    CheckReport, ColumnInfo, FileInfo, Finding, FormatDescriptor, LsEntry, Severity,
    SignalChannelInfo, Table, TableInfo, Trace, TraceInfo,
};
use crate::pixel::Plane;
use crate::provenance::ProvenanceMap;
use crate::reader::{Dataset, PlaneIndex};
use crate::source::Input;

/// One channel of a [`SeriesTrace`].
#[derive(Debug, Clone, Default)]
pub struct SeriesChannel {
    /// Channel name in the reader's vocabulary (`intensity`, `potential`, `current`, ...).
    pub name: String,
    /// Physical unit of the values, if any.
    pub unit: Option<String>,
    /// dtype of the stored values (`float32`, `int32`, `float64`; text formats: `float64`).
    pub dtype: String,
    /// Reader-specific channel metadata in the format's documented vocabulary.
    pub extra: BTreeMap<String, Value>,
}

impl SeriesChannel {
    /// A channel with a name, a unit and the stored dtype.
    pub fn new(name: impl Into<String>, unit: Option<&str>, dtype: &str) -> Self {
        SeriesChannel {
            name: name.into(),
            unit: unit.map(str::to_string),
            dtype: dtype.into(),
            extra: BTreeMap::new(),
        }
    }
}

/// A trace: channels sampled together, in one or more sweeps.
#[derive(Debug, Clone, Default)]
pub struct SeriesTrace {
    /// Trace name.
    pub name: String,
    /// The channels, in order.
    pub channels: Vec<SeriesChannel>,
    /// `sweeps[s][c][i]`: sample `i` of channel `c` in sweep `s`. Every sweep has one vector
    /// per channel, all of the same length within the sweep.
    pub sweeps: Vec<Vec<Vec<f64>>>,
    /// Samples per second when the trace is sampled in time at a fixed rate, else 0.
    pub sample_rate_hz: f64,
    /// Time of the first sample in seconds, when the trace is sampled in time.
    pub start_s: Option<f64>,
    /// Reader-specific metadata (`axis`, `kind`, ...).
    pub extra: BTreeMap<String, Value>,
}

impl SeriesTrace {
    fn sample_counts(&self) -> Vec<u64> {
        self.sweeps
            .iter()
            .map(|s| s.first().map_or(0, |c| c.len() as u64))
            .collect()
    }
}

/// One column of a [`SeriesTable`].
#[derive(Debug, Clone, Default)]
pub struct SeriesColumn {
    /// Column name in the reader's vocabulary.
    pub name: String,
    /// Physical unit, if any.
    pub unit: Option<String>,
    /// The values; for a text column, indices into `categories`.
    pub values: Vec<f64>,
    /// The texts of a category-coded column.
    pub categories: Option<Vec<String>>,
    /// Reader-specific column metadata.
    pub extra: BTreeMap<String, Value>,
}

impl SeriesColumn {
    /// A numeric column.
    pub fn numbers(name: impl Into<String>, unit: Option<&str>, values: Vec<f64>) -> Self {
        SeriesColumn {
            name: name.into(),
            unit: unit.map(str::to_string),
            values,
            categories: None,
            extra: BTreeMap::new(),
        }
    }

    /// A text column, coded as indices into its distinct texts (in order of first use).
    pub fn texts<S: AsRef<str>>(name: impl Into<String>, texts: &[S]) -> Self {
        let mut cats: Vec<String> = Vec::new();
        let mut index: BTreeMap<String, usize> = BTreeMap::new();
        let values = texts
            .iter()
            .map(|t| {
                let t = t.as_ref();
                let k = *index.entry(t.to_string()).or_insert_with(|| {
                    cats.push(t.to_string());
                    cats.len() - 1
                });
                k as f64
            })
            .collect();
        SeriesColumn {
            name: name.into(),
            unit: None,
            values,
            categories: Some(cats),
            extra: BTreeMap::new(),
        }
    }
}

/// A table: named columns of equal length.
#[derive(Debug, Clone, Default)]
pub struct SeriesTable {
    /// Table name.
    pub name: String,
    /// The columns.
    pub columns: Vec<SeriesColumn>,
    /// Reader-specific table metadata (`kind`, `trace`, ...).
    pub extra: BTreeMap<String, Value>,
}

impl SeriesTable {
    fn rows(&self) -> u64 {
        self.columns
            .iter()
            .map(|c| c.values.len() as u64)
            .max()
            .unwrap_or(0)
    }
}

/// Everything a reader learned from a file.
#[derive(Debug, Clone, Default)]
pub struct SeriesFile {
    /// The file's format or layout version, as `info` reports it.
    pub format_version: Option<String>,
    /// Traces, in order.
    pub traces: Vec<SeriesTrace>,
    /// Tables, in order.
    pub tables: Vec<SeriesTable>,
    /// Experiment facts, each with its origin in `provenance` (laid over those derived from
    /// `info`).
    pub experiment: Option<Experiment>,
    /// The vendor tree (`info --view full` → `vendor`).
    pub vendor: Value,
    /// The file's structure for `info --view structure`.
    pub entries: Vec<LsEntry>,
    /// Problems found while parsing (`check`; errors are also summarised by `info`).
    pub findings: Vec<Finding>,
    /// Notes for `info`.
    pub notes: Vec<String>,
    /// Provenance of the normalized fields.
    pub provenance: ProvenanceMap,
    /// Assurance observations only the reader can make (undecoded structures, assumptions).
    pub observations: Observations,
    /// What `check` verified beyond parsing, one line each.
    pub checks: Vec<String>,
    /// The files the data set is made of, when it is more than the path opened (a descriptor
    /// and a data file).
    pub members: Vec<PathBuf>,
}

/// An opened file served from a [`SeriesFile`].
#[derive(Debug)]
pub struct SeriesDataset {
    descriptor: FormatDescriptor,
    path: PathBuf,
    size: u64,
    file: SeriesFile,
}

/// Round-trip-safe JSON number (non-finite → null).
pub fn json_num(v: f64) -> Value {
    if v.is_finite() { json!(v) } else { Value::Null }
}

/// Read a whole (small) file into memory, refusing files larger than `cap` bytes.
pub fn read_all(input: &Input, format: &'static str, cap: u64) -> Result<Vec<u8>> {
    let f = input.open()?;
    let len = f.size().map_err(|e| Error::io(input.path(), e))?;
    if len > cap {
        return Err(Error::unsupported(
            format,
            format!("a {} MiB file", len >> 20),
            "Files this large are not read by this reader; no instrument of this kind writes them.",
        ));
    }
    let n = usize::try_from(len).map_err(|_| Error::Other("file larger than memory".into()))?;
    let mut buf = vec![0u8; n];
    f.read_exact_at(0, &mut buf)
        .map_err(|e| Error::io(input.path(), e))?;
    Ok(buf)
}

impl SeriesDataset {
    /// Wrap a parsed file.
    pub fn new(descriptor: FormatDescriptor, path: PathBuf, size: u64, file: SeriesFile) -> Self {
        SeriesDataset {
            descriptor,
            path,
            size,
            file,
        }
    }

    /// The parsed file.
    pub fn file(&self) -> &SeriesFile {
        &self.file
    }

    fn trace_info(index: u32, t: &SeriesTrace) -> TraceInfo {
        let counts = t.sample_counts();
        let longest = counts.iter().copied().max().unwrap_or(0);
        let mut extra = t.extra.clone();
        if counts.iter().any(|c| *c != longest) {
            extra.insert("sweep_sample_counts".into(), json!(counts));
        }
        TraceInfo {
            index,
            name: Some(t.name.clone()),
            sample_rate_hz: t.sample_rate_hz,
            sample_count: longest,
            sweep_count: u32::try_from(t.sweeps.len()).unwrap_or(u32::MAX),
            channels: t
                .channels
                .iter()
                .enumerate()
                .map(|(i, c)| SignalChannelInfo {
                    index: u32::try_from(i).unwrap_or(u32::MAX),
                    name: c.name.clone(),
                    unit: c.unit.clone(),
                    dtype: if c.dtype.is_empty() {
                        "float64".into()
                    } else {
                        c.dtype.clone()
                    },
                    scale: 1.0,
                    offset: 0.0,
                    extra: c.extra.clone(),
                })
                .collect(),
            start_s: t.start_s,
            extra,
        }
    }

    fn table_info(index: u32, t: &SeriesTable) -> TableInfo {
        TableInfo {
            index,
            name: Some(t.name.clone()),
            row_count: t.rows(),
            columns: t
                .columns
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let mut extra = c.extra.clone();
                    if let Some(cats) = &c.categories {
                        extra.insert("categories".into(), json!(cats));
                    }
                    ColumnInfo {
                        index: u32::try_from(i).unwrap_or(u32::MAX),
                        name: c.name.clone(),
                        label: None,
                        dtype: if c.categories.is_some() {
                            "uint32".into()
                        } else {
                            "float64".into()
                        },
                        unit: c.unit.clone(),
                        range: None,
                        extra,
                    }
                })
                .collect(),
            extra: t.extra.clone(),
        }
    }

    fn info_notes(&self) -> Vec<String> {
        let mut notes = self.file.notes.clone();
        let errors: Vec<&Finding> = self
            .file
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .collect();
        if let Some(first) = errors.first() {
            let more = match errors.len() {
                1 => String::new(),
                n => format!(" and {} more", n - 1),
            };
            notes.push(format!(
                "the file is damaged ({}{more}); run `check` for the list",
                first.message
            ));
        }
        notes
    }
}

#[allow(clippy::many_single_char_names)] // slicing code: a, b (window), n, s, t
impl Dataset for SeriesDataset {
    fn info(&self) -> Result<FileInfo> {
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.size,
            format: self.descriptor.clone(),
            format_version: self.file.format_version.clone(),
            plane_count: 0,
            images: Vec::new(),
            tables: self
                .file
                .tables
                .iter()
                .enumerate()
                .map(|(i, t)| Self::table_info(u32::try_from(i).unwrap_or(u32::MAX), t))
                .collect(),
            spectra: Vec::new(),
            traces: self
                .file
                .traces
                .iter()
                .enumerate()
                .map(|(i, t)| Self::trace_info(u32::try_from(i).unwrap_or(u32::MAX), t))
                .collect(),
            notes: self.info_notes(),
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        Ok(self.file.vendor.clone())
    }

    fn provenance(&self) -> ProvenanceMap {
        self.file.provenance.clone()
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(self.file.entries.clone())
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::Usage(format!(
            "a {} file holds traces and tables, not images",
            self.descriptor.name
        )))
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = self.check_headers()?;
        for c in &self.file.checks {
            r.performed(c.clone());
        }
        r.performed("every value decoded; values checked for non-finite numbers");
        let mut bad = 0u64;
        for t in &self.file.traces {
            for s in &t.sweeps {
                for c in s {
                    bad += c.iter().filter(|v| !v.is_finite()).count() as u64;
                }
            }
        }
        if bad > 0 {
            r.push(Finding::info(
                "non_finite_values",
                format!("{bad} samples are not finite numbers (NaN or infinite, as stored)"),
            ));
        }
        Ok(r)
    }

    fn check_headers(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), self.descriptor.id.clone());
        r.performed("file parsed; record sizes and counts checked against the data");
        for f in &self.file.findings {
            r.push(f.clone());
        }
        Ok(r)
    }

    fn read_trace(&mut self, index: u32, sweep: u32, first: u64, max: u64) -> Result<Trace> {
        let t = self.file.traces.get(index as usize).ok_or_else(|| {
            Error::Usage(format!(
                "trace {index} out of range (file has {} traces)",
                self.file.traces.len()
            ))
        })?;
        let s = t.sweeps.get(sweep as usize).ok_or_else(|| {
            Error::Usage(format!(
                "sweep {sweep} out of range (trace {index} has {} sweeps)",
                t.sweeps.len()
            ))
        })?;
        let n = s.first().map_or(0, Vec::len);
        let a = usize::try_from(first).unwrap_or(usize::MAX).min(n);
        let b = usize::try_from(first.saturating_add(max))
            .unwrap_or(usize::MAX)
            .min(n);
        Ok(Trace {
            trace: index,
            sweep,
            first_sample: a as u64,
            channels: s
                .iter()
                .map(|c| c.get(a..b).map(<[f64]>::to_vec).unwrap_or_default())
                .collect(),
        })
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        let t = self.file.tables.get(index as usize).ok_or_else(|| {
            Error::Usage(format!(
                "table {index} out of range (file has {} tables)",
                self.file.tables.len()
            ))
        })?;
        let n = usize::try_from(t.rows()).unwrap_or(usize::MAX);
        let a = usize::try_from(first_row).unwrap_or(usize::MAX).min(n);
        let b = usize::try_from(first_row.saturating_add(max_rows))
            .unwrap_or(usize::MAX)
            .min(n);
        Ok(Table {
            table: index,
            first_row: a as u64,
            columns: t
                .columns
                .iter()
                .map(|c| {
                    (a..b)
                        .map(|i| c.values.get(i).copied().unwrap_or(f64::NAN))
                        .collect()
                })
                .collect(),
        })
    }

    fn member_files(&self) -> Vec<PathBuf> {
        self.file.members.clone()
    }

    fn experiment(&self) -> Option<Experiment> {
        self.file.experiment.clone()
    }

    fn assurance_observations(&self) -> Observations {
        self.file.observations.clone()
    }
}

/// Builds an [`Experiment`] one fact at a time, recording where each came from.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    e: Experiment,
}

impl Facts {
    /// An empty builder.
    pub fn new() -> Self {
        Facts::default()
    }

    fn note(&mut self, key: &str, from: &str, source: crate::provenance::Source) {
        self.e.provenance.insert(
            key.to_string(),
            crate::experiment::Origin {
                source,
                from: from.to_string(),
            },
        );
    }

    fn text(v: &str) -> Option<String> {
        let v = v.trim();
        (!v.is_empty()).then(|| v.to_string())
    }

    fn instrument(&mut self) -> &mut crate::experiment::ExperimentInstrument {
        self.e.instrument.get_or_insert_with(Default::default)
    }

    fn method(&mut self) -> &mut crate::experiment::Method {
        self.e.method.get_or_insert_with(Default::default)
    }

    fn acquisition(&mut self) -> &mut crate::experiment::Acquisition {
        self.e.acquisition.get_or_insert_with(Default::default)
    }

    fn sample(&mut self) -> &mut crate::experiment::Sample {
        self.e.sample.get_or_insert_with(Default::default)
    }

    /// Set a text field of the experiment by its path (`instrument.model`, `sample.name`,
    /// `acquisition.operator`, `method.name`, ...). Blank values and unknown paths are ignored;
    /// a field already set is kept.
    pub fn set(&mut self, path: &str, value: &str, from: &str) -> &mut Self {
        self.set_with(path, value, from, crate::provenance::Source::Inferred)
    }

    /// [`Facts::set`] with an explicit provenance source.
    pub fn set_with(
        &mut self,
        path: &str,
        value: &str,
        from: &str,
        source: crate::provenance::Source,
    ) -> &mut Self {
        let Some(v) = Self::text(value) else {
            return self;
        };
        let slot: Option<&mut Option<String>> = match path {
            "instrument.vendor" => Some(&mut self.instrument().vendor),
            "instrument.model" => Some(&mut self.instrument().model),
            "instrument.serial" => Some(&mut self.instrument().serial),
            "instrument.software" => Some(&mut self.instrument().software),
            "instrument.software_version" => Some(&mut self.instrument().software_version),
            "method.name" => Some(&mut self.method().name),
            "acquisition.started_at" => Some(&mut self.acquisition().started_at),
            "acquisition.ended_at" => Some(&mut self.acquisition().ended_at),
            "acquisition.operator" => Some(&mut self.acquisition().operator),
            "acquisition.comment" => Some(&mut self.acquisition().comment),
            "sample.id" => Some(&mut self.sample().id),
            "sample.name" => Some(&mut self.sample().name),
            "sample.well" => Some(&mut self.sample().well),
            "sample.barcode" => Some(&mut self.sample().barcode),
            _ => None,
        };
        if let Some(slot) = slot
            && slot.is_none()
        {
            *slot = Some(v);
            self.note(path, from, source);
            // an identifier names the field it came from
            if path == "sample.id" && self.sample().source_field.is_none() {
                self.sample().source_field = Some(from.to_string());
            }
        }
        self
    }

    /// The instrument kind (an OBI/CHMO term id from [`crate::vocab::TERMS`]).
    pub fn instrument_kind(&mut self, term_id: &str) -> &mut Self {
        if let Some(t) = crate::vocab::term(term_id) {
            self.instrument().kind = Some(t);
            self.note(
                "instrument.kind",
                "the file format",
                crate::provenance::Source::Inferred,
            );
        }
        self
    }

    /// The method's technique (a term id from [`crate::vocab::TERMS`]).
    pub fn technique(&mut self, term_id: &str, from: &str) -> &mut Self {
        if let Some(t) = crate::vocab::term(term_id) {
            self.method().technique = Some(t);
            self.note(
                "method.technique",
                from,
                crate::provenance::Source::Inferred,
            );
        }
        self
    }

    /// A method parameter with a unit from [`crate::vocab::UNITS`] (non-finite values are
    /// ignored; a parameter already set is kept).
    pub fn number(&mut self, name: &str, value: f64, unit: &str, from: &str) -> &mut Self {
        if value.is_finite() && !self.method().parameters.contains_key(name) {
            self.method().parameters.insert(
                name.to_string(),
                crate::experiment::Quantity::number(value, unit),
            );
            self.note(
                &format!("method.parameters.{name}"),
                from,
                crate::provenance::Source::Inferred,
            );
        }
        self
    }

    /// A method parameter without a unit (a count, a word).
    pub fn plain(&mut self, name: &str, value: impl Into<Value>, from: &str) -> &mut Self {
        let value = value.into();
        let blank = match &value {
            Value::Null => true,
            Value::String(s) => s.trim().is_empty(),
            Value::Number(n) => n.as_f64().is_some_and(|f| !f.is_finite()),
            _ => false,
        };
        if !blank && !self.method().parameters.contains_key(name) {
            self.method()
                .parameters
                .insert(name.to_string(), crate::experiment::Quantity::plain(value));
            self.note(
                &format!("method.parameters.{name}"),
                from,
                crate::provenance::Source::Inferred,
            );
        }
        self
    }

    /// The acquisition's duration in seconds.
    pub fn duration(&mut self, seconds: f64, from: &str) -> &mut Self {
        if seconds.is_finite() && seconds >= 0.0 {
            self.acquisition().duration_s = Some(seconds);
            self.note(
                "acquisition.duration_s",
                from,
                crate::provenance::Source::Inferred,
            );
        }
        self
    }

    /// Describe one kind of data block.
    pub fn measurement(
        &mut self,
        kind: crate::experiment::MeasurementKind,
        indices: Vec<u32>,
        what: impl Into<String>,
        technique: Option<&str>,
    ) -> &mut Self {
        let k = self.e.measurements.len();
        self.e.measurements.push(crate::experiment::Measurement {
            kind,
            indices,
            what: what.into(),
            technique: technique.and_then(crate::vocab::term),
            terms: Vec::new(),
            parameters: BTreeMap::new(),
        });
        self.note(
            &format!("measurements[{k}]"),
            "the reader's data layout (traces, tables)",
            crate::provenance::Source::Inferred,
        );
        self
    }

    /// A note about the fields.
    pub fn remark(&mut self, text: impl Into<String>) -> &mut Self {
        self.e.notes.push(text.into());
        self
    }

    /// The experiment.
    pub fn build(self) -> Experiment {
        self.e
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DetectConfidence;
    use crate::provenance::Confidence;

    fn descriptor() -> FormatDescriptor {
        FormatDescriptor {
            id: "test-series".into(),
            name: "test".into(),
            vendor: "none".into(),
            extensions: vec![],
            family: "test".into(),
            can_read: true,
            can_write: false,
            confidence: Confidence::Low,
            known_gaps: vec![],
        }
    }

    #[test]
    fn serves_traces_tables_and_sweeps() {
        let _ = DetectConfidence::Definite;
        let mut f = SeriesFile::default();
        f.traces.push(SeriesTrace {
            name: "scan".into(),
            channels: vec![
                SeriesChannel::new("two_theta", Some("°"), "float64"),
                SeriesChannel::new("counts", None, "float64"),
            ],
            sweeps: vec![
                vec![vec![1.0, 2.0, 3.0], vec![10.0, 20.0, 30.0]],
                vec![vec![1.0, 2.0], vec![f64::NAN, 5.0]],
            ],
            ..SeriesTrace::default()
        });
        f.tables.push(SeriesTable {
            name: "steps".into(),
            columns: vec![
                SeriesColumn::numbers("step", None, vec![1.0, 2.0, 3.0]),
                SeriesColumn::texts("kind", &["CC", "CV", "CC"]),
            ],
            extra: BTreeMap::new(),
        });
        let mut ds = SeriesDataset::new(descriptor(), PathBuf::from("x"), 10, f);
        let info = ds.info().unwrap();
        assert_eq!(info.traces[0].sample_count, 3);
        assert_eq!(info.traces[0].sweep_count, 2);
        assert_eq!(info.traces[0].extra["sweep_sample_counts"], json!([3, 2]));
        assert_eq!(
            info.tables[0].columns[1].extra["categories"],
            json!(["CC", "CV"])
        );
        let t = ds.read_trace(0, 0, 1, 5).unwrap();
        assert_eq!(t.channels[1], vec![20.0, 30.0]);
        assert!(ds.read_trace(0, 2, 0, 5).is_err());
        assert!(ds.read_trace(1, 0, 0, 5).is_err());
        let tab = ds.read_table(0, 1, 10).unwrap();
        assert_eq!(tab.columns[1], vec![1.0, 0.0]);
        let r = ds.check().unwrap();
        assert!(r.ok);
        assert!(r.findings.iter().any(|f| f.code == "non_finite_values"));
        assert!(ds.read_plane(0, PlaneIndex::default()).is_err());
    }
}
