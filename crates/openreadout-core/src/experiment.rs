//! The experiment model: what was measured, on what, how, by whom and when, in the same shape
//! for a confocal stack, an LC-MS run and a plate read (`book/src/guides/metadata.md`).
//!
//! [`derive()`] is a pure function of the normalized [`FileInfo`] (and the reader's provenance
//! map): it maps normalized fields and the shared `extra` keys onto [`Experiment`]. A reader that
//! knows experiment facts the normalized model cannot carry (a Bruker NMR `title` file, an mzML
//! `sampleList`) overrides [`crate::reader::Dataset::experiment`];
//! [`of_dataset`] merges both. Every value carries its origin in [`Experiment::provenance`]:
//! the path of the field it came from and that field's [`Source`].
//!
//! Terms come from the curated table in [`crate::vocab`]; units carry UCUM codes.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::ops::{Deref, DerefMut};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{FileInfo, ImageInfo, SpectraInfo, TableInfo, TraceInfo};
use crate::provenance::{ProvenanceMap, Source};
use crate::reader::Dataset;
use crate::vocab::{self, Term};

/// A value with an optional unit: `{"value": 19, "unit": "min", "ucum": "min"}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Quantity {
    /// A number, a string, or a list of them.
    pub value: Value,
    /// Unit as written for people (`µm`, `°C`), when the value has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// UCUM code of `unit` (`um`, `Cel`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ucum: Option<String>,
}

impl Quantity {
    /// A number in `unit` (a symbol from [`vocab::UNITS`]).
    pub fn number(value: f64, unit: &str) -> Self {
        Quantity {
            value: finite(value),
            unit: Some(unit.to_string()),
            ucum: vocab::ucum(unit).map(str::to_string),
        }
    }
    /// A value without a unit (a count, a name, a list).
    pub fn plain(value: impl Into<Value>) -> Self {
        Quantity {
            value: value.into(),
            unit: None,
            ucum: None,
        }
    }
}

fn finite(v: f64) -> Value {
    if v.is_finite() {
        if v.fract() == 0.0 && v.abs() < 1e15 {
            Value::from(v as i64)
        } else {
            Value::from(v)
        }
    } else {
        Value::Null
    }
}

/// What was measured: the sample's identity as the file records it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Sample {
    /// The sample's identifier, from the field named in `source_field`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// A descriptive sample name when the file records one besides the id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Plate well (`B2`, `A01`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub well: Option<String>,
    /// Plate or tube barcode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub barcode: Option<String>,
    /// Position in the autosampler sequence or tray (vial `D:35`, `2:A,8`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence_position: Option<String>,
    /// Path of the field `id` came from (`spectra[0].extra.sample_name`, `tables[0].extra.specimen`,
    /// `pdata/1/title`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_field: Option<String>,
}

/// The instrument that made the measurement.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExperimentInstrument {
    /// Manufacturer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    /// Model (microscope stand, mass spectrometer, cytometer, plate reader, detector module).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Serial number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    /// Acquisition software.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub software: Option<String>,
    /// Version of the acquisition software.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub software_version: Option<String>,
    /// Kind of instrument (OBI term: `microscope`, `mass spectrometer`, `flow cytometer`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Term>,
}

/// How the measurement was made.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Method {
    /// Name of the acquisition method, protocol or experiment as the software saved it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The technique (CHMO, FBbi or OBI term: `liquid chromatography-mass spectrometry`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub technique: Option<Term>,
    /// The kind of assay (OBI term).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assay: Option<Term>,
    /// Method settings, keyed by our own snake_case names (`nucleus`, `polarity`, `z_step`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, Quantity>,
}

/// When and by whom.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Acquisition {
    /// ISO-8601 start (same zone rules as the rest of the model).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// ISO-8601 end, when recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    /// Operator or user name as recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator: Option<String>,
    /// Length of the acquisition (run length, recording length, time-lapse span) in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_s: Option<f64>,
    /// Free-text comment saved with the acquisition (ABF file comment, FCS `$COM`, Thermo
    /// sequence comment, ANDI sample comments), as recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// ISO-8601 time the file was last saved or exported, when that is the only time it
    /// records (a SoftMax Pro text export's `Date Last Saved`). It follows the measurement and
    /// is not its start: `started_at` stays empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_at: Option<String>,
}

/// Which part of the file a measurement describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum MeasurementKind {
    /// `images[]`.
    Image,
    /// `tables[]`.
    Table,
    /// `traces[]`.
    Trace,
    /// `spectra[]`.
    Spectra,
}

impl MeasurementKind {
    /// The `FileInfo` array the indices point into.
    pub fn array(self) -> &'static str {
        match self {
            MeasurementKind::Image => "images",
            MeasurementKind::Table => "tables",
            MeasurementKind::Trace => "traces",
            MeasurementKind::Spectra => "spectra",
        }
    }
}

/// One thing that was measured, in scientific words.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Measurement {
    /// Which array of the file it describes.
    pub kind: MeasurementKind,
    /// Indices into that array (images, tables, traces or runs) this entry covers.
    pub indices: Vec<u32>,
    /// What was measured, e.g. `fluorescence, 3 channels (DAPI, GFP, mCherry), 21 z-slices` or
    /// `LC-MS, negative mode, 2,031 scans, MS1`.
    pub what: String,
    /// The technique (CHMO, FBbi, OBI or PSI-MS term).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub technique: Option<Term>,
    /// Further terms: polarity, spectrum types, detectors, acquisition modes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub terms: Vec<Term>,
    /// Counts and settings with units (`scans`, `wells`, `wavelength`, `z_slices`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, Quantity>,
}

/// Where one experiment value came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Origin {
    /// How the meaning of the source field is known (the reader's provenance for that field;
    /// `inferred` for our own mapping and derived text).
    pub source: Source,
    /// The field it was taken from: a path into the `info` JSON (`spectra[0].extra.vial`), or a
    /// vendor file inside the dataset (`pdata/1/title`).
    pub from: String,
}

/// The experiment a file records: sample, instrument, method, acquisition and measurements.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Experiment {
    /// What was measured on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<Sample>,
    /// What it was measured with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instrument: Option<ExperimentInstrument>,
    /// How.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<Method>,
    /// When and by whom.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acquisition: Option<Acquisition>,
    /// What was measured, one entry per kind of data block.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measurements: Vec<Measurement>,
    /// Things to know when reading the fields above (several samples in one file, ...).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// Origin of every value, keyed by its path in this object (`sample.id`,
    /// `method.parameters.nucleus`, `measurements[0]`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provenance: BTreeMap<String, Origin>,
}

impl Experiment {
    /// True when nothing is known.
    pub fn is_empty(&self) -> bool {
        self.sample.is_none()
            && self.instrument.is_none()
            && self.method.is_none()
            && self.acquisition.is_none()
            && self.measurements.is_empty()
    }

    /// Every term in the experiment (instrument kind, method technique and assay, measurement
    /// techniques and terms).
    pub fn terms(&self) -> Vec<&Term> {
        let mut v: Vec<&Term> = Vec::new();
        if let Some(t) = self.instrument.as_ref().and_then(|i| i.kind.as_ref()) {
            v.push(t);
        }
        if let Some(m) = &self.method {
            v.extend(m.technique.iter().chain(m.assay.iter()));
        }
        for m in &self.measurements {
            v.extend(m.technique.iter().chain(m.terms.iter()));
        }
        v
    }

    /// Every quantity with its path (`method.parameters.x`, `measurements[0].parameters.y`).
    pub fn quantities(&self) -> Vec<(String, &Quantity)> {
        let mut v = Vec::new();
        if let Some(m) = &self.method {
            for (k, q) in &m.parameters {
                v.push((format!("method.parameters.{k}"), q));
            }
        }
        for (i, m) in self.measurements.iter().enumerate() {
            for (k, q) in &m.parameters {
                v.push((format!("measurements[{i}].parameters.{k}"), q));
            }
        }
        v
    }

    /// Paths of every value present (the keys `provenance` must cover).
    pub fn value_paths(&self) -> Vec<String> {
        let mut v = Vec::new();
        let mut add = |cond: bool, p: &str| {
            if cond {
                v.push(p.to_string());
            }
        };
        if let Some(s) = &self.sample {
            add(s.id.is_some(), "sample.id");
            add(s.name.is_some(), "sample.name");
            add(s.well.is_some(), "sample.well");
            add(s.barcode.is_some(), "sample.barcode");
            add(s.sequence_position.is_some(), "sample.sequence_position");
        }
        if let Some(i) = &self.instrument {
            add(i.vendor.is_some(), "instrument.vendor");
            add(i.model.is_some(), "instrument.model");
            add(i.serial.is_some(), "instrument.serial");
            add(i.software.is_some(), "instrument.software");
            add(i.software_version.is_some(), "instrument.software_version");
            add(i.kind.is_some(), "instrument.kind");
        }
        if let Some(m) = &self.method {
            add(m.name.is_some(), "method.name");
            add(m.technique.is_some(), "method.technique");
            add(m.assay.is_some(), "method.assay");
            for k in m.parameters.keys() {
                v.push(format!("method.parameters.{k}"));
            }
        }
        if let Some(a) = &self.acquisition {
            let mut add = |cond: bool, p: &str| {
                if cond {
                    v.push(p.to_string());
                }
            };
            add(a.started_at.is_some(), "acquisition.started_at");
            add(a.ended_at.is_some(), "acquisition.ended_at");
            add(a.operator.is_some(), "acquisition.operator");
            add(a.duration_s.is_some(), "acquisition.duration_s");
            add(a.comment.is_some(), "acquisition.comment");
        }
        for i in 0..self.measurements.len() {
            v.push(format!("measurements[{i}]"));
        }
        v
    }

    /// The origin covering `path`: its own entry, else the nearest ancestor's
    /// (`measurements[0]` covers `measurements[0].parameters.scans`).
    pub fn origin_of(&self, path: &str) -> Option<&Origin> {
        let mut p = path;
        loop {
            if let Some(o) = self.provenance.get(p) {
                return Some(o);
            }
            p = &p[..p.rfind('.')?];
        }
    }

    /// Overlay `other` (a reader's own facts) onto `self`: every value `other` has replaces
    /// the derived one, with its provenance.
    pub fn merge(&mut self, other: Experiment) {
        macro_rules! overlay {
            ($dst:expr, $src:expr, [$($f:ident),*]) => {
                $( if $src.$f.is_some() { $dst.$f = $src.$f; } )*
            };
        }
        if let Some(s) = other.sample {
            let d = self.sample.get_or_insert_with(Sample::default);
            overlay!(
                d,
                s,
                [id, name, well, barcode, sequence_position, source_field]
            );
        }
        if let Some(i) = other.instrument {
            let d = self
                .instrument
                .get_or_insert_with(ExperimentInstrument::default);
            overlay!(
                d,
                i,
                [vendor, model, serial, software, software_version, kind]
            );
        }
        if let Some(m) = other.method {
            let d = self.method.get_or_insert_with(Method::default);
            overlay!(d, m, [name, technique, assay]);
            d.parameters.extend(m.parameters);
        }
        if let Some(a) = other.acquisition {
            let d = self.acquisition.get_or_insert_with(Acquisition::default);
            overlay!(d, a, [started_at, ended_at, operator, duration_s, comment]);
        }
        if !other.measurements.is_empty() {
            // Reader-provided measurements replace derived ones of the same kind.
            let kinds: BTreeSet<&str> = other.measurements.iter().map(|m| m.kind.array()).collect();
            let old = std::mem::take(&mut self.measurements);
            let mut kept: Vec<Measurement> = Vec::new();
            let mut remap: Vec<(String, String)> = Vec::new();
            for (i, m) in old.into_iter().enumerate() {
                if !kinds.contains(m.kind.array()) {
                    remap.push((
                        format!("measurements[{i}]"),
                        format!("measurements[{}]", kept.len()),
                    ));
                    kept.push(m);
                }
            }
            let prov = std::mem::take(&mut self.provenance);
            for (k, o) in prov {
                if let Some(rest) = k.strip_prefix("measurements[") {
                    let head = format!("measurements[{}", &rest[..rest.find(']').unwrap_or(0)]);
                    let head = format!("{head}]");
                    if let Some((_, new)) = remap.iter().find(|(old, _)| *old == head) {
                        self.provenance.insert(k.replacen(&head, new, 1), o);
                    }
                } else {
                    self.provenance.insert(k, o);
                }
            }
            let base = kept.len();
            for (k, o) in &other.provenance {
                if let Some(rest) = k.strip_prefix("measurements[")
                    && let Some(end) = rest.find(']')
                    && let Ok(n) = rest[..end].parse::<usize>()
                {
                    self.provenance.insert(
                        format!("measurements[{}]{}", base + n, &rest[end + 1..]),
                        o.clone(),
                    );
                }
            }
            kept.extend(other.measurements);
            self.measurements = kept;
        }
        for (k, o) in other.provenance {
            if !k.starts_with("measurements[") {
                self.provenance.insert(k, o);
            }
        }
        self.notes.extend(other.notes);
        self.prune();
    }

    /// Drop empty sections and provenance entries whose value is absent.
    fn prune(&mut self) {
        if self
            .sample
            .as_ref()
            .is_some_and(|s| *s == Sample::default())
        {
            self.sample = None;
        }
        if self
            .instrument
            .as_ref()
            .is_some_and(|s| *s == ExperimentInstrument::default())
        {
            self.instrument = None;
        }
        if self
            .method
            .as_ref()
            .is_some_and(|s| *s == Method::default())
        {
            self.method = None;
        }
        if self
            .acquisition
            .as_ref()
            .is_some_and(|s| *s == Acquisition::default())
        {
            self.acquisition = None;
        }
        let present: BTreeSet<String> = self.value_paths().into_iter().collect();
        self.provenance.retain(|k, _| present.contains(k));
    }
}

