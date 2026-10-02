//! The three Parquet tables of an index, their columns, and generic reading and writing.
//!
//! Every column is declared once in [`EXPERIMENTS`], [`FILES`] or [`PROBLEMS`] (name, type,
//! description): the writer, the reader, `index.json` and `book/src/guides/lab-shares.md` all follow that list.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::builder::{
    BooleanBuilder, Float64Builder, Int64Builder, ListBuilder, StringBuilder,
    TimestampMicrosecondBuilder, UInt64Builder,
};
use arrow_array::cast::AsArray;
use arrow_array::types::{Float64Type, Int64Type, TimestampMicrosecondType, UInt64Type};
use arrow_array::{Array, ArrayRef, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use openreadout_core::{Error, Result};
use parquet::arrow::ArrowWriter;
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use serde::Serialize;

use crate::formats::preservation;
use crate::record::{ItemKind, Record};

/// Column type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ColumnKind {
    /// UTF-8 string.
    String,
    /// Unsigned 64-bit integer.
    Uint64,
    /// Signed 64-bit integer.
    Int64,
    /// 64-bit float.
    Float64,
    /// Boolean.
    Bool,
    /// Timestamp, microseconds since the Unix epoch, UTC.
    Timestamp,
    /// List of strings.
    StringList,
    /// List of signed 64-bit integers.
    Int64List,
}

impl ColumnKind {
    fn data_type(self) -> DataType {
        match self {
            ColumnKind::String => DataType::Utf8,
            ColumnKind::Uint64 => DataType::UInt64,
            ColumnKind::Int64 => DataType::Int64,
            ColumnKind::Float64 => DataType::Float64,
            ColumnKind::Bool => DataType::Boolean,
            ColumnKind::Timestamp => DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            ColumnKind::StringList => {
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, true)))
            }
            ColumnKind::Int64List => {
                DataType::List(Arc::new(Field::new("item", DataType::Int64, true)))
            }
        }
    }
}

/// What a numeric column measures, for units in queries (`size>1GB`, `rate>=20kHz`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UnitClass {
    /// Bytes (`KB`, `MB`, `GB`, `TB` are powers of 1000; `KiB`, `MiB`, ... of 1024).
    Bytes,
    /// Seconds (`ms`, `s`, `min`, `h`, `d`).
    Seconds,
    /// Hertz (`Hz`, `kHz`, `MHz`).
    Hertz,
    /// Micrometres (`nm`, `µm`/`um`, `mm`).
    Micrometres,
    /// Magnification (`63x`).
    Magnification,
}

/// One column.
#[derive(Debug, Clone, Copy, Serialize, schemars::JsonSchema)]
pub struct ColumnDef {
    /// Column name.
    pub name: &'static str,
    /// Type.
    pub kind: ColumnKind,
    /// Unit class of a numeric column (query units).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<UnitClass>,
    /// Meaning.
    pub description: &'static str,
}

const fn col(name: &'static str, kind: ColumnKind, description: &'static str) -> ColumnDef {
    ColumnDef {
        name,
        kind,
        unit: None,
        description,
    }
}

const fn ucol(
    name: &'static str,
    kind: ColumnKind,
    unit: UnitClass,
    description: &'static str,
) -> ColumnDef {
    ColumnDef {
        name,
        kind,
        unit: Some(unit),
        description,
    }
}

use ColumnKind as K;
use UnitClass as U;

/// `experiments.parquet`: one row per data set.
pub const EXPERIMENTS: &[ColumnDef] = &[
    col(
        "path",
        K::String,
        "Absolute path of the data set (a file, or a directory data set such as a Bruker .d).",
    ),
    col("name", K::String, "File or directory name."),
    col("dir", K::String, "Parent directory."),
    col("root", K::String, "The crawl root it was found under."),
    col(
        "kind",
        K::String,
        "`file` or `directory` (a directory data set).",
    ),
    col(
        "ext",
        K::String,
        "Lower-case extension (`czi`, `tiff`, `d`).",
    ),
    col(
        "format",
        K::String,
        "Format id (`czi`, `nd2`, `thermo-raw`, ...; `openreadout self formats`).",
    ),
    col("format_name", K::String, "Format name."),
    col(
        "family",
        K::String,
        "Format family: microscopy, electron-microscopy, flow-cytometry, electrophysiology, mass-spectrometry, chromatography, nmr, plate-reader, spectroscopy, container.",
    ),
    col(
        "format_vendor",
        K::String,
        "Vendor or standards body of the format.",
    ),
    col(
        "preservation",
        K::String,
        "`open` (an open, documented format), `vendor` (proprietary, maintained vendor software) or `legacy` (proprietary; the vendor's software is discontinued).",
    ),
    col(
        "confidence",
        K::String,
        "Detection confidence: `definite`, `likely` or `extension_only`.",
    ),
    col(
        "format_version",
        K::String,
        "Format version, when the reader reports one.",
    ),
    col(
        "assurance",
        K::String,
        "`validated`, `partially_validated` or `unvalidated`: whether the file lies inside what its reader was validated on (docs/assurance.md).",
    ),
    col(
        "variant",
        K::String,
        "Variant fingerprint (`info` → assurance.fingerprint): format id and every feature that decides how the file is decoded.",
    ),
    ucol(
        "size_bytes",
        K::Uint64,
        U::Bytes,
        "Bytes of the whole data set (the file plus its member files, or every file of a directory data set).",
    ),
    col("file_count", K::Uint64, "Files in the data set."),
    col(
        "mtime",
        K::Timestamp,
        "Modification time (the newest file of the data set).",
    ),
    col(
        "fingerprint",
        K::String,
        "xxh3-128 of the size and the first and last 64 KiB (directory data sets: member names and sizes plus the largest file's fingerprint).",
    ),
    col(
        "sample_id",
        K::String,
        "`experiment.sample.id`: the sample identifier the file records.",
    ),
    col("sample_name", K::String, "`experiment.sample.name`."),
    col(
        "sample_well",
        K::String,
        "`experiment.sample.well`: plate well.",
    ),
    col("sample_barcode", K::String, "`experiment.sample.barcode`."),
    col(
        "sample_position",
        K::String,
        "`experiment.sample.sequence_position`: vial or autosampler position.",
    ),
    col(
        "sample_source_field",
        K::String,
        "Field the sample id came from.",
    ),
    col(
        "instrument_vendor",
        K::String,
        "`experiment.instrument.vendor`.",
    ),
    col(
        "instrument_model",
        K::String,
        "`experiment.instrument.model`.",
    ),
    col(
        "instrument_serial",
        K::String,
        "`experiment.instrument.serial`.",
    ),
    col(
        "instrument_software",
        K::String,
        "`experiment.instrument.software`.",
    ),
    col(
        "instrument_software_version",
        K::String,
        "`experiment.instrument.software_version`.",
    ),
    col(
        "instrument_kind_id",
        K::String,
        "OBI device term id (`OBI:0400169` microscope).",
    ),
    col("instrument_kind_label", K::String, "OBI device term label."),
    col(
        "method_name",
        K::String,
        "`experiment.method.name`: method, protocol or experiment name.",
    ),
    col(
        "technique_id",
        K::String,
        "Technique term id (CHMO, FBbi or OBI: `FBbi:00000246`).",
    ),
    col(
        "technique_label",
        K::String,
        "Technique term label (`fluorescence microscopy`).",
    ),
    col("assay_id", K::String, "OBI assay term id."),
    col("assay_label", K::String, "OBI assay term label."),
    col(
        "terms",
        K::StringList,
        "Every term id in the experiment (technique, assay, instrument kind, measurement techniques and terms).",
    ),
    col(
        "term_labels",
        K::StringList,
        "Labels of `terms`, same order.",
    ),
    col(
        "operator",
        K::String,
        "`experiment.acquisition.operator` (personal data: see `pii_*`).",
    ),
    col(
        "started_at",
        K::Timestamp,
        "Acquisition start (`experiment.acquisition.started_at`, else the first image's `acquired_at`), UTC.",
    ),
    col(
        "started_at_text",
        K::String,
        "Acquisition start as recorded (time zone as the file gives it).",
    ),
    col("ended_at", K::Timestamp, "Acquisition end, UTC."),
    ucol(
        "duration_s",
        K::Float64,
        U::Seconds,
        "Length of the recorded data, seconds.",
    ),
    col("acquired_year", K::Int64, "Year of `started_at`."),
    col(
        "what",
        K::StringList,
        "Each measurement in scientific words (`experiment.measurements[].what`).",
    ),
    col(
        "parameters_json",
        K::String,
        "`experiment.method.parameters` as JSON (`{\"nucleus\": {\"value\": \"1H\"}}`); queried as `param.<name>`.",
    ),
    col("image_count", K::Uint64, "Images."),
    col("plane_count", K::Uint64, "2-D planes over all images."),
    col("size_x", K::Uint64, "Width of the largest image, pixels."),
    col("size_y", K::Uint64, "Height of the largest image, pixels."),
    col("size_z", K::Uint64, "Z-slices of the largest image."),
    col("size_c", K::Uint64, "Channels of the largest image."),
    col("size_t", K::Uint64, "Time points of the largest image."),
    col(
        "pixel_type",
        K::String,
        "Pixel type of the largest image (`uint16`, ...).",
    ),
    ucol(
        "physical_size_x_um",
        K::Float64,
        U::Micrometres,
        "Pixel width, µm.",
    ),
    ucol(
        "physical_size_y_um",
        K::Float64,
        U::Micrometres,
        "Pixel height, µm.",
    ),
    ucol(
        "physical_size_z_um",
        K::Float64,
        U::Micrometres,
        "Z-step, µm.",
    ),
    ucol(
        "time_increment_s",
        K::Float64,
        U::Seconds,
        "Time between time points, seconds.",
    ),
    col(
        "channels",
        K::StringList,
        "Channel names and fluorophores of every image; FCS `$PnS` labels.",
    ),
    col("objective", K::String, "Objective model."),
    ucol(
        "objective_magnification",
        K::Float64,
        U::Magnification,
        "Objective nominal magnification.",
    ),
    col("objective_na", K::Float64, "Objective numerical aperture."),
    col("immersion", K::String, "Objective immersion."),
    col("mosaic_tiles", K::Uint64, "Tiles of a mosaic image."),
    col(
        "table_count",
        K::Uint64,
        "Tables (FCS data sets, plate reads, event tables).",
    ),
    col(
        "table_rows",
        K::Uint64,
        "Rows over all tables (events, wells).",
    ),
    col(
        "table_columns",
        K::StringList,
        "Column names (FCS `$PnN`, ...), first 64.",
    ),
    col(
        "trace_count",
        K::Uint64,
        "Traces (sweeps, chromatograms, FIDs, spectra).",
    ),
    col(
        "trace_channels",
        K::Uint64,
        "Signal channels over all traces.",
    ),
    col("trace_sweeps", K::Uint64, "Sweeps over all traces."),
    col(
        "trace_samples",
        K::Uint64,
        "Samples per channel over all traces and sweeps.",
    ),
    ucol(
        "sample_rate_hz",
        K::Float64,
        U::Hertz,
        "Highest sample rate, Hz.",
    ),
    col("spectra_runs", K::Uint64, "Mass-spectrometry runs."),
    col("scan_count", K::Uint64, "Mass spectra over all runs."),
    col("ms_levels", K::Int64List, "MS levels present."),
    ucol(
        "rt_min_s",
        K::Float64,
        U::Seconds,
        "First retention time, seconds.",
    ),
    ucol(
        "rt_max_s",
        K::Float64,
        U::Seconds,
        "Last retention time, seconds.",
    ),
    col(
        "check_status",
        K::String,
        "`ok`, `warning`, `truncated`, `corrupt`, `unreadable`, `unsupported` or `not_checked`.",
    ),
    col(
        "check_mode",
        K::String,
        "`headers` (structure only; the default) or `full` (`check`).",
    ),
    col(
        "check_findings",
        K::Uint64,
        "Findings of the integrity check.",
    ),
    col(
        "check_codes",
        K::StringList,
        "Codes of the findings (`truncated`, `missing_part`, ...).",
    ),
    col(
        "error_code",
        K::String,
        "Why the data set could not be opened (`corrupt_file`, `unsupported_feature`, `io`).",
    ),
    col("error_message", K::String, "The error message."),
    col("pii_count", K::Uint64, "Personal-data flags."),
    col(
        "pii_kinds",
        K::StringList,
        "Kinds flagged (`person_name`, `email`, `phone`, `patient_id`, `date_of_birth`, `free_text`).",
    ),
    col(
        "pii_fields",
        K::StringList,
        "Field paths flagged (values are never stored).",
    ),
    col("notes", K::StringList, "Reader notes (first 8)."),
    col(
        "change",
        K::String,
        "`new`, `changed`, `unchanged` or `moved` relative to the previous run.",
    ),
    col(
        "moved_from",
        K::String,
        "Previous path of a moved data set.",
    ),
    col("indexed_at", K::Timestamp, "When the data set was read."),
    col(
        "tool_version",
        K::String,
        "openreadout version that read it.",
    ),
    col(
        "elapsed_ms",
        K::Float64,
        "Time spent reading it, milliseconds.",
    ),
    col(
        "experiment_json",
        K::String,
        "The whole `experiment` object as JSON (with provenance).",
    ),
];