/// What `info` prints: the normalized [`FileInfo`] plus the derived [`Experiment`] under the
/// additive `experiment` key. Dereferences to the `FileInfo`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InfoOutput {
    /// The reader's normalized summary.
    #[serde(flatten)]
    pub file: FileInfo,
    /// Sample, instrument, method, acquisition and measurements, each value with its origin
    /// (`book/src/guides/metadata.md`). Absent when nothing is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experiment: Option<Experiment>,
    /// Present when the file is not finished: still being written (`in_progress`, with the
    /// number of complete planes) or stopped before the end (`interrupted`). See `book/src/guides/lab-shares.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acquisition: Option<crate::live::Acquisition>,
    /// Multi-well plates (high-content screening): the plate id and type, rows and columns,
    /// the imaged wells with the images (fields) of each, and how many planes the copy on disk
    /// is missing (`docs/formats/hcs.md`). Absent for other files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plate: Option<crate::plate::PlateSummary>,
    /// Set when `images` lists only the first few images ([`InfoOutput::cap_images`]: screening
    /// plates by default, whose field images are all alike, or `--max-images N`): how many
    /// images the file holds. `plate.wells[].images` still indexes every field, the image
    /// commands take any index, and `--max-images 0` (MCP `max_images: 0`) lists them all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images_total: Option<u64>,
    /// Whether this file lies inside what its reader has been validated on: the variant
    /// fingerprint with the corpus evidence for each feature, structures not decoded, values
    /// assumed, vendor calibrations, and the outputs `--strict` refuses (`docs/assurance.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assurance: Option<crate::assurance::Assurance>,
}

/// Images `info` lists in full for a screening plate unless asked for more.
pub const PLATE_IMAGES_LISTED: usize = 16;

impl InfoOutput {
    /// `info` plus the experiment derived from it and the dataset's own facts.
    pub fn new(ds: &dyn Dataset, file: FileInfo) -> Self {
        let e = of_dataset(ds, &file);
        let assurance = crate::assurance::assess_dataset(ds, &file);
        let out = InfoOutput {
            experiment: (!e.is_empty()).then_some(e),
            plate: crate::plate::plate_layout(ds, &file),
            file,
            acquisition: None,
            images_total: None,
            assurance: Some(assurance),
        };
        if ds.is_strict() {
            out.withhold_unvalidated()
        } else {
            out
        }
    }

    /// `--strict`: null the fields the assurance withholds (`strict_withholds`) and say so in
    /// `notes`. The rest of the output is unchanged.
    #[must_use]
    pub fn withhold_unvalidated(self) -> Self {
        let Some(a) = self.assurance.clone() else {
            return self;
        };
        if a.strict_withholds.is_empty() {
            return self;
        }
        let Ok(mut v) = serde_json::to_value(&self) else {
            return self;
        };
        let n = crate::assurance::withhold(&mut v, &a);
        if n == 0 {
            return self;
        }
        match serde_json::from_value::<InfoOutput>(v) {
            Ok(mut out) => {
                out.file.notes.push(format!(
                    "--strict withheld {n} value(s) (null here) that no independent reader has confirmed: {}; rerun without --strict to see them",
                    a.strict_withholds
                        .iter()
                        .map(|w| w.field.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                out
            }
            Err(_) => self,
        }
    }

    /// Keep the answer small: list at most `max` images (`Some(0)`: all), by default
    /// [`PLATE_IMAGES_LISTED`] for a screening plate (one image per field of view, all alike)
    /// and every image otherwise. What was left out is said in `images_total` and `notes`.
    pub fn cap_images(&mut self, max: Option<usize>) {
        let limit = match max {
            Some(0) => return,
            Some(m) => m,
            None if self.plate.is_some() => PLATE_IMAGES_LISTED,
            None => return,
        };
        let n = self.file.images.len();
        if n <= limit {
            return;
        }
        self.file.images.truncate(limit);
        self.images_total = Some(n as u64);
        self.file.notes.push(format!(
            "images lists the first {limit} of {n} images{}; `--max-images 0` (MCP max_images: 0) lists all, and every image index works with stats, planes, export and preview",
            if self.plate.is_some() {
                " (one per field of view, all with the same size and channels; plate.wells[].images indexes every field)"
            } else {
                ""
            }
        ));
    }
    /// `info` plus the experiment derived from it alone (no dataset hook, no provenance map).
    pub fn from_info(file: FileInfo) -> Self {
        let e = derive(&file, &ProvenanceMap::new());
        let assurance = crate::assurance::assess(
            &file,
            crate::assurance::Observations::default(),
            &ProvenanceMap::new(),
        );
        InfoOutput {
            experiment: (!e.is_empty()).then_some(e),
            plate: crate::plate::layout_from_images(&file),
            file,
            acquisition: None,
            images_total: None,
            assurance: Some(assurance),
        }
    }
}

impl Deref for InfoOutput {
    type Target = FileInfo;
    fn deref(&self) -> &FileInfo {
        &self.file
    }
}

impl DerefMut for InfoOutput {
    fn deref_mut(&mut self) -> &mut FileInfo {
        &mut self.file
    }
}

/// The experiment of an open dataset: [`derive()`] from `info` and the reader's provenance, then
/// the reader's own facts ([`Dataset::experiment`]) on top.
pub fn of_dataset(ds: &dyn Dataset, info: &FileInfo) -> Experiment {
    let mut e = derive(info, &ds.provenance());
    if let Some(own) = ds.experiment() {
        e.merge(own);
    }
    e
}

// ======================================================================================
// derivation
// ======================================================================================

/// Values that mean "not recorded".
const PLACEHOLDERS: &[&str] = &[
    "", "na", "n/a", "none", "null", "unknown", "-", "-1", "0", "default", "untitled", "<>",
    "<user>", "user", "?", "not set",
];

fn is_placeholder(s: &str) -> bool {
    let t = s.trim().to_ascii_lowercase();
    PLACEHOLDERS.contains(&t.as_str()) || t.chars().all(|c| c == 'x' || c == 'X')
}

/// Formats that come from one vendor, whose name is the instrument's manufacturer.
const SINGLE_VENDOR: &[&str] = &[
    "czi",
    "lif",
    "nd2",
    "thermo-raw",
    "bruker-tdf",
    "agilent-masshunter",
    "sciex-wiff",
    "oir",
    "vsi",
    "abf",
    "atf",
    "neuralynx",
    "blackrock",
    "intan",
    "plexon",
    "chemstation",
    "openlab-cds",
    "waters-raw",
    "shimadzu",
    "bruker-nmr",
    "varian-nmr",
    "jeol-jdf",
    "magritek-spinsolve",
    "zvi",
    "ims",
    "dcimg",
];

struct Ctx<'a> {
    info: &'a FileInfo,
    json: Value,
    prov: BTreeMap<String, Source>,
    e: Experiment,
}

/// `images[0].channels[1].name` → `images[].channels[].name`; `[key=value]` selectors → `[]`.
fn normalize_path(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    let mut in_sel = false;
    for c in p.chars() {
        match c {
            '[' => {
                in_sel = true;
                out.push('[');
            }
            ']' => {
                in_sel = false;
                out.push(']');
            }
            _ if in_sel => {}
            _ => out.push(c),
        }
    }
    out
}

/// Resolve `a.b[0].c` in a JSON value. Keys may contain spaces.
fn lookup<'v>(root: &'v Value, path: &str) -> Option<&'v Value> {
    let mut cur = root;
    for seg in path.split('.') {
        let (name, mut rest) = seg.split_at(seg.find('[').unwrap_or(seg.len()));
        if !name.is_empty() {
            cur = cur.get(name)?;
        }
        while let Some(r) = rest.strip_prefix('[') {
            let end = r.find(']')?;
            cur = cur.get(r[..end].parse::<usize>().ok()?)?;
            rest = &r[end + 1..];
        }
    }
    Some(cur)
}

fn text_of(v: &Value) -> Option<String> {
    let s = match v {
        Value::String(s) => s.split_whitespace().collect::<Vec<_>>().join(" "),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    (!is_placeholder(&s)).then_some(s)
}

impl<'a> Ctx<'a> {
    fn new(info: &'a FileInfo, provenance: &ProvenanceMap) -> Self {
        Ctx {
            info,
            json: serde_json::to_value(info).unwrap_or(Value::Null),
            prov: provenance
                .iter()
                .map(|(k, s)| (normalize_path(k), *s))
                .collect(),
            e: Experiment::default(),
        }
    }

    fn family(&self) -> &str {
        self.info.format.family.as_str()
    }
    fn format(&self) -> &str {
        self.info.format.id.as_str()
    }

    /// The reader's source for a field (its own entry or the nearest ancestor's).
    fn source_of(&self, path: &str) -> Source {
        let mut p = normalize_path(path);
        loop {
            if let Some(s) = self.prov.get(&p) {
                return *s;
            }
            match p.rfind('.') {
                Some(i) => p.truncate(i),
                None => return Source::Inferred,
            }
        }
    }

    fn origin(&self, from: &str) -> Origin {
        Origin {
            source: self.source_of(from),
            from: from.to_string(),
        }
    }

    fn record(&mut self, key: &str, from: &str) {
        let o = self.origin(from);
        self.e.provenance.insert(key.to_string(), o);
    }

    fn record_inferred(&mut self, key: &str, from: &str) {
        self.e.provenance.insert(
            key.to_string(),
            Origin {
                source: Source::Inferred,
                from: from.to_string(),
            },
        );
    }

    fn get(&self, path: &str) -> Option<&Value> {
        self.resolve(path).map(|(v, _)| v)
    }

    /// The value at `path` and the path it was actually found at. A file that stores its
    /// signals only as tables (a Waters MRM run with unevenly spaced scans) has no
    /// `traces[0]`; its header fields are then read from `tables[0].extra`.
    fn resolve(&self, path: &str) -> Option<(&Value, String)> {
        if let Some(v) = lookup(&self.json, path).filter(|v| !v.is_null()) {
            return Some((v, path.to_string()));
        }
        let rest = path.strip_prefix("traces[0].extra.")?;
        if lookup(&self.json, "traces[0]").is_some() {
            return None;
        }
        let alt = format!("tables[0].extra.{rest}");
        lookup(&self.json, &alt)
            .filter(|v| !v.is_null())
            .map(|v| (v, alt))
    }

    fn text(&self, path: &str) -> Option<String> {
        self.get(path).and_then(text_of)
    }

    fn num(&self, path: &str) -> Option<f64> {
        self.get(path)
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite())
    }

    /// First present, non-placeholder text among `paths`, with its path.
    fn first_text(&self, paths: &[&str]) -> Option<(String, String)> {
        paths.iter().find_map(|p| {
            self.resolve(p)
                .and_then(|(v, at)| text_of(v).map(|s| (s, at)))
        })
    }

    /// First present text among `paths` that is not a software default name.
    fn first_sample_text(&self, paths: &[&str]) -> Option<(String, String)> {
        paths.iter().find_map(|p| {
            self.resolve(p)
                .and_then(|(v, at)| text_of(v).filter(|s| !is_generic_name(s)).map(|s| (s, at)))
        })
    }

    /// Put a method parameter taken from `from`.
    fn param(&mut self, name: &str, q: Quantity, from: &str) {
        if q.value.is_null() {
            return;
        }
        self.e
            .method
            .get_or_insert_with(Method::default)
            .parameters
            .insert(name.to_string(), q);
        self.record(&format!("method.parameters.{name}"), from);
    }

    fn param_text(&mut self, name: &str, path: &str) {
        if let Some(v) = self.text(path) {
            self.param(name, Quantity::plain(v), path);
        }
    }

    fn param_num(&mut self, name: &str, path: &str, unit: &str) {
        if let Some(v) = self.num(path) {
            self.param(name, Quantity::number(v, unit), path);
        }
    }
}

/// Derive the experiment from `info`. `provenance` is the reader's map (keys are paths into
/// `info`); it gives each value's [`Source`]. Pure: never touches the file.
pub fn derive(info: &FileInfo, provenance: &ProvenanceMap) -> Experiment {
    let mut c = Ctx::new(info, provenance);
    sample(&mut c);
    instrument(&mut c);
    acquisition(&mut c);
    measurements(&mut c);
    method(&mut c);
    c.e.prune();
    c.e
}

// ---------------------------------------------------------------- sample

/// Generic names acquisition software gives positions, scenes and series.
const GENERIC_NAMES: &[&str] = &[
    "position",
    "pos",
    "point",
    "pointname",
    "scene",
    "series",
    "image",
    "img",
    "tilescan",
    "scanregion",
    "region",
    "currentposition",
    "p",
    "s",
    "xy",
    "field",
    "fov",
    "stage",
    "location",
    "mark",
    "markandfind",
    "snapshot",
    "lambda",
    "frame",
    "untitled",
    "default",
    "experiment",
    "project",
    "sample",
    "tile",
    "labelimage",
    "label",
    "macro",
    "overview",
    "thumbnail",
    "preview",
    "ch",
    "channel",
    "z",
    "t",
    "zstack",
    "timelapse",
    "roi",
    "area",
    "spot",
    "plate",
    "block",
    "data",
    "fid",
    "ser",
    "tic",
    "bpc",
    "test",
    "testspec",
    "diff",
    "peaks",
    "func",
    "msd",
    "specimen",
    "multifile",
];

const PROCESSING_WORDS: &[&str] = &[
    "crop",
    "resize",
    "merg",
    "projection",
    "mip",
    "maxip",
    "copy",
    "processed",
    "deconvol",
    "stitch",
    "zoom",
    "mmstack",
    "default",
];

const FILE_EXTENSIONS: &[&str] = &[
    ".czi", ".lif", ".nd2", ".tif", ".tiff", ".btf", ".tf2", ".tf8", ".jdx", ".dx", ".fcs", ".lmd",
    ".mqd", ".raw", ".d", ".xlef", ".oir", ".vsi", ".ims", ".zvi", ".pic", ".ser", ".dm3", ".dm4",
    ".emd", ".mrc", ".svs", ".ndpi", ".qptiff", ".dcimg", ".stk", ".nd",
];

/// A software default rather than a sample's own name (`Plate 1`, `Specimen_001`,
/// `Default Patient ID`, `test`).
fn is_generic_name(v: &str) -> bool {
    let lower = v.to_ascii_lowercase();
    let letters: String = lower.chars().filter(char::is_ascii_alphabetic).collect();
    // `Default Patient ID`, `default_sample`: a name that starts with the word (a method's
    // `_default_0.3cycle` inside an acquired name is not a default name)
    GENERIC_NAMES.contains(&letters.as_str()) || lower.trim_start().starts_with("default")
}