/// `files.parquet`: one row per file seen (and per directory data set).
pub const FILES: &[ColumnDef] = &[
    col("path", K::String, "Absolute path."),
    col(
        "dataset_path",
        K::String,
        "The data set it belongs to (itself for a data set; null for unrecognised files).",
    ),
    col(
        "role",
        K::String,
        "`dataset` (the file or directory opened), `member` (another file of a data set) or `unknown` (no reader recognises it).",
    ),
    col("kind", K::String, "`file` or `directory`."),
    ucol("size_bytes", K::Uint64, U::Bytes, "Bytes."),
    col("mtime", K::Timestamp, "Modification time."),
    col("ext", K::String, "Lower-case extension."),
    col(
        "format",
        K::String,
        "Format id of the data set it belongs to.",
    ),
    col(
        "fingerprint",
        K::String,
        "Content fingerprint (data sets and files up to 64 KiB).",
    ),
    col(
        "change",
        K::String,
        "`new`, `changed`, `unchanged` or `moved`.",
    ),
];

/// `problems.parquet`: one row per problem found.
pub const PROBLEMS: &[ColumnDef] = &[
    col("path", K::String, "The file or data set."),
    col("format", K::String, "Format id, when detected."),
    col(
        "category",
        K::String,
        "`integrity` (check findings), `readability` (could not be opened or summarized), `walk` (a directory or entry that could not be read) or `pii` (a personal-data flag).",
    ),
    col(
        "severity",
        K::String,
        "`error`, `warning`, `info` or `review` (personal data).",
    ),
    col(
        "code",
        K::String,
        "Stable code: a check finding code, an error code, or the personal-data kind.",
    ),
    col(
        "message",
        K::String,
        "Human-readable message (never a personal-data value).",
    ),
    col("field", K::String, "Personal data: the field path."),
    col("rule", K::String, "Personal data: the rule that fired."),
];

/// One value.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    /// No value.
    Null,
    /// A string.
    Str(String),
    /// An unsigned integer.
    U64(u64),
    /// A signed integer.
    I64(i64),
    /// A float.
    F64(f64),
    /// A boolean.
    Bool(bool),
    /// Microseconds since the Unix epoch, UTC.
    Time(i64),
    /// A list of strings.
    List(Vec<String>),
    /// A list of integers.
    IntList(Vec<i64>),
}

impl Cell {
    fn opt_str(v: Option<&str>) -> Cell {
        v.filter(|s| !s.is_empty())
            .map_or(Cell::Null, |s| Cell::Str(s.to_string()))
    }
    fn opt_f64(v: Option<f64>) -> Cell {
        v.filter(|x| x.is_finite()).map_or(Cell::Null, Cell::F64)
    }
    fn opt_u64(v: Option<u64>) -> Cell {
        v.map_or(Cell::Null, Cell::U64)
    }

    /// As JSON (timestamps as ISO-8601 UTC).
    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::Value;
        match self {
            Cell::Null => Value::Null,
            Cell::Str(s) => Value::from(s.as_str()),
            Cell::U64(v) => Value::from(*v),
            Cell::I64(v) => Value::from(*v),
            Cell::F64(v) => Value::from(*v),
            Cell::Bool(b) => Value::from(*b),
            Cell::Time(t) => Value::from(iso_from_micros(*t)),
            Cell::List(l) => Value::from(l.clone()),
            Cell::IntList(l) => Value::from(l.clone()),
        }
    }

    /// As short text for tables.
    pub fn to_text(&self) -> String {
        match self {
            Cell::Null => String::new(),
            Cell::Str(s) => s.clone(),
            Cell::U64(v) => v.to_string(),
            Cell::I64(v) => v.to_string(),
            Cell::F64(v) => format!("{v}"),
            Cell::Bool(b) => b.to_string(),
            Cell::Time(t) => iso_from_micros(*t),
            Cell::List(l) => l.join(", "),
            Cell::IntList(l) => l
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        }
    }
}

/// ISO-8601 UTC (`2020-05-03T10:11:12.000Z`) from microseconds since the epoch.
pub fn iso_from_micros(us: i64) -> String {
    let secs = us.div_euclid(1_000_000);
    let millis = (us.rem_euclid(1_000_000) / 1000) as u32;
    openreadout_core::time::unix_to_iso8601(secs, millis)
}

/// Microseconds since the epoch from an ISO-8601 timestamp (a missing zone is read as UTC).
pub fn micros_from_iso(s: &str) -> Option<i64> {
    let t = openreadout_core::time::iso8601_to_unix(s)?;
    let us = (t * 1e6).round();
    (us.is_finite() && us.abs() < 9.0e18).then_some(us as i64)
}