/// Does a name look like a sample identifier rather than a software default (`Position 1`,
/// `Series011`, `TileScan_002_Merging`, `multi-channel.ome.tiff`)? Only the last `/` component
/// counts.
pub fn looks_like_sample_id(name: &str) -> bool {
    let last = name.rsplit(['/', '\\']).next().unwrap_or(name).trim();
    if last.len() < 2
        || last.len() > 48
        || last.contains(" #")
        || last.contains(',')
        || is_placeholder(last)
    {
        return false;
    }
    let lower = last.to_ascii_lowercase();
    if FILE_EXTENSIONS.iter().any(|e| lower.ends_with(e)) || lower.contains(".ome.") {
        return false;
    }
    if last.split_whitespace().count() > 3 || !last.chars().any(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    let letters: String = lower.chars().filter(char::is_ascii_alphabetic).collect();
    if GENERIC_NAMES.contains(&letters.as_str()) {
        return false;
    }
    !PROCESSING_WORDS.iter().any(|w| lower.contains(w))
}

fn sample(c: &mut Ctx<'_>) {
    let mut s = Sample::default();
    let mut id: Option<(String, String)> = None;
    let mut name: Option<(String, String)> = None;
    match c.family() {
        "mass-spectrometry" | "chromatography" => {
            id = c.first_sample_text(&[
                "spectra[0].extra.sample_id",
                "traces[0].extra.sample_id",
                "spectra[0].extra.sample_name",
                "traces[0].extra.sample_name",
            ]);
            // MassLynx keeps a free-text `Sample Description` beside the acquired name
            name = if c.format() == "waters-raw" {
                c.first_text(&[
                    "spectra[0].extra.description",
                    "traces[0].extra.description",
                ])
            } else {
                None
            }
            .or_else(|| {
                c.first_text(&[
                    "spectra[0].extra.sample_name",
                    "traces[0].extra.sample_name",
                ])
            });
            if let Some((v, p)) = c.first_text(&[
                "spectra[0].extra.vial",
                "traces[0].extra.vial",
                "traces[0].extra.autosampler_position",
            ]) {
                s.sequence_position = Some(v);
                c.record("sample.sequence_position", &p);
            }
        }
        "flow-cytometry" => {
            let tube = fcs_vendor_keyword(
                c,
                &[
                    "TUBE NAME",
                    "TUBENAME",
                    "TBNM",
                    "SAMPLE ID",
                    "SAMPLEID",
                    "SAMPLE NAME",
                ],
            );
            id = c
                .first_sample_text(&["tables[0].extra.specimen"])
                .or(tube.filter(|(v, _)| !is_generic_name(v)))
                .or_else(|| {
                    c.first_sample_text(&["tables[0].extra.well_id", "tables[0].extra.source"])
                });
            if let Some((v, p)) = c.first_text(&["tables[0].extra.well_id"]) {
                s.well = Some(v);
                c.record("sample.well", &p);
            }
            name = c
                .first_text(&["tables[0].extra.cells"])
                .filter(|(v, _)| id.as_ref().is_none_or(|(i, _)| i != v));
        }
        "plate-reader" => {
            if let Some((v, p)) = c.first_text(&["tables[0].extra.barcode"]) {
                s.barcode = Some(v.clone());
                c.record("sample.barcode", &p);
                id = Some((v, p));
            } else {
                id = c
                    .first_text(&["tables[0].extra.plate", "tables[0].name"])
                    .filter(|(v, _)| looks_like_sample_id(v));
            }
        }
        "spectroscopy" | "nmr" => {
            if let Some((v, p)) = c.first_text(&["traces[0].extra.sample_description"]) {
                name = Some((v, p));
            }
            id = c
                .first_text(&["traces[0].extra.title"])
                .filter(|(v, _)| looks_like_sample_id(v));
        }
        // Scene, position and series names only in the formats whose users name them.
        "microscopy" if matches!(c.format(), "czi" | "nd2" | "lif") => {
            let wells: BTreeSet<String> = (0..c.info.images.len())
                .filter_map(|i| c.text(&format!("images[{i}].extra.scene.well.name")))
                .collect();
            if wells.len() == 1 {
                let w = wells.into_iter().next().unwrap_or_default();
                let p = (0..c.info.images.len())
                    .map(|i| format!("images[{i}].extra.scene.well.name"))
                    .find(|p| c.text(p).is_some())
                    .unwrap_or_default();
                s.well = Some(w.clone());
                c.record("sample.well", &p);
                id = Some((w, p));
            } else if wells.len() > 1 {
                c.e.notes.push(format!(
                    "The images cover {} plate wells ({}); `images[].extra.scene.well` says which image is which.",
                    wells.len(),
                    wells.iter().take(8).cloned().collect::<Vec<_>>().join(", ")
                ));
            }
            if id.is_none() {
                let names: BTreeSet<String> = c
                    .info
                    .images
                    .iter()
                    .filter_map(|i| i.name.as_deref())
                    .map(|n| n.rsplit(['/', '\\']).next().unwrap_or(n).trim().to_string())
                    .collect();
                if names.len() == 1
                    && let Some(n) = names.into_iter().next()
                    && looks_like_sample_id(&n)
                {
                    let i = c
                        .info
                        .images
                        .iter()
                        .position(|i| i.name.is_some())
                        .unwrap_or(0);
                    id = Some((n, format!("images[{i}].name")));
                }
            }
        }
        _ => {}
    }
    if let Some((v, p)) = id {
        s.id = Some(v);
        s.source_field = Some(p.clone());
        c.record("sample.id", &p);
    }
    if let Some((v, p)) = name
        && s.id.as_deref() != Some(v.as_str())
    {
        s.name = Some(v);
        c.record("sample.name", &p);
    }
    c.e.sample = Some(s);
}

/// A keyword from an FCS file's vendor keyword groups (`extra.vendor_keywords`), by name.
fn fcs_vendor_keyword(c: &Ctx<'_>, names: &[&str]) -> Option<(String, String)> {
    let groups = c.get("tables[0].extra.vendor_keywords")?.as_object()?;
    for want in names {
        for (g, kv) in groups {
            if let Some(obj) = kv.as_object() {
                for (k, v) in obj {
                    if k.eq_ignore_ascii_case(want)
                        && let Some(t) = text_of(v)
                    {
                        return Some((t, format!("tables[0].extra.vendor_keywords.{g}.{k}")));
                    }
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------- instrument

/// A configured instrument or module name rather than a model: ChemStation's default
/// `Instrument 1` (cut to `Instrumen` in MS headers) and module instances named after their
/// signal files (`DAD1`, `MWD1`).
fn is_instance_name(v: &str) -> bool {
    let t = v.trim();
    let lower = t.to_ascii_lowercase();
    let letters: String = lower
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .collect();
    let rest = &lower[letters.len()..];
    let instrument = ("instrument".starts_with(letters.as_str()) && letters.len() >= 9)
        && rest.trim().chars().all(|c| c.is_ascii_digit());
    let module = matches!(
        letters.as_str(),
        "dad" | "mwd" | "vwd" | "fld" | "rid" | "fid" | "tcd"
    ) && !rest.is_empty()
        && rest.chars().all(|c| c.is_ascii_digit());
    instrument || module
}

/// ChemStation's generic GC module tag at 0x9BC of version-179/181 GC signal files (`GCI`),
/// the same in every such file whatever the instrument (docs/provenance/chemstation.md):
/// not a model.
fn is_generic_module_tag(v: &str) -> bool {
    v.trim().eq_ignore_ascii_case("GCI")
}

fn instrument(c: &mut Ctx<'_>) {
    let mut i = ExperimentInstrument::default();
    let first_image = |f: &str| {
        (0..c.info.images.len().min(8))
            .map(|n| format!("images[{n}].instrument.{f}"))
            .collect::<Vec<_>>()
    };
    let paths = |field: &str, extra: &[&str]| -> Vec<String> {
        let mut v = first_image(field);
        v.push(format!("spectra[0].instrument.{field}"));
        v.extend(extra.iter().map(|s| (*s).to_string()));
        v
    };
    let vendor_p = paths(
        "manufacturer",
        &[
            "tables[0].extra.manufacturer",
            "tables[0].extra.instrument.manufacturer",
            "tables[0].extra.platform.vendor",
        ],
    );
    let mut model_extra: Vec<&str> = vec!["tables[0].extra.instrument.model"];
    match c.format() {
        // `extra.instrument` is the module or system name in these formats.
        // ChemStation's `Result.xml` export names the instrument (`Agilent 7890A`)
        "chemstation" => {
            model_extra.push("traces[0].extra.result_modules[0].name");
            model_extra.push("traces[0].extra.instrument");
        }
        "waters-raw" | "openlab-cds" => {
            model_extra.push("traces[0].extra.instrument");
        }
        "bruker-nmr" | "varian-nmr" | "jeol-jdf" | "magritek-spinsolve" => {
            model_extra.push("traces[0].extra.instrument");
        }
        "jcamp-dx" => model_extra.push("traces[0].extra.spectrometer"),
        "spikeglx" => model_extra.push("traces[0].extra.probe_type"),
        _ => {}
    }
    model_extra.push("images[0].extra.microscope");
    let model_p = paths("model", &model_extra);
    let serial_p = vec![
        "spectra[0].extra.instrument_serial".to_string(),
        "traces[0].extra.result_modules[0].serial".to_string(),
        "spectra[0].extra.method_file.ms_serial".to_string(),
        "traces[0].extra.method_file.ms_serial".to_string(),
        "tables[0].extra.instrument.serial_number".to_string(),
        "traces[0].extra.probe_serial_number".to_string(),
        "traces[0].extra.instrument_serial".to_string(),
    ];
    let software_p = paths(
        "software",
        &[
            "tables[0].extra.software",
            "traces[0].extra.software",
            "traces[0].extra.creator",
            "traces[0].extra.application",
            "tables[0].extra.application",
        ],
    );
    let version_p = paths(
        "software_version",
        &[
            "tables[0].extra.software_version",
            "traces[0].extra.software_version",
            "traces[0].extra.creator_version",
            "traces[0].extra.application_version",
            "tables[0].extra.application_version",
            "traces[0].extra.app_version",
        ],
    );
    // The name the method report gives the configured instrument (`GCMSD_1`) is not a model.
    let configured: Option<String> = c
        .first_text(&[
            "spectra[0].extra.method_file.instrument_name",
            "traces[0].extra.method_file.instrument_name",
        ])
        .map(|(v, _)| v);
    let pick = |c: &Ctx<'_>, ps: &[String], field: &str| {
        ps.iter().find_map(|p| {
            c.text(p)
                .filter(|v| {
                    field != "model"
                        || !(is_instance_name(v)
                            || configured.as_deref() == Some(v.as_str())
                            || (c.format() == "chemstation" && is_generic_module_tag(v)))
                })
                .map(|v| (v, p.clone()))
        })
    };
    let sets: [(&str, &Vec<String>); 5] = [
        ("vendor", &vendor_p),
        ("model", &model_p),
        ("serial", &serial_p),
        ("software", &software_p),
        ("software_version", &version_p),
    ];
    for (field, ps) in sets {
        if let Some((v, p)) = pick(c, ps, field) {
            // "spect" is Bruker's placeholder instrument name.
            if field == "model" && v.eq_ignore_ascii_case("spect") {
                continue;
            }
            match field {
                "vendor" => i.vendor = Some(v),
                "model" => i.model = Some(v),
                "serial" => i.serial = Some(v),
                "software" => i.software = Some(v),
                _ => i.software_version = Some(v),
            }
            c.record(&format!("instrument.{field}"), &p);
        }
    }
    if i.vendor.is_none() && SINGLE_VENDOR.contains(&c.format()) {
        i.vendor = Some(c.info.format.vendor.clone());
        c.record_inferred("instrument.vendor", "format.vendor");
    }
    if i.software.is_none() && c.format() == "spikeglx" && i.software_version.is_some() {
        i.software = Some("SpikeGLX".into());
        c.record_inferred("instrument.software", "format.id");
    }
    let confocal = c.info.images.iter().any(|im| {
        im.channels.iter().any(|ch| {
            ch.acquisition_mode.as_deref().is_some_and(|m| {
                m.to_ascii_lowercase().contains("confocal")
                    || m.to_ascii_lowercase().contains("laser scanning")
            })
        })
    });
    let kind = match c.family() {
        "microscopy" if confocal => "OBI:0001079",
        "microscopy" => "OBI:0400169",
        "electron-microscopy" => "OBI:0000990",
        "flow-cytometry" => "OBI:0400044",
        "mass-spectrometry" => "OBI:0000049",
        "plate-reader" => "OBI:0001058",
        "nmr" => "OBI:0000566",
        "chromatography" if c.info.spectra.is_empty() => {
            if separation(c) == Some("GC") {
                "OBI:0000485"
            } else {
                "OBI:0001057"
            }
        }
        "chromatography" => "OBI:0000049",
        "spectroscopy" => match jcamp_kind(c) {
            Some(JcampKind::Nmr) => "OBI:0000566",
            Some(JcampKind::Mass) => "OBI:0000049",
            Some(_) => "OBI:0400115",
            None => "",
        },
        _ => "",
    };
    i.kind = vocab::term(kind);
    if i.kind.is_some() {
        c.record_inferred("instrument.kind", "format.family");
    }
    c.e.instrument = Some(i);
}

// ---------------------------------------------------------------- acquisition

fn is_iso(s: &str) -> bool {
    crate::time::iso8601_to_unix(s).is_some()
}

fn acquisition(c: &mut Ctx<'_>) {
    let mut a = Acquisition::default();
    // Earliest image start.
    let mut best: Option<(f64, String, String)> = None;
    for (n, im) in c.info.images.iter().enumerate() {
        if let Some(t) = im.acquired_at.as_deref()
            && let Some(u) = crate::time::iso8601_to_unix(t)
            && best.as_ref().is_none_or(|(b, _, _)| u < *b)
        {
            best = Some((u, t.to_string(), format!("images[{n}].acquired_at")));
        }
    }
    let start = best.map(|(_, t, p)| (t, p)).or_else(|| {
        c.first_text(&[
            "tables[0].extra.acquisition_start",
            "tables[0].extra.acquired_at",
            "spectra[0].extra.acquired_at",
            "traces[0].extra.acquired_at",
            "traces[0].extra.created_at",
            "traces[0].extra.recorded_at",
            "tables[0].extra.recorded_at",
            "traces[0].extra.opened_at",
            "tables[0].extra.opened_at",
        ])
        .filter(|(v, _)| is_iso(v))
    });
    // JCAMP-DX `##LONG DATE= YYYY/MM/DD HH:MM:SS` (local time, no zone).
    let start = start.or_else(|| {
        (c.family() == "spectroscopy")
            .then(|| c.text("traces[0].extra.long_date"))
            .flatten()
            .and_then(|v| jcamp_long_date(&v))
            .map(|v| (v, "traces[0].extra.long_date".to_string()))
    });
    if let Some((v, p)) = start {
        a.started_at = Some(v);
        c.record("acquisition.started_at", &p);
    }
    if a.started_at.is_none()
        && let Some((v, p)) = c
            .first_text(&["tables[0].extra.saved_at"])
            .filter(|(v, _)| is_iso(v))
    {
        a.saved_at = Some(v);
        c.record("acquisition.saved_at", &p);
    }
    if let Some((v, p)) = c
        .first_text(&[
            "tables[0].extra.acquisition_end",
            "traces[0].extra.closed_at",
            "tables[0].extra.closed_at",
        ])
        .filter(|(v, _)| is_iso(v))
    {
        a.ended_at = Some(v);
        c.record("acquisition.ended_at", &p);
    }
    let mut op_paths = vec![
        "spectra[0].extra.operator",
        "traces[0].extra.operator",
        "tables[0].extra.operator",
        "tables[0].extra.experimenter",
        "images[0].extra.experimenter",
        "images[0].extra.experimenter.user_name",
        "images[0].extra.experimenter.last_name",
    ];
    if c.family() == "nmr" {
        op_paths.push("traces[0].extra.owner");
    }
    if let Some((v, p)) = c.first_text(&op_paths) {
        a.operator = Some(v);
        c.record("acquisition.operator", &p);
    }
    if let Some((v, p)) = c.first_text(&[
        "traces[0].extra.comment",
        "tables[0].extra.comment",
        "spectra[0].extra.comment",
        "traces[0].extra.sample_comments",
    ]) {
        a.comment = Some(v);
        c.record("acquisition.comment", &p);
    }
    // Duration: end − start, else the length of the data itself.
    let span = match (&a.started_at, &a.ended_at) {
        (Some(s), Some(e)) => match (
            crate::time::iso8601_to_unix(s),
            crate::time::iso8601_to_unix(e),
        ) {
            (Some(s), Some(e)) if e >= s => Some((
                e - s,
                "acquisition.started_at, acquisition.ended_at".to_string(),
            )),
            _ => None,
        },
        _ => None,
    };
    let span = span.or_else(|| data_length(c));
    if let Some((d, from)) = span
        && d.is_finite()
        && round_to(d, 6) > 0.0
    {
        a.duration_s = Some(if d >= 1.0 {
            round_to(d, 3)
        } else {
            round_to(d, 6)
        });
        c.record_inferred("acquisition.duration_s", &from);
    }
    c.e.acquisition = Some(a);
}

/// `2019/07/27 11:58:49.000` → `2019-07-27T11:58:49.000` (JCAMP-DX 5 `##LONG DATE`); anything
/// else (the two-digit `##DATE` forms, whose day/month order varies) gives `None`.
fn jcamp_long_date(v: &str) -> Option<String> {
    let (date, time) = v.trim().split_once(' ')?;
    let d: Vec<&str> = date.split('/').collect();
    let ok = d.len() == 3
        && d[0].len() == 4
        && d.iter()
            .all(|x| !x.is_empty() && x.chars().all(|ch| ch.is_ascii_digit()));
    if !ok {
        return None;
    }
    let iso = format!("{}-{:0>2}-{:0>2}T{}", d[0], d[1], d[2], time.trim());
    is_iso(&iso).then_some(iso)
}

fn round_to(v: f64, decimals: i32) -> f64 {
    let f = 10f64.powi(decimals);
    (v * f).round() / f
}

/// Length of the recorded data in seconds, with the fields it came from.
fn data_length(c: &Ctx<'_>) -> Option<(f64, String)> {
    if let Some([_, b]) = c.info.spectra.first().and_then(|s| s.rt_range_s) {
        return Some((b, "spectra[0].rt_range_s".into()));
    }
    if c.family() == "chromatography" {
        if let Some(v) = c.num("traces[0].extra.run_time_s") {
            return Some((v, "traces[0].extra.run_time_s".into()));
        }
        if let Some(v) = c.num("traces[0].extra.x_end_min") {
            return Some((v * 60.0, "traces[0].extra.x_end_min".into()));
        }
    }
    if c.family() == "electrophysiology"
        && let Some(t) = c.info.traces.iter().find(|t| t.sample_rate_hz > 0.0)
    {
        let n = recorded_s(t);
        return Some((
            n,
            format!(
                "traces[{}].sample_count, sample_rate_hz, sweep_count",
                t.index
            ),
        ));
    }
    let im = c
        .info
        .images
        .iter()
        .filter(|i| i.size_t > 1)
        .find_map(|i| i.time_increment_s.map(|dt| (i, dt)))?;
    Some((
        im.1 * f64::from(im.0.size_t - 1),
        format!("images[{}].time_increment_s, size_t", im.0.index),
    ))
}

/// Recorded seconds of a trace: the continuous stretch, or sweeps × sweep length.
fn recorded_s(t: &TraceInfo) -> f64 {
    t.sample_count as f64 * f64::from(t.sweep_count.max(1)) / t.sample_rate_hz
}

// ---------------------------------------------------------------- measurements

fn push_measurement(c: &mut Ctx<'_>, m: Measurement, from: String) {
    let i = c.e.measurements.len();
    c.e.measurements.push(m);
    c.record_inferred(&format!("measurements[{i}]"), &from);
}

fn measurements(c: &mut Ctx<'_>) {
    // Images, grouped by identical description.
    let mut groups: Vec<Measurement> = Vec::new();
    for im in &c.info.images {
        let m = image_measurement(c, im);
        match groups
            .iter_mut()
            .find(|g| g.what == m.what && g.technique == m.technique)
        {
            Some(g) => g.indices.push(im.index),
            None => groups.push(m),
        }
    }
    for m in groups {
        let from = indices_path("images", &m.indices);
        push_measurement(c, m, from);
    }
    let tables: Vec<Measurement> = c
        .info
        .tables
        .iter()
        .map(|t| table_measurement(c, t))
        .collect();
    for m in tables {
        let from = indices_path("tables", &m.indices);
        push_measurement(c, m, from);
    }
    let mut groups: Vec<Measurement> = Vec::new();
    for t in &c.info.traces {
        let m = trace_measurement(c, t);
        match groups.iter_mut().find(|g| g.what == m.what) {
            Some(g) => g.indices.push(t.index),
            None => groups.push(m),
        }
    }
    for m in groups {
        let from = indices_path("traces", &m.indices);
        push_measurement(c, m, from);
    }
    let spectra: Vec<Measurement> = c
        .info
        .spectra
        .iter()
        .map(|s| spectra_measurement(c, s))
        .collect();
    for m in spectra {
        let from = indices_path("spectra", &m.indices);
        push_measurement(c, m, from);
    }
}

fn indices_path(array: &str, idx: &[u32]) -> String {
    match idx {
        [one] => format!("{array}[{one}]"),
        _ if idx.len() <= 6 => format!(
            "{array}[{}]",
            idx.iter().map(u32::to_string).collect::<Vec<_>>().join(",")
        ),
        _ => format!("{array}[{}..{}]", idx[0], idx[idx.len() - 1]),
    }
}

/// `1234567` → `1,234,567`.
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn plural(n: u64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{} {many}", thousands(n))
    }
}

/// Three significant digits, no trailing zeros.
fn sig(v: f64) -> String {
    if v == 0.0 || !v.is_finite() {
        return "0".into();
    }
    let mag = v.abs().log10().floor() as i32;
    let decimals = (2 - mag).clamp(0, 6) as usize;
    let s = format!("{v:.decimals$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

fn duration_words(s: f64) -> String {
    if s < 1.0 {
        format!("{} ms", sig(s * 1e3))
    } else if s < 120.0 {
        format!("{} s", sig(s))
    } else if s < 7200.0 {
        format!("{} min", sig(s / 60.0))
    } else {
        format!("{} h", sig(s / 3600.0))
    }
}

fn rate_words(hz: f64) -> String {
    if hz >= 1e6 {
        format!("{} MHz", sig(hz / 1e6))
    } else if hz >= 1e3 {
        format!("{} kHz", sig(hz / 1e3))
    } else {
        format!("{} Hz", sig(hz))
    }
}

/// Imaging modality of an image: (words, FBbi term id).
/// Channel-name fragments that name a fluorescent dye or protein.
const DYE_WORDS: &[&str] = &[
    "dapi", "hoechst", "gfp", "rfp", "yfp", "cfp", "bfp", "mcherry", "tdtomato", "fitc", "tritc",
    "cy2", "cy3", "cy5", "cy7", "alexa", "texas", "mcitrine", "mscarlet", "venus", "dio", "dii",
];

/// A length in µm in words: `0.172 µm`, `2.73 nm`, `0.3 Å`.
fn length_words(um: f64) -> String {
    if um < 1e-3 {
        format!("{} Å", sig(um * 1e4))
    } else if um < 0.1 {
        format!("{} nm", sig(um * 1e3))
    } else {
        format!("{} µm", sig(um))
    }
}

fn modality(c: &Ctx<'_>, im: &ImageInfo) -> (&'static str, &'static str) {
    if c.family() == "electron-microscopy" {
        let text = [
            im.extra.get("mode").and_then(Value::as_str),
            im.instrument.as_ref().and_then(|i| i.detector.as_deref()),
            im.name.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
        if text.contains("stem")
            || text.contains("haadf")
            || text.contains("df4")
            || text.contains("bf-s")
        {
            return ("scanning transmission electron microscopy", "FBbi:00000380");
        }
        if text.contains("tem") {
            return ("transmission electron microscopy", "FBbi:00000258");
        }
        return ("electron microscopy", "FBbi:00000256");
    }
    let modes: String = im
        .channels
        .iter()
        .filter_map(|ch| ch.acquisition_mode.as_deref())
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    let names: Vec<String> = im
        .channels
        .iter()
        .filter_map(|ch| ch.name.as_deref())
        .map(str::to_ascii_lowercase)
        .collect();
    let all_named = |words: &[&str]| {
        !names.is_empty() && names.iter().all(|n| words.iter().any(|w| n.contains(w)))
    };
    if im.samples_per_pixel >= 3 || all_named(&["bright", "tl ", "transmitted"]) {
        return ("brightfield", "FBbi:00000243");
    }
    if all_named(&["dic"]) {
        return ("DIC", "FBbi:00000245");
    }
    if all_named(&["phase", "ph3", "ph2", "ph1"]) {
        return ("phase contrast", "FBbi:00000247");
    }
    let fluor = im.channels.iter().any(|ch| {
        ch.fluorophore.is_some() || ch.excitation_nm.is_some() || ch.emission_nm.is_some()
    }) || modes.contains("fluor")
        || names
            .iter()
            .any(|n| DYE_WORDS.iter().any(|d| n.contains(d)));
    if im.extra.contains_key("flim") {
        return ("fluorescence lifetime imaging", "FBbi:00000368");
    }
    if modes.contains("spinning") {
        return ("spinning-disk confocal fluorescence", "FBbi:00000253");
    }
    if modes.contains("multiphoton")
        || modes.contains("two-photon")
        || modes.contains("2-photon")
        || modes.contains("multi-photon")
    {
        return ("multiphoton fluorescence", "FBbi:00000255");
    }
    if modes.contains("confocal")
        || modes.contains("laser scanning")
        || modes.contains("laserscanning")
    {
        return ("confocal fluorescence", "FBbi:00000251");
    }
    if modes.contains("light sheet") || modes.contains("lightsheet") || modes.contains("spim") {
        return ("light-sheet fluorescence", "FBbi:00000369");
    }
    if modes.contains("tirf") || modes.contains("total internal") {
        return ("TIRF fluorescence", "FBbi:00000617");
    }
    if fluor {
        return ("fluorescence", "FBbi:00000246");
    }
    if modes.contains("dic") || modes.contains("differential interference") {
        return ("DIC", "FBbi:00000245");
    }
    if modes.contains("phase") {
        return ("phase contrast", "FBbi:00000247");
    }
    if modes.contains("bright") || modes.contains("transmitted") {
        return ("brightfield", "FBbi:00000243");
    }
    ("light microscopy", "FBbi:00000345")
}

fn image_measurement(c: &Ctx<'_>, im: &ImageInfo) -> Measurement {
    let (words, term) = modality(c, im);
    let mut what = words.to_string();
    let mut params = BTreeMap::new();
    let placeholder_channels = im.size_c == 1
        && im.channels.iter().all(|ch| {
            ch.fluorophore.is_none() && (ch.name.is_none() || c.family() == "electron-microscopy")
        });
    if im.samples_per_pixel >= 3 {
        what.push_str(", colour (RGB)");
    } else if placeholder_channels {
    } else if !im.channels.is_empty() {
        let names: Vec<String> = im
            .channels
            .iter()
            .map(|ch| {
                ch.fluorophore
                    .clone()
                    .or_else(|| ch.name.clone())
                    .unwrap_or_else(|| format!("channel {}", ch.index))
            })
            .collect();
        let shown: Vec<String> = names.iter().take(6).cloned().collect();
        let more = if names.len() > 6 { ", …" } else { "" };
        let _ = write!(
            what,
            ", {} ({}{more})",
            plural(u64::from(im.size_c), "channel", "channels"),
            shown.join(", ")
        );
        params.insert("channels".to_string(), Quantity::plain(Value::from(names)));
    } else if im.size_c > 1 {
        let _ = write!(
            what,
            ", {}",
            plural(u64::from(im.size_c), "channel", "channels")
        );
    }
    if im.size_z > 1 {
        let _ = write!(what, ", {} z-slices", im.size_z);
        params.insert("z_slices".into(), Quantity::plain(im.size_z));
        if let Some(z) = im.physical_size.z {
            let _ = write!(what, " {} apart", length_words(z));
            params.insert("z_step".into(), Quantity::number(z, "µm"));
        }
    }
    if im.size_t > 1 {
        let _ = write!(what, ", {} time points", im.size_t);
        params.insert("time_points".into(), Quantity::plain(im.size_t));
        if let Some(dt) = im.time_increment_s.filter(|d| *d > 0.0) {
            let _ = write!(what, " every {}", duration_words(dt));
            params.insert("time_interval".into(), Quantity::number(dt, "s"));
        }
    }
    if let Some(m) = &im.mosaic {
        let _ = write!(
            what,
            ", mosaic of {}",
            plural(u64::from(m.tile_count), "tile", "tiles")
        );
    }
    let _ = write!(what, ", {} × {} px", im.size_x, im.size_y);
    if let Some(x) = im.physical_size.x {
        let _ = write!(what, " at {}/px", length_words(x));
        params.insert("pixel_size".into(), Quantity::number(x, "µm"));
    }
    let mut terms = Vec::new();
    if im.size_t > 1 && c.family() == "microscopy" {
        terms.extend(vocab::term("FBbi:00000249"));
    }
    Measurement {
        kind: MeasurementKind::Image,
        indices: vec![im.index],
        what,
        technique: vocab::term(term),
        terms,
        parameters: params,
    }
}

fn table_measurement(c: &Ctx<'_>, t: &TableInfo) -> Measurement {
    let mut params = BTreeMap::new();
    let mut terms = Vec::new();
    let mut technique = None;
    let what = match c.family() {
        "flow-cytometry" => {
            technique = vocab::term("CHMO:0000061");
            let names: Vec<String> = t
                .columns
                .iter()
                .map(|col| {
                    col.label
                        .clone()
                        .filter(|l| !l.trim().is_empty())
                        .unwrap_or_else(|| col.name.clone())
                })
                .collect();
            params.insert("events".into(), Quantity::plain(t.row_count));
            params.insert(
                "parameters".into(),
                Quantity::plain(Value::from(names.clone())),
            );
            let shown: Vec<String> = names.iter().take(8).cloned().collect();
            format!(
                "flow cytometry, {} × {} ({}{})",
                plural(t.row_count, "event", "events"),
                plural(t.columns.len() as u64, "parameter", "parameters"),
                shown.join(", "),
                if names.len() > 8 { ", …" } else { "" }
            )
        }
        "plate-reader" => plate_what(t, &mut technique, &mut terms, &mut params),
        _ => {
            let kind = if c.family() == "chromatography" && t.name.as_deref() == Some("peaks") {
                "peak table".to_string()
            } else if let Some(n) = &t.name {
                format!("table “{n}”")
            } else {
                "table".to_string()
            };
            params.insert("rows".into(), Quantity::plain(t.row_count));
            format!(
                "{kind}, {} × {}",
                plural(t.row_count, "row", "rows"),
                plural(t.columns.len() as u64, "column", "columns")
            )
        }
    };
    Measurement {
        kind: MeasurementKind::Table,
        indices: vec![t.index],
        what,
        technique,
        terms,
        parameters: params,
    }
}

fn plate_what(
    t: &TableInfo,
    technique: &mut Option<Term>,
    terms: &mut Vec<Term>,
    params: &mut BTreeMap<String, Quantity>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    let reads = t
        .extra
        .get("reads")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut wavelengths: Vec<f64> = Vec::new();
    for r in reads.iter().filter(|r| {
        !r.get("calculated")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }) {
        let mode = r.get("mode").and_then(Value::as_str).unwrap_or("unknown");
        let f = |k: &str| r.get(k).and_then(Value::as_f64);
        let (text, term) = match mode {
            "absorbance" => (
                f("wavelength_nm").map_or_else(
                    || "absorbance".to_string(),
                    |w| {
                        wavelengths.push(w);
                        format!("absorbance at {} nm", sig(w))
                    },
                ),
                "CHMO:0000292",
            ),
            "fluorescence" => (
                match (f("excitation_nm"), f("emission_nm")) {
                    (Some(x), Some(m)) => {
                        format!("fluorescence (ex {} nm / em {} nm)", sig(x), sig(m))
                    }
                    _ => "fluorescence".to_string(),
                },
                "CHMO:0000060",
            ),
            "luminescence" => ("luminescence".to_string(), "CHMO:0000057"),
            other => (other.to_string(), ""),
        };
        if !parts.contains(&text) {
            parts.push(text);
        }
        if let Some(t) = vocab::term(term)
            && !terms.contains(&t)
        {
            terms.push(t);
        }
    }
    if technique.is_none() && !terms.is_empty() {
        *technique = Some(terms.remove(0));
    }
    if wavelengths.len() == 1 {
        params.insert("wavelength".into(), Quantity::number(wavelengths[0], "nm"));
    }
    let mut what = if parts.is_empty() {
        "plate read".to_string()
    } else {
        parts.join("; ")
    };
    let wells = t.extra.get("wells_measured").and_then(Value::as_u64);
    let plate = t.extra.get("plate_well_count").and_then(Value::as_u64);
    match (wells, plate) {
        (Some(w), Some(p)) if w < p => {
            let _ = write!(what, ", {} of {} wells", thousands(w), thousands(p));
        }
        (Some(w), _) | (None, Some(w)) => {
            let _ = write!(what, ", {}", plural(w, "well", "wells"));
        }
        _ => {}
    }
    if let Some(w) = wells.or(plate) {
        params.insert("wells".into(), Quantity::plain(w));
    }
    match t.extra.get("read_type").and_then(Value::as_str) {
        Some("kinetic") => {
            what.push_str(", kinetic");
            if let Some(n) = t
                .extra
                .get("kinetic_points")
                .or_else(|| t.extra.get("time_points"))
                .and_then(|v| v.as_u64().or_else(|| v.as_array().map(|a| a.len() as u64)))
            {
                let _ = write!(what, " ({n} reads per well)");
                params.insert("reads_per_well".into(), Quantity::plain(n));
            }
        }
        Some("spectrum") => what.push_str(", spectral scan"),
        _ => {}
    }
    if let Some(tc) = t.extra.get("temperature_c").and_then(Value::as_f64)
        && tc > 0.0
    {
        let _ = write!(what, ", {} °C", sig(tc));
        params.insert("temperature".into(), Quantity::number(tc, "°C"));
    }
    what
}

/// Chromatography separation (`GC`/`LC`) as the file names it.
fn separation(c: &Ctx<'_>) -> Option<&'static str> {
    let s = c
        .first_text(&[
            "traces[0].extra.separation",
            "spectra[0].extra.separation",
            "traces[1].extra.separation",
        ])
        .map(|(v, _)| v.to_ascii_lowercase())?;
    if s == "gc" || s.contains("gas") {
        Some("GC")
    } else if s == "lc" || s.contains("liquid") {
        Some("LC")
    } else {
        None
    }
}

/// Detector kind of a chromatogram: (words, CHMO term).
fn detector_of(t: &TraceInfo) -> Option<(&'static str, &'static str)> {
    let text = [
        t.extra.get("detector").and_then(Value::as_str),
        t.extra.get("signal").and_then(Value::as_str),
        t.name.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ")
    .to_ascii_uppercase();
    let words: Vec<&str> = text.split(|c: char| !c.is_ascii_alphanumeric()).collect();
    DETECTORS
        .iter()
        .find(|(keys, ..)| keys.iter().any(|k| words.iter().any(|w| w.starts_with(k))))
        .map(|&(_, words, term)| (words, term))
}

/// Chromatography detectors: the words that name them (a word starting with any), how
/// `what` says it, and the CHMO or MS term. The first match wins.
const DETECTORS: &[(&[&str], &str, &str)] = &[
    (&["FID"], "FID", "CHMO:0001719"),
    (&["TCD"], "TCD", "CHMO:0001731"),
    (&["DAD", "PDA"], "diode-array (DAD)", "CHMO:0001728"),
    (&["RID"], "refractive index", "CHMO:0001730"),
    (&["FLD"], "fluorescence", "CHMO:0000060"),
    (&["MWD", "VWD", "UV"], "UV-visible", "CHMO:0000292"),
    (&["TIC"], "total ion current", "MS:1000235"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JcampKind {
    Nmr,
    Infrared,
    UvVis,
    Mass,
    Raman,
}

fn jcamp_kind_of(data_type: &str) -> Option<JcampKind> {
    let d = data_type.to_ascii_uppercase();
    if d.contains("NMR") {
        Some(JcampKind::Nmr)
    } else if d.contains("INFRARED") || d.starts_with("IR") {
        Some(JcampKind::Infrared)
    } else if d.contains("UV") || d.contains("VIS") {
        Some(JcampKind::UvVis)
    } else if d.contains("MASS") {
        Some(JcampKind::Mass)
    } else if d.contains("RAMAN") {
        Some(JcampKind::Raman)
    } else {
        None
    }
}

fn jcamp_kind(c: &Ctx<'_>) -> Option<JcampKind> {
    c.text("traces[0].extra.data_type")
        .and_then(|d| jcamp_kind_of(&d))
}

fn nucleus_of(t: &TraceInfo) -> Option<String> {
    t.extra
        .get("nucleus")
        .and_then(Value::as_str)
        .map(|n| n.trim_start_matches('^').trim().to_string())
        .filter(|n| !n.is_empty() && !n.eq_ignore_ascii_case("off"))
}

fn nmr_technique(nucleus: Option<&str>, two_d: bool) -> &'static str {
    if two_d {
        return "CHMO:0000598";
    }
    match nucleus {
        Some("1H") => "CHMO:0000593",
        Some("13C") => "CHMO:0000595",
        _ => "CHMO:0000591",
    }
}

fn trace_measurement(c: &Ctx<'_>, t: &TraceInfo) -> Measurement {
    let mut m = Measurement {
        kind: MeasurementKind::Trace,
        indices: vec![t.index],
        what: String::new(),
        technique: None,
        terms: Vec::new(),
        parameters: BTreeMap::new(),
    };
    let name = t
        .name
        .clone()
        .unwrap_or_else(|| format!("trace {}", t.index));
    m.what = match c.family() {
        "electrophysiology" => ephys_trace(c, t, &mut m),
        "nmr" => nmr_trace(t, &mut m),
        "spectroscopy" => spectrum_trace(t, &mut m),
        "chromatography" | "mass-spectrometry" => chromatogram_trace(c, t, &name, &mut m),
        _ => {
            let mut s = format!(
                "signal “{name}”, {}",
                plural(t.channels.len() as u64, "channel", "channels")
            );
            if t.sample_rate_hz > 0.0 {
                let _ = write!(s, " at {}", rate_words(t.sample_rate_hz));
            }
            s
        }
    };
    m.parameters
        .insert("points".into(), Quantity::plain(t.sample_count));
    m
}

fn ephys_trace(c: &Ctx<'_>, t: &TraceInfo, m: &mut Measurement) -> String {
    m.technique = vocab::term(if matches!(c.format(), "abf" | "atf") {
        "OBI:0002176"
    } else {
        "OBI:0000454"
    });
    let ch: Vec<String> = t
        .channels
        .iter()
        .map(|ch| match &ch.unit {
            Some(u) if !u.is_empty() => format!("{} [{u}]", ch.name),
            _ => ch.name.clone(),
        })
        .collect();
    let shown: Vec<String> = ch.iter().take(6).cloned().collect();
    let mut s = format!(
        "electrophysiology, {} ({}{})",
        plural(t.channels.len() as u64, "channel", "channels"),
        shown.join(", "),
        if ch.len() > 6 { ", …" } else { "" }
    );
    if t.sample_rate_hz > 0.0 {
        let len = t.sample_count as f64 / t.sample_rate_hz;
        if t.sweep_count > 1 {
            let _ = write!(s, ", {} sweeps × {}", t.sweep_count, duration_words(len));
        } else {
            let _ = write!(s, ", {}", duration_words(len));
        }
        let _ = write!(s, " at {}", rate_words(t.sample_rate_hz));
        m.parameters.insert(
            "sample_rate".into(),
            Quantity::number(t.sample_rate_hz, "Hz"),
        );
    }
    m.parameters
        .insert("channels".into(), Quantity::plain(t.channels.len() as u64));
    m.parameters
        .insert("sweeps".into(), Quantity::plain(t.sweep_count));
    s
}

fn nmr_trace(t: &TraceInfo, m: &mut Measurement) -> String {
    let nucleus = nucleus_of(t);
    let dims = 1 + t
        .extra
        .get("indirect_dimensions")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    m.technique = vocab::term(if dims > 2 {
        "CHMO:0000591"
    } else {
        nmr_technique(nucleus.as_deref(), dims == 2)
    });
    let kind = match t.extra.get("kind").and_then(Value::as_str) {
        Some("time_domain") => "FID (time domain)",
        _ => "processed spectrum",
    };
    let mut s = format!(
        "{}{} NMR {kind}",
        if dims > 1 {
            format!("{dims}D ")
        } else {
            String::new()
        },
        nucleus.as_deref().unwrap_or("")
    )
    .trim_start()
    .to_string();
    if let Some(f) = t
        .extra
        .get("spectrometer_frequency_mhz")
        .and_then(Value::as_f64)
    {
        let _ = write!(s, ", {} MHz", sig(f));
    }
    if let Some(n) = t.extra.get("scans").and_then(Value::as_u64) {
        let _ = write!(s, ", {}", plural(n, "scan", "scans"));
        m.parameters.insert("scans".into(), Quantity::plain(n));
    }
    if let Some(p) = t.extra.get("pulse_program").and_then(Value::as_str) {
        let _ = write!(s, ", pulse program {p}");
    }
    s
}

/// A 1-D spectrum (JCAMP-DX and the spectroscopy readers), described by its data type.
fn spectrum_trace(t: &TraceInfo, m: &mut Measurement) -> String {
    let dt = t
        .extra
        .get("data_type")
        .and_then(Value::as_str)
        .unwrap_or("spectrum");
    m.technique = jcamp_kind_of(dt).and_then(|k| {
        vocab::term(match k {
            JcampKind::Nmr => nmr_technique(nucleus_of(t).as_deref(), dt.contains("nD")),
            JcampKind::Infrared => "CHMO:0000630",
            JcampKind::UvVis => "CHMO:0000292",
            JcampKind::Mass => "CHMO:0000470",
            JcampKind::Raman => "CHMO:0000656",
        })
    });
    let mut s = dt.to_lowercase();
    if let Some(n) = nucleus_of(t) {
        let _ = write!(s, ", {n}");
    }
    if let Some(f) = t.extra.get("observe_frequency_mhz").and_then(Value::as_f64) {
        let _ = write!(s, ", {} MHz", sig(f));
    }
    let _ = write!(s, ", {}", plural(t.sample_count, "point", "points"));
    s
}

fn chromatogram_trace(c: &Ctx<'_>, t: &TraceInfo, name: &str, out: &mut Measurement) -> String {
    let det = detector_of(t);
    if let Some((_, term)) = det {
        out.terms.extend(vocab::term(term));
    }
    let is_chrom = t.extra.contains_key("irregular_sampling")
        || t.extra.contains_key("x_end_min")
        || t.extra.contains_key("run_time_s");
    out.technique = match (c.family(), separation(c)) {
        ("chromatography", Some("GC")) => vocab::term("CHMO:0001002"),
        ("chromatography", _) => vocab::term("CHMO:0001004"),
        _ => None,
    };
    let upper = name.to_ascii_uppercase();
    let mut s = if upper == "TIC" || upper.starts_with("TIC ") {
        "total ion current chromatogram".to_string()
    } else if upper == "BPC" || upper.starts_with("BPC ") {
        "base peak chromatogram".to_string()
    } else {
        let mut s = if is_chrom { "chromatogram" } else { "signal" }.to_string();
        match det {
            Some((w, _)) => {
                let _ = write!(s, ", {w} detector");
            }
            None => {
                let _ = write!(s, " “{name}”");
            }
        }
        s
    };
    if let Some(w) = t.extra.get("wavelength_nm").and_then(Value::as_f64) {
        let _ = write!(s, " at {} nm", sig(w));
        out.parameters
            .insert("wavelength".into(), Quantity::number(w, "nm"));
    }
    let end = t
        .extra
        .get("x_end_min")
        .and_then(Value::as_f64)
        .map(|min| min * 60.0)
        .or_else(|| t.extra.get("run_time_s").and_then(Value::as_f64));
    if let Some(e) = end.filter(|e| *e > 0.0) {
        let _ = write!(s, ", {}", duration_words(e));
    }
    let _ = write!(s, ", {}", plural(t.sample_count, "point", "points"));
    s
}

/// Polarities of a run: from `extra.polarities` (list) or `extra.polarity` (text).
fn polarities(s: &SpectraInfo) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    if let Some(a) = s.extra.get("polarities").and_then(Value::as_array) {
        v.extend(
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_ascii_lowercase),
        );
    }
    if let Some(p) = s.extra.get("polarity").and_then(Value::as_str) {
        v.push(p.to_ascii_lowercase());
    }
    let mut out: Vec<String> = Vec::new();
    for p in v {
        let p = if p.contains("pos") {
            "positive"
        } else if p.contains("neg") {
            "negative"
        } else {
            continue;
        };
        if !out.iter().any(|x| x == p) {
            out.push(p.to_string());
        }
    }
    out
}

/// The whole-run technique: (words, CHMO term, OBI assay).
fn ms_technique(c: &Ctx<'_>, s: &SpectraInfo) -> (String, &'static str, &'static str) {
    let tandem = s.ms_levels.iter().any(|l| *l > 1);
    let sources = ion_sources(c, s);
    let imaging = c.format() == "imzml"
        || s.extra.get("imaging").is_some_and(|v| !v.is_null())
        || (sources.iter().any(|x| x.contains("MALDI"))
            && c.format() == "thermo-raw"
            && s.scan_count > 50);
    if imaging {
        return (
            "imaging mass spectrometry".into(),
            "CHMO:0000053",
            "OBI:0003099",
        );
    }
    let devices = s
        .extra
        .get("method_summary")
        .and_then(|m| m.get("devices"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" ")
                .to_ascii_lowercase()
        })
        .unwrap_or_default();
    let gc = c.family() == "chromatography" && separation(c) == Some("GC")
        || s.extra
            .get("ionization")
            .and_then(Value::as_str)
            .is_some_and(|i| i.to_ascii_lowercase().contains("electron impact"));
    // Flow-injection methods (`…_FIAMS_…`) use the LC pump without a column.
    let method = s
        .extra
        .get("method_summary")
        .and_then(|m| m.get("instrument_method"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let flow_injection = method
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or("")
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| w.starts_with("fia"));
    let lc = !gc
        && !flow_injection
        && (devices.contains("pump")
            || devices.contains("lc")
            || c.format() == "bruker-tdf"
            || c.family() == "chromatography"
            || c.format() == "waters-raw");
    match (gc, lc, tandem) {
        (true, _, _) => ("GC-MS".into(), "CHMO:0000497", "OBI:0003110"),
        (_, true, true) => ("LC-MS/MS".into(), "CHMO:0000701", "OBI:0003097"),
        (_, true, false) => ("LC-MS".into(), "CHMO:0000524", "OBI:0003097"),
        (_, false, true) => ("tandem MS".into(), "CHMO:0000575", "OBI:0000470"),
        _ => ("MS".into(), "CHMO:0000470", "OBI:0000470"),
    }
}

fn ion_sources(c: &Ctx<'_>, s: &SpectraInfo) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    if let Some(a) = s.extra.get("ion_sources").and_then(Value::as_array) {
        v.extend(a.iter().filter_map(Value::as_str).map(str::to_string));
    }
    for k in ["ionisation", "ionization"] {
        if let Some(x) = s.extra.get(k).and_then(Value::as_str) {
            v.push(x.to_string());
        }
    }
    if let Some(cfg) = s
        .extra
        .get("instrument_configurations")
        .and_then(Value::as_array)
    {
        for comp in cfg
            .iter()
            .filter_map(|x| x.get("components").and_then(Value::as_array))
            .flatten()
        {
            if comp.get("kind").and_then(Value::as_str) == Some("source")
                && let Some(t) = comp.get("terms").and_then(Value::as_array)
                && let Some(first) = t.first().and_then(Value::as_str)
            {
                v.push(first.to_string());
            }
        }
    }
    let _ = c;
    v.into_iter()
        .map(|x| {
            let l = x.to_ascii_lowercase();
            if l == "nsi" || l.contains("nanoelectrospray") || l.contains("nanospray") {
                "nanoESI".to_string()
            } else if l == "esi" || l.contains("electrospray") {
                "ESI".to_string()
            } else if l.contains("maldi") || l.contains("matrix-assisted") {
                "MALDI".to_string()
            } else if l.contains("apci") || l.contains("chemical ionization") {
                "APCI".to_string()
            } else if l == "ei"
                || l.contains("electron impact")
                || l.contains("electron ionization")
            {
                "EI".to_string()
            } else {
                x
            }
        })
        .fold(Vec::new(), |mut acc, x| {
            if !acc.contains(&x) {
                acc.push(x);
            }
            acc
        })
}

fn analyzers(s: &SpectraInfo) -> Vec<&'static str> {
    let model = s
        .instrument
        .as_ref()
        .and_then(|i| i.model.as_deref())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut text: Vec<String> = Vec::new();
    if let Some(a) = s.extra.get("analyzers").and_then(Value::as_array) {
        text.extend(
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_ascii_lowercase),
        );
    }
    if let Some(a) = s.extra.get("mass_analyzer").and_then(Value::as_str) {
        text.push(a.to_ascii_lowercase());
    }
    if let Some(cfg) = s
        .extra
        .get("instrument_configurations")
        .and_then(Value::as_array)
    {
        for comp in cfg
            .iter()
            .filter_map(|x| x.get("components").and_then(Value::as_array))
            .flatten()
        {
            if comp.get("kind").and_then(Value::as_str) == Some("analyzer")
                && let Some(t) = comp.get("terms").and_then(Value::as_array)
            {
                text.extend(
                    t.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_ascii_lowercase),
                );
            }
        }
    }
    let mut out = Vec::new();
    let mut add = |id: &'static str| {
        if !out.contains(&id) {
            out.push(id);
        }
    };
    for t in &text {
        if t == "ftms" {
            if model.contains("orbitrap") || model.contains("exactive") {
                add("MS:1000484");
            } else {
                add("MS:1000079");
            }
        } else if t.contains("orbitrap") {
            add("MS:1000484");
        } else if t.contains("cyclotron") {
            add("MS:1000079");
        } else if t == "itms" || t.contains("ion trap") {
            add("MS:1000264");
        } else if t.contains("time-of-flight") || t == "tof" || t.contains("tofms") {
            add("MS:1000084");
        } else if t.contains("quadrupole") || t == "tqms" || t == "sqms" {
            add("MS:1000081");
        }
    }
    if out.is_empty() && model.contains("tof") {
        out.push("MS:1000084");
    }
    out
}

fn spectra_measurement(c: &Ctx<'_>, s: &SpectraInfo) -> Measurement {
    let (words, technique, _) = ms_technique(c, s);
    let mut what = words;
    let mut terms = Vec::new();
    let mut params = BTreeMap::new();
    let pol = polarities(s);
    match pol.as_slice() {
        [one] => {
            let _ = write!(what, ", {one} mode");
        }
        [_, _] => what.push_str(", positive and negative mode (switching)"),
        _ => {}
    }
    for p in &pol {
        terms.extend(vocab::term(if p == "positive" {
            "MS:1000130"
        } else {
            "MS:1000129"
        }));
    }
    if !pol.is_empty() {
        params.insert("polarity".into(), Quantity::plain(Value::from(pol.clone())));
    }
    let sources = ion_sources(c, s);
    if let Some(src) = sources.first() {
        let _ = write!(what, ", {src}");
        let id = match src.as_str() {
            "ESI" => "MS:1000073",
            "nanoESI" => "MS:1000398",
            "MALDI" => "MS:1000075",
            "APCI" => "MS:1000070",
            "EI" => "MS:1000389",
            _ => "",
        };
        terms.extend(vocab::term(id));
        params.insert("ion_source".into(), Quantity::plain(src.clone()));
    }
    let _ = write!(what, ", {}", plural(s.scan_count, "scan", "scans"));
    params.insert("scans".into(), Quantity::plain(s.scan_count));
    if !s.ms_levels.is_empty() {
        let lv: Vec<String> = s.ms_levels.iter().map(|l| format!("MS{l}")).collect();
        let _ = write!(what, ", {}", lv.join("+"));
        params.insert(
            "ms_levels".into(),
            Quantity::plain(Value::from(s.ms_levels.clone())),
        );
        if s.ms_levels.contains(&1) {
            terms.extend(vocab::term("MS:1000579"));
        }
        if s.ms_levels.iter().any(|l| *l > 1) {
            terms.extend(vocab::term("MS:1000580"));
        }
    }
    if let Some(acq) = s.extra.get("acquisition").and_then(Value::as_str) {
        let a = acq.to_ascii_uppercase();
        if a.contains("DDA") {
            terms.extend(vocab::term("MS:1003221"));
        } else if a.contains("DIA") {
            terms.extend(vocab::term("MS:1003215"));
        }
        let _ = write!(what, ", {acq}");
    }
    for a in analyzers(s) {
        terms.extend(vocab::term(a));
    }
    if let Some([a, b]) = s.rt_range_s
        && b > 0.0
    {
        if b >= 120.0 {
            let _ = write!(
                what,
                ", retention time {}–{} min",
                sig(a / 60.0),
                sig(b / 60.0)
            );
        } else {
            let _ = write!(what, ", retention time {}–{} s", sig(a), sig(b));
        }
        params.insert(
            "retention_time_range".into(),
            Quantity {
                value: Value::from(vec![finite(round_to(a, 3)), finite(round_to(b, 3))]),
                unit: Some("s".into()),
                ucum: Some("s".into()),
            },
        );
    }
    if let Some(r) = s
        .extra
        .get("mz_range")
        .or_else(|| s.extra.get("mz_acquisition_range"))
        .and_then(Value::as_array)
        && let [Some(a), Some(b)] = [
            r.first().and_then(Value::as_f64),
            r.get(1).and_then(Value::as_f64),
        ]
    {
        let _ = write!(what, ", m/z {}–{}", sig(a), sig(b));
        params.insert(
            "mz_range".into(),
            Quantity {
                value: Value::from(vec![finite(a), finite(b)]),
                unit: Some("m/z".into()),
                ucum: vocab::ucum("m/z").map(str::to_string),
            },
        );
    }
    Measurement {
        kind: MeasurementKind::Spectra,
        indices: vec![s.index],
        what,
        technique: vocab::term(technique),
        terms,
        parameters: params,
    }
}

// ---------------------------------------------------------------- method

/// `C:\Xcalibur\methods\GM\UPLC-profiling.meth` → `UPLC-profiling`.
fn method_stem(path: &str) -> String {
    let last = path.rsplit(['\\', '/']).next().unwrap_or(path).trim();
    let lower = last.to_ascii_lowercase();
    for ext in [
        ".meth", ".m", ".exp", ".prt", ".mth", ".lcm", ".skax", ".xpt", ".wsp",
    ] {
        if lower.ends_with(ext) && last.len() > ext.len() {
            return last[..last.len() - ext.len()].to_string();
        }
    }
    last.to_string()
}

fn method(c: &mut Ctx<'_>) {
    let family = c.family().to_string();
    let mut m = Method::default();
    let (technique, assay, technique_from) = method_terms(c, &family);
    if let Some(t) = technique {
        m.technique = Some(t);
        c.record_inferred("method.technique", &technique_from);
    }
    if let Some(a) = vocab::term(assay) {
        m.assay = Some(a);
        c.record_inferred("method.assay", "format.family");
    }
    c.e.method = Some(m);
    if let Some((v, p)) = method_name(c, &family) {
        let stem = method_stem(&v);
        if let Some(mm) = c.e.method.as_mut() {
            mm.name = Some(stem);
        }
        c.record("method.name", &p);
    }
    method_params(c, &family);
}

/// OBI assay of a family whose assay does not depend on the measurements.
const FAMILY_ASSAYS: &[(&str, &str)] = &[
    ("microscopy", "OBI:0002119"),
    ("electron-microscopy", "OBI:0001631"),
    ("flow-cytometry", "OBI:0000916"),
    ("nmr", "OBI:0000623"),
];

/// The method's technique, its assay (empty when unknown) and the field the technique came
/// from: the first measurement's, made whole-run for mass spectrometry.
fn method_terms(c: &Ctx<'_>, family: &str) -> (Option<Term>, &'static str, String) {
    if let Some(s) = c.info.spectra.first() {
        let (_, t, a) = ms_technique(c, s);
        return (vocab::term(t), a, "spectra[0]".to_string());
    }
    // NMR data sets can list a sampling-schedule table (Bruker `nuslist`, JEOL axis lists)
    // before their traces: take the first measurement that has a technique.
    let k = if family == "nmr" {
        c.e.measurements
            .iter()
            .position(|x| x.technique.is_some())
            .unwrap_or(0)
    } else {
        0
    };
    let first = c.e.measurements.get(k).and_then(|x| x.technique.clone());
    let assay = match family {
        "electrophysiology" => {
            if matches!(c.format(), "abf" | "atf") {
                "OBI:0002176"
            } else {
                "OBI:0000454"
            }
        }
        "plate-reader" => match first.as_ref().map(|t| t.id.as_str()) {
            Some("CHMO:0000057") => "OBI:0003829",
            Some("CHMO:0000060") => "OBI:0001501",
            _ => "",
        },
        "spectroscopy" => match jcamp_kind(c) {
            Some(JcampKind::Nmr) => "OBI:0000623",
            Some(JcampKind::Mass) => "OBI:0000470",
            _ => "",
        },
        _ => FAMILY_ASSAYS
            .iter()
            .find(|(f, _)| *f == family)
            .map_or("", |(_, a)| a),
    };
    let technique = match family {
        "chromatography" => vocab::term(match separation(c) {
            Some("GC") => "CHMO:0001002",
            Some("LC") => "CHMO:0001004",
            _ => "CHMO:0001000",
        }),
        "electrophysiology" => first.or_else(|| vocab::term(assay)),
        _ => first,
    };
    (technique, assay, format!("measurements[{k}]"))
}

/// Where mass-spectrometry and chromatography files record the method's name.
const MS_METHOD_NAME_PATHS: &[&str] = &[
    "spectra[0].extra.method_summary.instrument_method",
    "spectra[0].extra.method",
    "traces[0].extra.method",
    "traces[0].extra.detection_method",
];

/// Where each family's files record the method's name, in order of preference.
const METHOD_NAME_PATHS: &[(&str, &[&str])] = &[
    ("mass-spectrometry", MS_METHOD_NAME_PATHS),
    ("chromatography", MS_METHOD_NAME_PATHS),
    (
        "plate-reader",
        &["tables[0].extra.protocol", "tables[0].extra.experiment"],
    ),
    ("flow-cytometry", &["tables[0].extra.project"]),
    (
        "nmr",
        &[
            "traces[0].extra.experiment",
            "traces[0].extra.pulse_program",
        ],
    ),
    ("electrophysiology", &["traces[0].extra.protocol"]),
];

/// The method's name (as recorded) and the field it came from.
fn method_name(c: &Ctx<'_>, family: &str) -> Option<(String, String)> {
    let paths = METHOD_NAME_PATHS
        .iter()
        .find(|(f, _)| *f == family)
        .map_or(&[][..], |(_, p)| *p);
    let mut name = c.first_text(paths);
    if family == "flow-cytometry"
        && let Some(x) = fcs_vendor_keyword(c, &["EXPERIMENT NAME"])
    {
        name = Some(x);
    }
    if name.is_none() && c.format() == "lif" {
        name = lif_project_name(c);
    }
    name
}

/// A LAS X project (the LIF root element every image path starts with) is named by the user
/// after the experiment (`2021.02.02 Ishi hF-FSHR DMSO/Series003`); file names and defaults
/// (`Project007.lif`, `Experiment_002`) are not.
fn lif_project_name(c: &Ctx<'_>) -> Option<(String, String)> {
    let projects: BTreeSet<&str> = c
        .info
        .images
        .iter()
        .filter_map(|i| i.name.as_deref()?.split_once('/').map(|(p, _)| p.trim()))
        .collect();
    if projects.len() != 1 {
        return None;
    }
    let p = projects.into_iter().next()?;
    if is_generic_name(p)
        || is_placeholder(p)
        || !p.chars().any(|ch| ch.is_ascii_alphabetic())
        || FILE_EXTENSIONS
            .iter()
            .any(|e| p.to_ascii_lowercase().ends_with(e))
    {
        return None;
    }
    let i = c
        .info
        .images
        .iter()
        .position(|i| i.name.is_some())
        .unwrap_or(0);
    Some((p.to_string(), format!("images[{i}].name")))
}

/// How a method parameter is read from its field.
#[derive(Clone, Copy)]
enum ParamKind {
    /// Text as recorded.
    Text,
    /// A number with this UCUM unit.
    Number(&'static str),
    /// A whole count.
    Count,
    /// The recorded value, whatever its JSON type.
    Value,
    /// `true` when the field is present.
    Present,
}

/// (name, field, kind) of each parameter, read in order.
type ParamTable = &'static [(&'static str, &'static str, ParamKind)];

/// Method parameters per family.
const METHOD_PARAMS: &[(&str, ParamTable)] = &[
    (
        "nmr",
        &[
            ("nucleus", "traces[0].extra.nucleus", ParamKind::Text),
            (
                "pulse_program",
                "traces[0].extra.pulse_program",
                ParamKind::Text,
            ),
            (
                "spectrometer_frequency",
                "traces[0].extra.spectrometer_frequency_mhz",
                ParamKind::Number("MHz"),
            ),
            (
                "temperature",
                "traces[0].extra.temperature_k",
                ParamKind::Number("K"),
            ),
            ("solvent", "traces[0].extra.solvent", ParamKind::Text),
            ("probe", "traces[0].extra.probe", ParamKind::Text),
            ("scans", "traces[0].extra.scans", ParamKind::Count),
            (
                "dummy_scans",
                "traces[0].extra.dummy_scans",
                ParamKind::Count,
            ),
            (
                "spectral_width",
                "traces[0].extra.spectral_width_ppm",
                ParamKind::Number("ppm"),
            ),
        ],
    ),
    (
        "spectroscopy",
        &[
            ("nucleus", "traces[0].extra.nucleus", ParamKind::Text),
            (
                "spectrometer_frequency",
                "traces[0].extra.observe_frequency_mhz",
                ParamKind::Number("MHz"),
            ),
            ("solvent", "traces[0].extra.solvent", ParamKind::Text),
            ("data_type", "traces[0].extra.data_type", ParamKind::Text),
        ],
    ),
    (
        "flow-cytometry",
        &[
            (
                "cytometry_technology",
                "tables[0].extra.platform.technology",
                ParamKind::Text,
            ),
            (
                "spectral_detectors",
                "tables[0].extra.platform.detector_count",
                ParamKind::Count,
            ),
            (
                "mass_channels",
                "tables[0].extra.platform.mass_channel_count",
                ParamKind::Count,
            ),
        ],
    ),
    (
        "plate-reader",
        &[
            ("read_type", "tables[0].extra.read_type", ParamKind::Text),
            ("read_modes", "tables[0].extra.read_modes", ParamKind::Value),
            ("plate_type", "tables[0].extra.plate_type", ParamKind::Text),
            (
                "temperature",
                "tables[0].extra.temperature_c",
                ParamKind::Number("°C"),
            ),
        ],
    ),
    (
        "electrophysiology",
        &[(
            "acquisition_mode",
            "traces[0].extra.acquisition_mode",
            ParamKind::Text,
        )],
    ),
];

/// Flow-cytometry parameters after the trigger.
const FLOW_PARAMS_AFTER_TRIGGER: ParamTable = &[
    (
        "volume",
        "tables[0].extra.volume_nl",
        ParamKind::Number("nL"),
    ),
    (
        "timestep",
        "tables[0].extra.timestep_s",
        ParamKind::Number("s"),
    ),
    (
        "compensation_matrix",
        "tables[0].extra.spillover",
        ParamKind::Present,
    ),
];

fn read_params(c: &mut Ctx<'_>, table: ParamTable) {
    for &(name, path, kind) in table {
        match kind {
            ParamKind::Text => c.param_text(name, path),
            ParamKind::Number(unit) => c.param_num(name, path, unit),
            ParamKind::Count => {
                if let Some(n) = c.num(path) {
                    c.param(name, Quantity::plain(n as u64), path);
                }
            }
            ParamKind::Value => {
                if let Some(v) = c.get(path).cloned() {
                    c.param(name, Quantity::plain(v), path);
                }
            }
            ParamKind::Present => {
                if c.get(path).is_some() {
                    c.param(name, Quantity::plain(true), path);
                }
            }
        }
    }
}

fn method_params(c: &mut Ctx<'_>, family: &str) {
    match family {
        "mass-spectrometry" | "chromatography" => return ms_chrom_params(c),
        "microscopy" | "electron-microscopy" => return imaging_params(c),
        _ => {}
    }
    if let Some((_, table)) = METHOD_PARAMS.iter().find(|(f, _)| *f == family) {
        read_params(c, table);
    }
    match family {
        "flow-cytometry" => {
            if let Some(tr) = c.get("tables[0].extra.trigger").cloned()
                && let Some(p) = tr.get("parameter").and_then(text_of)
            {
                c.param("trigger", Quantity::plain(p), "tables[0].extra.trigger");
            }
            read_params(c, FLOW_PARAMS_AFTER_TRIGGER);
        }
        "electrophysiology" => {
            if let Some(t) = c.info.traces.iter().find(|t| t.sample_rate_hz > 0.0) {
                let (rate, i) = (t.sample_rate_hz, t.index);
                c.param(
                    "sample_rate",
                    Quantity::number(rate, "Hz"),
                    &format!("traces[{i}].sample_rate_hz"),
                );
            }
        }
        _ => {}
    }
}

fn ms_chrom_params(c: &mut Ctx<'_>) {
    if let Some(v) = c.num("spectra[0].extra.method_summary.method_length_min") {
        c.param(
            "method_length",
            Quantity::number(v, "min"),
            "spectra[0].extra.method_summary.method_length_min",
        );
    }
    if let Some(d) = c.get("spectra[0].extra.method_summary.devices").cloned()
        && d.as_array().is_some_and(|a| !a.is_empty())
    {
        c.param(
            "devices",
            Quantity::plain(d),
            "spectra[0].extra.method_summary.devices",
        );
    }
    for p in [
        "spectra[0].extra.injection_volume",
        "traces[0].extra.injection_volume",
        "spectra[0].extra.method_file.injection_volume_ul",
        "traces[0].extra.method_file.injection_volume_ul",
    ] {
        if let Some(v) = c.num(p).filter(|v| *v > 0.0) {
            c.param("injection_volume", Quantity::number(v, "µL"), p);
            break;
        }
    }
    if let Some(s) = c.info.spectra.first() {
        let pol = polarities(s);
        if !pol.is_empty() {
            c.param(
                "polarity",
                Quantity::plain(Value::from(pol)),
                "spectra[0].extra.polarities",
            );
        }
        if !s.ms_levels.is_empty() {
            let lv = s.ms_levels.clone();
            c.param(
                "ms_levels",
                Quantity::plain(Value::from(lv)),
                "spectra[0].ms_levels",
            );
        }
        if let Some(src) = ion_sources(c, s).first().cloned() {
            c.param(
                "ion_source",
                Quantity::plain(src),
                "spectra[0].extra.ion_sources",
            );
        }
        c.param_text("acquisition_mode", "spectra[0].extra.acquisition");
    }
    if let Some(sep) = separation(c) {
        c.param(
            "separation",
            Quantity::plain(sep),
            "traces[0].extra.separation",
        );
    }
    let detectors: Vec<String> = c
        .info
        .traces
        .iter()
        .filter_map(detector_of)
        .map(|(w, _)| w.to_string())
        .fold(Vec::new(), |mut acc, x| {
            if !acc.contains(&x) {
                acc.push(x);
            }
            acc
        });
    if c.family() == "chromatography" && !detectors.is_empty() {
        c.param(
            "detectors",
            Quantity::plain(Value::from(detectors)),
            "traces[].extra.detector",
        );
    }
    let p = "traces[0].extra.run_time_s";
    if let Some(v) = c.num(p) {
        c.param("run_time", Quantity::number(v, "s"), p);
    }
    // GC settings from the ChemStation method report (`acqmeth.txt`).
    for base in [
        "spectra[0].extra.method_file",
        "traces[0].extra.method_file",
    ] {
        if c.get(base).is_none() {
            continue;
        }
        if let Some(steps) = c
            .get(&format!("{base}.oven_program"))
            .filter(|v| v.as_array().is_some_and(|a| !a.is_empty()))
            .cloned()
        {
            c.param(
                "oven_program",
                Quantity::plain(steps),
                &format!("{base}.oven_program"),
            );
        }
        if !c
            .e
            .method
            .as_ref()
            .is_some_and(|m| m.parameters.contains_key("method_length"))
        {
            c.param_num("method_length", &format!("{base}.run_time_min"), "min");
        }
        c.param_text("column", &format!("{base}.column"));
        c.param_text("inlet_mode", &format!("{base}.inlet_mode"));
        break;
    }
    // LC pump program (Thermo method text).
    for p in [
        "spectra[0].extra.method_summary.gradient",
        "traces[0].extra.method_file.gradient",
        "spectra[0].extra.method_file.gradient",
    ] {
        if let Some(g) = c.get(p).cloned() {
            if let Some(steps) = g
                .get("steps")
                .filter(|v| v.as_array().is_some_and(|a| !a.is_empty()))
            {
                c.param("gradient", Quantity::plain(steps.clone()), p);
            }
            if let Some(sol) = g
                .get("solvents")
                .filter(|v| v.as_object().is_some_and(|o| !o.is_empty()))
            {
                c.param("solvents", Quantity::plain(sol.clone()), p);
            }
            break;
        }
    }
    c.param_text("tune_method", "traces[0].extra.tune_method");
    c.param_text("inlet_method", "traces[0].extra.inlet_method");
}

fn imaging_params(c: &mut Ctx<'_>) {
    let Some(im) = c.info.images.first() else {
        return;
    };
    if let Some(o) = &im.objective {
        let (mag, na, imm, model) = (
            o.nominal_magnification,
            o.lens_na,
            o.immersion.clone(),
            o.model.clone(),
        );
        if let Some(m) = mag {
            c.param(
                "objective_magnification",
                Quantity::plain(finite(m)),
                "images[0].objective.nominal_magnification",
            );
        }
        if let Some(n) = na {
            c.param(
                "objective_na",
                Quantity::plain(finite(n)),
                "images[0].objective.lens_na",
            );
        }
        if let Some(i) = imm.filter(|s| !is_placeholder(s)) {
            c.param(
                "immersion",
                Quantity::plain(i),
                "images[0].objective.immersion",
            );
        }
        if let Some(mo) = model.filter(|s| !is_placeholder(s)) {
            c.param(
                "objective",
                Quantity::plain(mo.split_whitespace().collect::<Vec<_>>().join(" ")),
                "images[0].objective.model",
            );
        }
    }
    if let Some(x) = im.physical_size.x {
        c.param(
            "pixel_size",
            Quantity::number(x, "µm"),
            "images[0].physical_size.x",
        );
    }
    if let Some(z) = im.physical_size.z.filter(|_| im.size_z > 1) {
        c.param(
            "z_step",
            Quantity::number(z, "µm"),
            "images[0].physical_size.z",
        );
    }
    if let Some(t) = im.time_increment_s.filter(|_| im.size_t > 1) {
        c.param(
            "time_interval",
            Quantity::number(t, "s"),
            "images[0].time_increment_s",
        );
    }
    if let Some(d) = im.instrument.as_ref().and_then(|i| i.detector.clone()) {
        c.param(
            "detector",
            Quantity::plain(d),
            "images[0].instrument.detector",
        );
    }
    let ch: Vec<Value> = im
        .channels
        .iter()
        .map(|ch| {
            let mut o = serde_json::Map::new();
            o.insert("index".into(), Value::from(ch.index));
            if let Some(n) = &ch.name {
                o.insert("name".into(), Value::from(n.clone()));
            }
            if let Some(f) = &ch.fluorophore {
                o.insert("fluorophore".into(), Value::from(f.clone()));
            }
            if let Some(x) = ch.excitation_nm {
                o.insert("excitation_nm".into(), finite(x));
            }
            if let Some(x) = ch.emission_nm {
                o.insert("emission_nm".into(), finite(x));
            }
            Value::Object(o)
        })
        .collect();
    if !ch.is_empty() && im.samples_per_pixel < 3 {
        c.param(
            "channels",
            Quantity::plain(Value::from(ch)),
            "images[0].channels",
        );
    }
    for (k, unit, name) in [
        ("voltage_kv", "kV", "accelerating_voltage"),
        ("high_tension_kv", "kV", "accelerating_voltage"),
        ("camera_length_m", "m", "camera_length"),
    ] {
        let p = format!("images[0].extra.{k}");
        if let Some(v) = c.num(&p)
            && !c
                .e
                .method
                .as_ref()
                .is_some_and(|m| m.parameters.contains_key(name))
        {
            c.param(name, Quantity::number(v, unit), &p);
        }
    }
    if let Some(m) = c.num("images[0].extra.magnification") {
        c.param(
            "magnification",
            Quantity::plain(finite(m)),
            "images[0].extra.magnification",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        ChannelInfo, ColumnInfo, FormatDescriptor, InstrumentInfo, PhysicalSize, SignalChannelInfo,
    };
    use crate::pixel::PixelType;
    use crate::provenance::Confidence;
    use serde_json::json;

    fn file(id: &str, family: &str, vendor: &str) -> FileInfo {
        FileInfo {
            path: format!("x.{id}"),
            size_bytes: 1,
            format: FormatDescriptor {
                id: id.into(),
                name: id.into(),
                vendor: vendor.into(),
                extensions: vec![],
                family: family.into(),
                can_read: true,
                can_write: false,
                confidence: Confidence::High,
                known_gaps: vec![],
            },
            format_version: None,
            images: vec![],
            tables: vec![],
            spectra: vec![],
            traces: vec![],
            plane_count: 0,
            notes: vec![],
        }
    }

    fn check(e: &Experiment) {
        for t in e.terms() {
            assert!(vocab::is_known(t), "unknown term {t:?}");
        }
        for (p, q) in e.quantities() {
            if let Some(u) = &q.unit {
                assert_eq!(
                    q.ucum.as_deref(),
                    vocab::ucum(u),
                    "{p}: unit {u} has no UCUM code"
                );
            }
        }
        for p in e.value_paths() {
            assert!(e.origin_of(&p).is_some(), "no provenance for {p}");
        }
        if let Some(s) = &e.sample
            && s.id.is_some()
        {
            assert!(s.source_field.is_some());
        }
    }

    #[test]
    fn thermo_run() {
        let mut f = file(
            "thermo-raw",
            "mass-spectrometry",
            "Thermo Fisher Scientific",
        );
        f.spectra.push(SpectraInfo {
            index: 0,
            name: Some("QC1".into()),
            scan_count: 2048,
            ms_levels: vec![1],
            rt_range_s: Some([0.7, 1139.5]),
            instrument: Some(InstrumentInfo {
                manufacturer: Some("Thermo Fisher Scientific".into()),
                model: Some("LTQ Orbitrap Discovery".into()),
                ..Default::default()
            }),
            extra: [
                ("sample_name", json!("QC1")),
                ("vial", json!("D:35")),
                ("operator", json!("LTQ OT Discovery")),
                ("acquired_at", json!("2009-05-07T20:03:10.046Z")),
                ("polarities", json!(["negative"])),
                ("ion_sources", json!(["ESI"])),
                ("analyzers", json!(["FTMS"])),
                ("injection_volume", json!(10.0)),
                ("comment", json!("2uL injection, top8")),
                (
                    "method_summary",
                    json!({"instrument_method": "C:\\Xcalibur\\methods\\GM\\UPLC-ESI-profiling-19min_profile_NEG.meth",
                           "method_length_min": 19.0, "devices": ["Accela Pump", "LTQ Orbitrap Discovery MS"],
                           "gradient": {"device": "LegacyDualPump", "solvents": {"A": "water"},
                                        "steps": [{"time_min": 0.0, "flow_ul_min": 500.0, "percent": {"A": 100.0, "B": 0.0}},
                                                  {"time_min": 13.0, "flow_ul_min": 500.0, "percent": {"A": 0.0, "B": 100.0}}]}}),
                ),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
        });
        let mut prov = ProvenanceMap::new();
        prov.insert("spectra[0].extra.sample_name".into(), Source::PriorArt);
        let e = derive(&f, &prov);
        check(&e);
        let s = e.sample.as_ref().unwrap();
        assert_eq!(s.id.as_deref(), Some("QC1"));
        assert_eq!(
            s.source_field.as_deref(),
            Some("spectra[0].extra.sample_name")
        );
        assert_eq!(s.sequence_position.as_deref(), Some("D:35"));
        assert_eq!(e.provenance["sample.id"].source, Source::PriorArt);
        let m = e.method.as_ref().unwrap();
        assert_eq!(
            m.name.as_deref(),
            Some("UPLC-ESI-profiling-19min_profile_NEG")
        );
        assert_eq!(m.technique.as_ref().unwrap().id, "CHMO:0000524");
        assert_eq!(m.parameters["method_length"].ucum.as_deref(), Some("min"));
        let what = &e.measurements[0].what;
        assert!(
            what.starts_with("LC-MS, negative mode, ESI, 2,048 scans, MS1"),
            "{what}"
        );
        assert!(e.measurements[0].terms.iter().any(|t| t.id == "MS:1000484"));
        assert_eq!(
            e.instrument.as_ref().unwrap().kind.as_ref().unwrap().id,
            "OBI:0000049"
        );
        assert_eq!(e.acquisition.as_ref().unwrap().duration_s, Some(1139.5));
        assert_eq!(
            e.acquisition.as_ref().unwrap().comment.as_deref(),
            Some("2uL injection, top8")
        );
        assert_eq!(
            e.provenance["acquisition.comment"].from,
            "spectra[0].extra.comment"
        );
        assert_eq!(m.parameters["gradient"].value.as_array().unwrap().len(), 2);
        assert_eq!(m.parameters["solvents"].value["A"], "water");
    }

    #[test]
    fn fluorescence_stack() {
        let mut f = file("czi", "microscopy", "Carl Zeiss Microscopy");
        let mut im = ImageInfo::new(0, 1024, 1024, PixelType::Uint16);
        im.size_c = 3;
        im.size_z = 21;
        im.physical_size = PhysicalSize::micrometres(Some(0.1), Some(0.1), Some(0.5));
        im.channels = ["DAPI", "GFP", "mCherry"]
            .iter()
            .enumerate()
            .map(|(i, n)| ChannelInfo {
                index: i as u32,
                name: Some((*n).into()),
                excitation_nm: Some(400.0 + 100.0 * i as f64),
                ..Default::default()
            })
            .collect();
        im.name = Some("Position 1".into());
        f.images.push(im.finish());
        let e = derive(&f, &ProvenanceMap::new());
        check(&e);
        assert_eq!(
            e.measurements[0].what,
            "fluorescence, 3 channels (DAPI, GFP, mCherry), 21 z-slices 0.5 µm apart, 1024 × 1024 px at 0.1 µm/px"
        );
        assert_eq!(
            e.measurements[0].technique.as_ref().unwrap().id,
            "FBbi:00000246"
        );
        assert!(e.sample.is_none(), "Position 1 is not a sample id");
        assert_eq!(
            e.instrument.as_ref().unwrap().vendor.as_deref(),
            Some("Carl Zeiss Microscopy")
        );
        assert_eq!(e.provenance["instrument.vendor"].from, "format.vendor");
    }

    #[test]
    fn plate_read() {
        let mut f = file("plate", "plate-reader", "various");
        f.tables.push(TableInfo {
            index: 0,
            name: Some("Plate 1".into()),
            row_count: 96,
            columns: vec![ColumnInfo::default()],
            extra: [
                ("barcode", json!("PL-0042")),
                (
                    "reads",
                    json!([{"mode": "absorbance", "wavelength_nm": 450.0}]),
                ),
                ("wells_measured", json!(96)),
                ("plate_well_count", json!(96)),
                ("protocol", json!("ELISA.prt")),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
        });
        let e = derive(&f, &ProvenanceMap::new());
        check(&e);
        assert_eq!(e.measurements[0].what, "absorbance at 450 nm, 96 wells");
        let s = e.sample.as_ref().unwrap();
        assert_eq!(s.id.as_deref(), Some("PL-0042"));
        assert_eq!(s.barcode.as_deref(), Some("PL-0042"));
        assert_eq!(e.method.as_ref().unwrap().name.as_deref(), Some("ELISA"));
    }

    #[test]
    fn ephys_and_merge() {
        let mut f = file("abf", "electrophysiology", "Molecular Devices");
        f.traces.push(TraceInfo {
            index: 0,
            name: Some("proto".into()),
            sample_rate_hz: 20000.0,
            sample_count: 40000,
            sweep_count: 3,
            channels: vec![SignalChannelInfo {
                index: 0,
                name: "IN 0".into(),
                unit: Some("pA".into()),
                dtype: "int16".into(),
                scale: 1.0,
                offset: 0.0,
                extra: BTreeMap::new(),
            }],
            start_s: None,
            extra: BTreeMap::new(),
        });
        let mut e = derive(&f, &ProvenanceMap::new());
        check(&e);
        assert_eq!(
            e.measurements[0].what,
            "electrophysiology, 1 channel (IN 0 [pA]), 3 sweeps × 2 s at 20 kHz"
        );
        assert_eq!(e.acquisition.as_ref().unwrap().duration_s, Some(6.0));
        let mut own = Experiment {
            sample: Some(Sample {
                id: Some("cell-7".into()),
                source_field: Some("comment".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        own.provenance.insert(
            "sample.id".into(),
            Origin {
                source: Source::Inferred,
                from: "comment".into(),
            },
        );
        e.merge(own);
        check(&e);
        assert_eq!(e.sample.as_ref().unwrap().id.as_deref(), Some("cell-7"));
    }

    #[test]
    fn jcamp_long_dates() {
        assert_eq!(
            jcamp_long_date("2019/07/27 11:58:49.000").as_deref(),
            Some("2019-07-27T11:58:49.000")
        );
        assert_eq!(jcamp_long_date("95/11/30 08:32:13"), None);
        assert_eq!(jcamp_long_date("2019/13/27 11:58:49"), None);
    }

    #[test]
    fn instance_names_are_not_models() {
        for v in ["Instrumen", "Instrument 1", "DAD1", "MWD1", "fid2"] {
            assert!(is_instance_name(v), "{v}");
        }
        for v in [
            "G1315B",
            "HP G1530A",
            "5977B MSD",
            "GCI",
            "DAD",
            "Instrument X",
        ] {
            assert!(!is_instance_name(v), "{v}");
        }
        assert!(is_generic_module_tag("GCI") && !is_generic_module_tag("G1315B"));
    }

    #[test]
    fn sample_id_heuristic() {
        for bad in [
            "Position 1",
            "point name 2",
            "Series011",
            "TileScan_002_Merging",
            "multi-channel.ome.tiff",
            "s_1_t_1_c_1_z_1.czi #1",
            "Current-Position",
            "ScanRegion0",
            "Cell 1 Dendrites zoom 1.5",
            "b2_001_Crop001_Resize001",
            "423_001",
            "P1",
            "label image",
        ] {
            assert!(!looks_like_sample_id(bad), "{bad}");
        }
        for good in [
            "PEI_laminin_35k",
            "tubhiswt",
            "HeLa-GFP-03",
            "Exp/KO_mouse_12",
        ] {
            assert!(looks_like_sample_id(good), "{good}");
        }
    }

    #[test]
    fn info_output_flattens() {
        let f = file("abf", "electrophysiology", "Molecular Devices");
        let o = InfoOutput::from_info(f);
        let v = serde_json::to_value(&o).unwrap();
        assert!(v.get("path").is_some());
        assert_eq!(o.format.id, "abf");
        let back: InfoOutput = serde_json::from_value(v).unwrap();
        assert_eq!(back.path, "x.abf");
    }
}