/// The year of a timestamp.
pub fn year_of(us: i64) -> i64 {
    iso_from_micros(us)
        .get(..4)
        .and_then(|y| y.parse().ok())
        .unwrap_or(1970)
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn parent(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Series or claims resolved at finalize: what to add to a primary data set.
#[derive(Debug, Clone, Default)]
pub struct Additions {
    /// Extra bytes (series members).
    pub bytes: u64,
    /// Extra files.
    pub files: u64,
    /// Newer modification time.
    pub mtime_us: Option<i64>,
    /// Summaries of series members.
    pub summary: Option<crate::record::Summary>,
}

/// The `experiments.parquet` row of a data set record.
#[allow(clippy::many_single_char_names)]
pub fn experiment_row(r: &Record, add: Option<&Additions>) -> Vec<Cell> {
    let e = r.experiment.as_ref();
    let sample = e.and_then(|e| e.sample.as_ref());
    let ins = e.and_then(|e| e.instrument.as_ref());
    let method = e.and_then(|e| e.method.as_ref());
    let acq = e.and_then(|e| e.acquisition.as_ref());
    let mut summary = r.summary.clone();
    if let Some(s) = add.and_then(|a| a.summary.as_ref()) {
        summary.add(s);
    }
    let s = &summary;
    let started_text = acq
        .and_then(|a| a.started_at.clone())
        .or_else(|| s.acquired_at.clone());
    let started = started_text.as_deref().and_then(micros_from_iso);
    let ended = acq
        .and_then(|a| a.ended_at.as_deref())
        .and_then(micros_from_iso);
    let mut terms: Vec<(String, String)> = Vec::new();
    if let Some(e) = e {
        for t in e.terms() {
            if !terms.iter().any(|(id, _)| *id == t.id) {
                terms.push((t.id.clone(), t.label.clone()));
            }
        }
    }
    let what: Vec<String> = e
        .map(|e| e.measurements.iter().map(|m| m.what.clone()).collect())
        .unwrap_or_default();
    let params = method
        .filter(|m| !m.parameters.is_empty())
        .and_then(|m| serde_json::to_string(&m.parameters).ok());
    let (size, files, mtime) = {
        let base = r.total_size();
        let files = 1 + r.members.len() as u64;
        match add {
            Some(a) => (
                base.saturating_add(a.bytes),
                files + a.files,
                match (r.mtime_us, a.mtime_us) {
                    (Some(x), Some(y)) => Some(x.max(y)),
                    (x, y) => x.or(y),
                },
            ),
            None => (base, files, r.mtime_us),
        }
    };
    let file_count = match r.kind {
        ItemKind::Directory => r.members.len().max(1) as u64 + add.map_or(0, |a| a.files),
        ItemKind::File => files,
    };
    let check = r.check.as_ref();
    let mut pii_kinds: Vec<String> = r.pii.iter().map(|p| p.kind.clone()).collect();
    pii_kinds.sort();
    pii_kinds.dedup();
    let u = |v: Option<u32>| v.map_or(Cell::Null, |x| Cell::U64(u64::from(x)));
    vec![
        Cell::Str(r.path.clone()),
        Cell::Str(file_name(&r.path)),
        Cell::Str(parent(&r.path)),
        Cell::Str(r.root.clone()),
        Cell::Str(match r.kind {
            ItemKind::File => "file".into(),
            ItemKind::Directory => "directory".into(),
        }),
        Cell::opt_str(Some(&r.ext)),
        Cell::opt_str(r.format.as_deref()),
        Cell::opt_str(r.format_name.as_deref()),
        Cell::opt_str(r.family.as_deref()),
        Cell::opt_str(r.format_vendor.as_deref()),
        Cell::opt_str(r.format.as_deref().map(preservation)),
        Cell::opt_str(r.confidence.as_deref()),
        Cell::opt_str(r.format_version.as_deref()),
        Cell::opt_str(r.assurance.as_deref()),
        Cell::opt_str(r.variant.as_deref()),
        Cell::U64(size),
        Cell::U64(file_count),
        mtime.map_or(Cell::Null, Cell::Time),
        Cell::opt_str(r.fingerprint.as_deref()),
        Cell::opt_str(sample.and_then(|x| x.id.as_deref())),
        Cell::opt_str(sample.and_then(|x| x.name.as_deref())),
        Cell::opt_str(sample.and_then(|x| x.well.as_deref())),
        Cell::opt_str(sample.and_then(|x| x.barcode.as_deref())),
        Cell::opt_str(sample.and_then(|x| x.sequence_position.as_deref())),
        Cell::opt_str(sample.and_then(|x| x.source_field.as_deref())),
        Cell::opt_str(ins.and_then(|x| x.vendor.as_deref())),
        Cell::opt_str(ins.and_then(|x| x.model.as_deref())),
        Cell::opt_str(ins.and_then(|x| x.serial.as_deref())),
        Cell::opt_str(ins.and_then(|x| x.software.as_deref())),
        Cell::opt_str(ins.and_then(|x| x.software_version.as_deref())),
        Cell::opt_str(ins.and_then(|x| x.kind.as_ref()).map(|t| t.id.as_str())),
        Cell::opt_str(ins.and_then(|x| x.kind.as_ref()).map(|t| t.label.as_str())),
        Cell::opt_str(method.and_then(|x| x.name.as_deref())),
        Cell::opt_str(
            method
                .and_then(|x| x.technique.as_ref())
                .map(|t| t.id.as_str()),
        ),
        Cell::opt_str(
            method
                .and_then(|x| x.technique.as_ref())
                .map(|t| t.label.as_str()),
        ),
        Cell::opt_str(method.and_then(|x| x.assay.as_ref()).map(|t| t.id.as_str())),
        Cell::opt_str(
            method
                .and_then(|x| x.assay.as_ref())
                .map(|t| t.label.as_str()),
        ),
        Cell::List(terms.iter().map(|t| t.0.clone()).collect()),
        Cell::List(terms.iter().map(|t| t.1.clone()).collect()),
        Cell::opt_str(acq.and_then(|x| x.operator.as_deref())),
        started.map_or(Cell::Null, Cell::Time),
        Cell::opt_str(started_text.as_deref()),
        ended.map_or(Cell::Null, Cell::Time),
        Cell::opt_f64(acq.and_then(|x| x.duration_s)),
        started.map_or(Cell::Null, |t| Cell::I64(year_of(t))),
        Cell::List(what),
        Cell::opt_str(params.as_deref()),
        Cell::U64(u64::from(s.image_count)),
        Cell::U64(s.plane_count),
        u(s.size_x),
        u(s.size_y),
        u(s.size_z),
        u(s.size_c),
        u(s.size_t),
        Cell::opt_str(s.pixel_type.as_deref()),
        Cell::opt_f64(s.physical_size_x_um),
        Cell::opt_f64(s.physical_size_y_um),
        Cell::opt_f64(s.physical_size_z_um),
        Cell::opt_f64(s.time_increment_s),
        Cell::List(s.channels.clone()),
        Cell::opt_str(s.objective.as_deref()),
        Cell::opt_f64(s.objective_magnification),
        Cell::opt_f64(s.objective_na),
        Cell::opt_str(s.immersion.as_deref()),
        u(s.mosaic_tiles),
        Cell::U64(u64::from(s.table_count)),
        Cell::U64(s.table_rows),
        Cell::List(s.table_columns.clone()),
        Cell::U64(u64::from(s.trace_count)),
        Cell::U64(u64::from(s.trace_channels)),
        Cell::U64(s.trace_sweeps),
        Cell::U64(s.trace_samples),
        Cell::opt_f64(s.sample_rate_hz),
        Cell::U64(u64::from(s.spectra_runs)),
        Cell::U64(s.scan_count),
        Cell::IntList(s.ms_levels.iter().map(|&l| i64::from(l)).collect()),
        Cell::opt_f64(s.rt_min_s),
        Cell::opt_f64(s.rt_max_s),
        Cell::Str(r.check_status().to_string()),
        Cell::opt_str(check.map(|c| c.mode.as_str())),
        Cell::opt_u64(check.map(|c| u64::from(c.finding_count))),
        Cell::List(check.map_or_else(Vec::new, |c| {
            let mut v: Vec<String> = c.findings.iter().map(|f| f.code.clone()).collect();
            v.dedup();
            v
        })),
        Cell::opt_str(r.error.as_ref().map(|e| e.code.as_str())),
        Cell::opt_str(r.error.as_ref().map(|e| e.message.as_str())),
        Cell::U64(r.pii.len() as u64),
        Cell::List(pii_kinds),
        Cell::List(r.pii.iter().map(|p| p.field.clone()).collect()),
        Cell::List(r.notes.iter().take(8).cloned().collect()),
        Cell::Str(r.change.as_str().into()),
        Cell::opt_str(r.moved_from.as_deref()),
        micros_from_iso(&r.indexed_at).map_or(Cell::Null, Cell::Time),
        Cell::opt_str(Some(&r.tool_version)),
        Cell::F64(r.elapsed_ms),
        r.experiment
            .as_ref()
            .and_then(|e| serde_json::to_string(e).ok())
            .map_or(Cell::Null, Cell::Str),
    ]
}

/// A `files.parquet` row.
#[allow(clippy::too_many_arguments)]
pub fn file_row(
    path: &str,
    dataset: Option<&str>,
    role: &str,
    kind: ItemKind,
    size: u64,
    mtime: Option<i64>,
    format: Option<&str>,
    fingerprint: Option<&str>,
    change: &str,
) -> Vec<Cell> {
    vec![
        Cell::Str(path.to_string()),
        Cell::opt_str(dataset),
        Cell::Str(role.to_string()),
        Cell::Str(match kind {
            ItemKind::File => "file".into(),
            ItemKind::Directory => "directory".into(),
        }),
        Cell::U64(size),
        mtime.map_or(Cell::Null, Cell::Time),
        Cell::Str(crate::crawl::extension_of(Path::new(path))),
        Cell::opt_str(format),
        Cell::opt_str(fingerprint),
        Cell::Str(change.to_string()),
    ]
}

/// A `problems.parquet` row.
#[allow(clippy::too_many_arguments)]
pub fn problem_row(
    path: &str,
    format: Option<&str>,
    category: &str,
    severity: &str,
    code: &str,
    message: &str,
    field: Option<&str>,
    rule: Option<&str>,
) -> Vec<Cell> {
    vec![
        Cell::Str(path.to_string()),
        Cell::opt_str(format),
        Cell::Str(category.to_string()),
        Cell::Str(severity.to_string()),
        Cell::Str(code.to_string()),
        Cell::Str(message.to_string()),
        Cell::opt_str(field),
        Cell::opt_str(rule),
    ]
}

/// Arrow schema of a table.
pub fn schema_of(cols: &[ColumnDef]) -> SchemaRef {
    Arc::new(Schema::new(
        cols.iter()
            .map(|c| Field::new(c.name, c.kind.data_type(), true))
            .collect::<Vec<_>>(),
    ))
}

enum Builder {
    Str(StringBuilder),
    U64(UInt64Builder),
    I64(Int64Builder),
    F64(Float64Builder),
    Bool(BooleanBuilder),
    Time(TimestampMicrosecondBuilder),
    List(ListBuilder<StringBuilder>),
    IntList(ListBuilder<Int64Builder>),
}

impl Builder {
    fn new(kind: ColumnKind) -> Self {
        match kind {
            K::String => Builder::Str(StringBuilder::new()),
            K::Uint64 => Builder::U64(UInt64Builder::new()),
            K::Int64 => Builder::I64(Int64Builder::new()),
            K::Float64 => Builder::F64(Float64Builder::new()),
            K::Bool => Builder::Bool(BooleanBuilder::new()),
            K::Timestamp => Builder::Time(TimestampMicrosecondBuilder::new().with_timezone("UTC")),
            K::StringList => Builder::List(ListBuilder::new(StringBuilder::new())),
            K::Int64List => Builder::IntList(ListBuilder::new(Int64Builder::new())),
        }
    }

    fn push(&mut self, c: &Cell) {
        match (self, c) {
            (Builder::Str(b), Cell::Str(s)) => b.append_value(s),
            (Builder::Str(b), _) => b.append_null(),
            (Builder::U64(b), Cell::U64(v)) => b.append_value(*v),
            (Builder::U64(b), _) => b.append_null(),
            (Builder::I64(b), Cell::I64(v)) => b.append_value(*v),
            (Builder::I64(b), _) => b.append_null(),
            (Builder::F64(b), Cell::F64(v)) => b.append_value(*v),
            (Builder::F64(b), _) => b.append_null(),
            (Builder::Bool(b), Cell::Bool(v)) => b.append_value(*v),
            (Builder::Bool(b), _) => b.append_null(),
            (Builder::Time(b), Cell::Time(v)) => b.append_value(*v),
            (Builder::Time(b), _) => b.append_null(),
            (Builder::List(b), Cell::List(v)) => {
                for s in v {
                    b.values().append_value(s);
                }
                b.append(true);
            }
            (Builder::List(b), _) => b.append(true),
            (Builder::IntList(b), Cell::IntList(v)) => {
                for s in v {
                    b.values().append_value(*s);
                }
                b.append(true);
            }
            (Builder::IntList(b), _) => b.append(true),
        }
    }

    fn finish(&mut self) -> ArrayRef {
        match self {
            Builder::Str(b) => Arc::new(b.finish()),
            Builder::U64(b) => Arc::new(b.finish()),
            Builder::I64(b) => Arc::new(b.finish()),
            Builder::F64(b) => Arc::new(b.finish()),
            Builder::Bool(b) => Arc::new(b.finish()),
            Builder::Time(b) => Arc::new(b.finish()),
            Builder::List(b) => Arc::new(b.finish()),
            Builder::IntList(b) => Arc::new(b.finish()),
        }
    }
}

/// Rows per record batch handed to the Parquet writer.
pub const BATCH_ROWS: usize = 4096;
/// Rows per Parquet row group (bounds the writer's memory).
pub const ROW_GROUP_ROWS: usize = 65_536;

fn perr(path: &Path, e: impl std::fmt::Display) -> Error {
    Error::io(path, std::io::Error::other(e.to_string()))
}

/// Streams rows into a Parquet file under a temporary name; [`TableWriter::finish`] checks the
/// row count by reading the file's footer back and renames it into place.
pub struct TableWriter {
    cols: &'static [ColumnDef],
    schema: SchemaRef,
    builders: Vec<Builder>,
    pending: usize,
    rows: u64,
    writer: ArrowWriter<File>,
    tmp: PathBuf,
    dest: PathBuf,
}

impl std::fmt::Debug for TableWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TableWriter")
            .field("dest", &self.dest)
            .field("rows", &self.rows)
            .finish_non_exhaustive()
    }
}

impl TableWriter {
    /// Start writing `dest` (Snappy-compressed; `created_by` names the tool).
    pub fn create(dest: &Path, cols: &'static [ColumnDef], created_by: &str) -> Result<Self> {
        let tmp = dest.with_extension("parquet.tmp");
        let file = File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
        let schema = schema_of(cols);
        let props = WriterProperties::builder()
            .set_compression(Compression::SNAPPY)
            .set_max_row_group_row_count(Some(ROW_GROUP_ROWS))
            .set_created_by(created_by.to_string())
            .build();
        let writer =
            ArrowWriter::try_new(file, schema.clone(), Some(props)).map_err(|e| perr(&tmp, e))?;
        Ok(TableWriter {
            cols,
            schema,
            builders: cols.iter().map(|c| Builder::new(c.kind)).collect(),
            pending: 0,
            rows: 0,
            writer,
            tmp,
            dest: dest.to_path_buf(),
        })
    }

    /// Append one row (cells in column order).
    pub fn push(&mut self, row: &[Cell]) -> Result<()> {
        debug_assert_eq!(row.len(), self.cols.len(), "row width");
        for (b, c) in self.builders.iter_mut().zip(row) {
            b.push(c);
        }
        self.pending += 1;
        self.rows += 1;
        if self.pending >= BATCH_ROWS {
            self.flush_batch()?;
        }
        Ok(())
    }

    fn flush_batch(&mut self) -> Result<()> {
        if self.pending == 0 {
            return Ok(());
        }
        let arrays: Vec<ArrayRef> = self.builders.iter_mut().map(Builder::finish).collect();
        let batch =
            RecordBatch::try_new(self.schema.clone(), arrays).map_err(|e| perr(&self.tmp, e))?;
        self.writer.write(&batch).map_err(|e| perr(&self.tmp, e))?;
        self.pending = 0;
        Ok(())
    }

    /// Rows written so far.
    pub fn rows(&self) -> u64 {
        self.rows
    }

    /// Close, verify the row count from the footer, rename into place. Returns the rows.
    pub fn finish(mut self) -> Result<u64> {
        self.flush_batch()?;
        let meta = self.writer.close().map_err(|e| perr(&self.tmp, e))?;
        let written: i64 = meta.file_metadata().num_rows();
        let f = File::open(&self.tmp).map_err(|e| Error::io(&self.tmp, e))?;
        let back = ParquetRecordBatchReaderBuilder::try_new(f).map_err(|e| perr(&self.tmp, e))?;
        let read_rows = back.metadata().file_metadata().num_rows();
        if u64::try_from(written).ok() != Some(self.rows) || read_rows != written {
            return Err(Error::Other(format!(
                "{}: wrote {} rows but the footer reads back {read_rows}",
                self.tmp.display(),
                self.rows
            )));
        }
        std::fs::rename(&self.tmp, &self.dest).map_err(|e| Error::io(&self.dest, e))?;
        Ok(self.rows)
    }
}

/// A batch of rows read back from a table.
#[derive(Debug)]
pub struct Rows {
    names: Vec<String>,
    arrays: Vec<ArrayRef>,
    len: usize,
}

impl Rows {
    /// Number of rows.
    pub fn len(&self) -> usize {
        self.len
    }
    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Column position by name.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n == name)
    }
    /// Column names.
    pub fn names(&self) -> &[String] {
        &self.names
    }
    /// The value at (`row`, column `col`).
    pub fn cell(&self, row: usize, col: usize) -> Cell {
        let a = &self.arrays[col];
        if a.is_null(row) {
            return Cell::Null;
        }
        match a.data_type() {
            DataType::Utf8 => Cell::Str(a.as_string::<i32>().value(row).to_string()),
            DataType::UInt64 => Cell::U64(a.as_primitive::<UInt64Type>().value(row)),
            DataType::Int64 => Cell::I64(a.as_primitive::<Int64Type>().value(row)),
            DataType::Float64 => Cell::F64(a.as_primitive::<Float64Type>().value(row)),
            DataType::Boolean => Cell::Bool(a.as_boolean().value(row)),
            DataType::Timestamp(TimeUnit::Microsecond, _) => {
                Cell::Time(a.as_primitive::<TimestampMicrosecondType>().value(row))
            }
            DataType::List(f) => {
                let l = a.as_list::<i32>().value(row);
                match f.data_type() {
                    DataType::Utf8 => {
                        let s = l.as_string::<i32>();
                        Cell::List(
                            (0..s.len())
                                .filter(|&i| !s.is_null(i))
                                .map(|i| s.value(i).to_string())
                                .collect(),
                        )
                    }
                    DataType::Int64 => {
                        let s = l.as_primitive::<Int64Type>();
                        Cell::IntList(
                            (0..s.len())
                                .filter(|&i| !s.is_null(i))
                                .map(|i| s.value(i))
                                .collect(),
                        )
                    }
                    _ => Cell::Null,
                }
            }
            _ => Cell::Null,
        }
    }
    /// The value of column `name` at `row` (`Null` when there is no such column).
    pub fn get(&self, row: usize, name: &str) -> Cell {
        self.index_of(name)
            .map_or(Cell::Null, |c| self.cell(row, c))
    }
}

/// Read a table in batches, optionally only some columns. Calls `f` for each batch; stops at
/// the first error `f` returns.
pub fn scan_table(
    path: &Path,
    columns: Option<&[&str]>,
    f: &mut dyn FnMut(&Rows) -> Result<()>,
) -> Result<()> {
    let file = File::open(path).map_err(|e| Error::io(path, e))?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).map_err(|e| perr(path, e))?;
    let schema = builder.schema().clone();
    let builder = if let Some(cols) = columns {
        let idx: Vec<usize> = cols
            .iter()
            .filter_map(|c| schema.index_of(c).ok())
            .collect();
        let mask = ProjectionMask::roots(builder.parquet_schema(), idx);
        builder.with_projection(mask)
    } else {
        builder
    };
    let reader = builder
        .with_batch_size(BATCH_ROWS)
        .build()
        .map_err(|e| perr(path, e))?;
    for batch in reader {
        let batch = batch.map_err(|e| perr(path, e))?;
        let names = batch
            .schema()
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect();
        let rows = Rows {
            names,
            len: batch.num_rows(),
            arrays: batch.columns().to_vec(),
        };
        f(&rows)?;
    }
    Ok(())
}

/// Row count of a table from its footer.
pub fn row_count(path: &Path) -> Result<u64> {
    let file = File::open(path).map_err(|e| Error::io(path, e))?;
    let b = ParquetRecordBatchReaderBuilder::try_new(file).map_err(|e| perr(path, e))?;
    Ok(u64::try_from(b.metadata().file_metadata().num_rows()).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_record_fills_every_column() {
        let r = Record {
            path: "/data/a.czi".into(),
            format: Some("czi".into()),
            ..Record::default()
        };
        assert_eq!(experiment_row(&r, None).len(), EXPERIMENTS.len());
        assert_eq!(
            file_row(
                "/a",
                None,
                "unknown",
                ItemKind::File,
                1,
                None,
                None,
                None,
                "new"
            )
            .len(),
            FILES.len()
        );
        assert_eq!(
            problem_row("/a", None, "walk", "error", "x", "y", None, None).len(),
            PROBLEMS.len()
        );
        let mut names: Vec<&str> = EXPERIMENTS.iter().map(|c| c.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), EXPERIMENTS.len(), "column names are unique");
    }

    #[test]
    fn round_trip_through_parquet() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("files.parquet");
        let mut w = TableWriter::create(&p, FILES, "test").unwrap();
        for i in 0..5000u64 {
            w.push(&file_row(
                &format!("/r/f{i}"),
                (i % 2 == 0).then_some("/r/ds"),
                "member",
                ItemKind::File,
                i,
                Some(1_600_000_000_000_000),
                Some("czi"),
                None,
                "new",
            ))
            .unwrap();
        }
        assert_eq!(w.finish().unwrap(), 5000);
        assert!(!d.path().join("files.parquet.tmp").exists());
        assert_eq!(row_count(&p).unwrap(), 5000);
        let mut seen = 0usize;
        scan_table(
            &p,
            Some(&["path", "size_bytes", "mtime", "dataset_path"]),
            &mut |rows| {
                for r in 0..rows.len() {
                    let i = seen + r;
                    assert_eq!(rows.get(r, "path"), Cell::Str(format!("/r/f{i}")));
                    assert_eq!(rows.get(r, "size_bytes"), Cell::U64(i as u64));
                    assert_eq!(rows.get(r, "mtime").to_text(), "2020-09-13T12:26:40.000Z");
                    assert_eq!(rows.get(r, "role"), Cell::Null, "not projected");
                    if i % 2 == 1 {
                        assert_eq!(rows.get(r, "dataset_path"), Cell::Null);
                    }
                }
                seen += rows.len();
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(seen, 5000);
    }
}
