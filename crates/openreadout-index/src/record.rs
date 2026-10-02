//! What the crawl records per walk item: one [`Record`] per file or directory data set (and per
//! file no reader recognises). Records are journaled as JSON Lines while crawling and flattened
//! into the Parquet tables ([`crate::tables`]) when the run is finalized.

use std::collections::BTreeSet;

use openreadout_core::model::{CheckReport, FileInfo, Severity};
use openreadout_core::{Error, Experiment};
use serde::{Deserialize, Serialize};

/// Whether a walk item is a file or a directory data set (a Bruker `.d`, a ChemStation `.D`, a
/// Waters `.raw`, a TopSpin experiment, an OME-Zarr store, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// A regular file.
    #[default]
    File,
    /// A directory a reader recognises as one data set.
    Directory,
}

/// How a record relates to the previous complete run of the same index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    /// Not in the previous run.
    #[default]
    New,
    /// In the previous run with another size, modification time or tool version: read again.
    Changed,
    /// Same size, modification time and tool version: the previous record was reused without
    /// opening the file.
    Unchanged,
    /// Not in the previous run under this path, but a file of the same size and content
    /// fingerprint disappeared from another path (`moved_from`).
    Moved,
}

impl Change {
    /// The snake_case name used in the tables.
    pub fn as_str(self) -> &'static str {
        match self {
            Change::New => "new",
            Change::Changed => "changed",
            Change::Unchanged => "unchanged",
            Change::Moved => "moved",
        }
    }
}

/// A file that belongs to a data set: inside a directory data set, or named by the reader as
/// part of a multi-file data set ([`openreadout_core::Dataset::member_files`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    /// Absolute path.
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    /// Modification time, microseconds since the Unix epoch (UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtime_us: Option<i64>,
}

/// Why a data set could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorInfo {
    /// Stable error code (`corrupt_file`, `unsupported_feature`, `io`, ...).
    pub code: String,
    /// Process exit code the CLI would return (4 corrupt, 5 I/O, 6 unsupported, ...).
    pub exit_code: i32,
    /// Human-readable message.
    pub message: String,
    /// What to do about it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl ErrorInfo {
    /// From a reader error.
    pub fn from_error(e: &Error) -> Self {
        ErrorInfo {
            code: e.code().to_string(),
            exit_code: e.exit_code(),
            message: e.to_string(),
            hint: e.hint(),
        }
    }
}

/// One finding of the integrity check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckFinding {
    /// `error`, `warning` or `info`.
    pub severity: String,
    /// Stable finding code (`truncated`, `missing_part`, ...).
    pub code: String,
    /// Human-readable message.
    pub message: String,
}

/// Most findings kept per record (the count is kept in full).
pub const MAX_FINDINGS: usize = 20;

/// Integrity status of a data set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CheckSummary {
    /// `ok`, `warning` (only warnings), `truncated` (an error that says data are missing:
    /// `truncated`, `missing_part`, `missing_pixels`, `unfinished_write`), `corrupt` (any other
    /// error), `unreadable` (the file could not be opened), `unsupported` (known format,
    /// unsupported feature) or `not_checked`.
    pub status: String,
    /// `headers` or `full`: which check ran ([`crate::CheckMode`]).
    pub mode: String,
    /// Number of findings (all severities).
    pub finding_count: u32,
    /// The first [`MAX_FINDINGS`] findings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<CheckFinding>,
}

const TRUNCATION_CODES: &[&str] = &[
    "truncated",
    "missing_part",
    "missing_pixels",
    "missing_planes",
    "missing_data",
    "short_file",
    "unfinished_write",
];

impl CheckSummary {
    /// Summarize a reader's check report.
    pub fn from_report(r: &CheckReport, mode: &str) -> Self {
        let is_truncation = |code: &str| {
            let c = code.to_ascii_lowercase();
            TRUNCATION_CODES.iter().any(|t| c.contains(t))
        };
        let errors: Vec<_> = r
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .collect();
        let status = if errors.iter().any(|f| is_truncation(&f.code)) {
            "truncated"
        } else if !errors.is_empty() {
            "corrupt"
        } else if r
            .findings
            .iter()
            .any(|f| f.severity == Severity::Warning && is_truncation(&f.code))
        {
            // An unfinished write reported as a warning (CZI `update_pending`) still means data
            // may be missing.
            "truncated"
        } else if r.findings.iter().any(|f| f.severity == Severity::Warning) {
            "warning"
        } else {
            "ok"
        };
        CheckSummary {
            status: status.into(),
            mode: mode.into(),
            finding_count: u32::try_from(r.findings.len()).unwrap_or(u32::MAX),
            findings: r
                .findings
                .iter()
                .take(MAX_FINDINGS)
                .map(|f| CheckFinding {
                    severity: f.severity.as_str().into(),
                    code: f.code.clone(),
                    message: f.message.clone(),
                })
                .collect(),
        }
    }

    /// The status for a data set that could not be opened or summarized.
    pub fn from_error(e: &ErrorInfo, mode: &str) -> Self {
        let status = match e.exit_code {
            4 => {
                let m = e.message.to_ascii_lowercase();
                if m.contains("truncat") || m.contains("end of file") || m.contains("too short") {
                    "truncated"
                } else {
                    "corrupt"
                }
            }
            6 => "unsupported",
            _ => "unreadable",
        };
        CheckSummary {
            status: status.into(),
            mode: mode.into(),
            finding_count: 1,
            findings: vec![CheckFinding {
                severity: "error".into(),
                code: e.code.clone(),
                message: e.message.clone(),
            }],
        }
    }
}

/// One personal-data flag: which field, what kind, which rule. The value itself is never
/// stored in the index.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct PiiFlag {
    /// Path of the field in the `info` JSON (`experiment.acquisition.operator`,
    /// `tables[0].extra.vendor_keywords.EXPORT.EXPORT USER NAME`).
    pub field: String,
    /// `person_name`, `email`, `phone`, `patient_id`, `date_of_birth` or `free_text`.
    pub kind: String,
    /// The rule that fired (`book/src/guides/lab-shares.md#personal-data`).
    pub rule: String,
}

/// Numbers and names summarizing what a data set holds (from `info`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[allow(missing_docs)] // field names are the column names documented in `tables`
pub struct Summary {
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub image_count: u32,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub plane_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_x: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_y: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_z: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_c: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_t: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pixel_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub physical_size_x_um: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub physical_size_y_um: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub physical_size_z_um: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_increment_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective_magnification: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective_na: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub immersion: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mosaic_tiles: Option<u32>,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub table_count: u32,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub table_rows: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub table_columns: Vec<String>,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub trace_count: u32,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub trace_channels: u32,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub trace_sweeps: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub trace_samples: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate_hz: Option<f64>,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub spectra_runs: u32,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub scan_count: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ms_levels: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rt_min_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rt_max_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acquired_at: Option<String>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// Most distinct channel or column names kept per record.
pub const MAX_NAMES: usize = 64;

fn push_unique(v: &mut Vec<String>, s: &str) {
    let s = s.trim();
    if !s.is_empty() && v.len() < MAX_NAMES && !v.iter().any(|x| x == s) {
        v.push(s.to_string());
    }
}

fn min_opt(a: Option<f64>, b: f64) -> f64 {
    a.map_or(b, |a| a.min(b))
}

fn max_opt(a: Option<f64>, b: f64) -> f64 {
    a.map_or(b, |a| a.max(b))
}

impl Summary {
    /// Summarize `info`: the largest image (by pixels per plane) gives the dimensions and
    /// optics; counts are summed over images, tables, traces and spectra runs.
    pub fn from_info(info: &FileInfo) -> Self {
        let mut s = Summary {
            image_count: u32::try_from(info.images.len()).unwrap_or(u32::MAX),
            plane_count: info.plane_count,
            ..Summary::default()
        };
        if let Some(im) = info.images.iter().max_by_key(|i| {
            (
                u64::from(i.size_x) * u64::from(i.size_y),
                u64::MAX - u64::from(i.index),
            )
        }) {
            s.size_x = Some(im.size_x);
            s.size_y = Some(im.size_y);
            s.size_z = Some(im.size_z);
            s.size_c = Some(im.size_c);
            s.size_t = Some(im.size_t);
            s.pixel_type = serde_json::to_value(im.pixel_type)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string));
            // readers normalize in `ImageInfo::finish`; also here for any that set sizes later
            let r = |v: Option<f64>| v.map(openreadout_core::model::round_noise);
            s.physical_size_x_um = r(im.physical_size.x);
            s.physical_size_y_um = r(im.physical_size.y);
            s.physical_size_z_um = r(im.physical_size.z);
            s.time_increment_s = r(im.time_increment_s);
            s.mosaic_tiles = im.mosaic.as_ref().map(|m| m.tile_count);
        }
        for im in &info.images {
            for c in &im.channels {
                if let Some(n) = &c.name {
                    push_unique(&mut s.channels, n);
                }
                if let Some(f) = &c.fluorophore {
                    push_unique(&mut s.channels, f);
                }
            }
            if let Some(o) = &im.objective {
                if s.objective.is_none() {
                    s.objective.clone_from(&o.model);
                }
                if s.objective_magnification.is_none() {
                    s.objective_magnification = o.nominal_magnification;
                }
                if s.objective_na.is_none() {
                    s.objective_na = o.lens_na;
                }
                if s.immersion.is_none() {
                    s.immersion.clone_from(&o.immersion);
                }
            }
            if s.acquired_at.is_none() {
                s.acquired_at.clone_from(&im.acquired_at);
            }
        }
        s.table_count = u32::try_from(info.tables.len()).unwrap_or(u32::MAX);
        for t in &info.tables {
            s.table_rows = s.table_rows.saturating_add(t.row_count);
            for c in &t.columns {
                push_unique(&mut s.table_columns, &c.name);
                if let Some(l) = &c.label {
                    push_unique(&mut s.channels, l);
                }
            }
        }
        s.trace_count = u32::try_from(info.traces.len()).unwrap_or(u32::MAX);
        for t in &info.traces {
            s.trace_channels = s
                .trace_channels
                .saturating_add(u32::try_from(t.channels.len()).unwrap_or(u32::MAX));
            s.trace_sweeps = s.trace_sweeps.saturating_add(u64::from(t.sweep_count));
            s.trace_samples = s.trace_samples.saturating_add(
                t.sample_count
                    .saturating_mul(u64::from(t.sweep_count.max(1))),
            );
            if t.sample_rate_hz.is_finite() && t.sample_rate_hz > 0.0 {
                s.sample_rate_hz = Some(max_opt(s.sample_rate_hz, t.sample_rate_hz));
            }
        }
        s.spectra_runs = u32::try_from(info.spectra.len()).unwrap_or(u32::MAX);
        let mut levels = BTreeSet::new();
        for r in &info.spectra {
            s.scan_count = s.scan_count.saturating_add(r.scan_count);
            levels.extend(r.ms_levels.iter().copied());
            if let Some([a, b]) = r.rt_range_s {
                s.rt_min_s = Some(min_opt(s.rt_min_s, a));
                s.rt_max_s = Some(max_opt(s.rt_max_s, b));
            }
        }
        s.ms_levels = levels.into_iter().collect();
        s
    }

    /// Add the counts of another stream of the same series (a SpikeGLX LF band next to the AP
    /// band, a Blackrock `.nev` next to its `.ns5`).
    pub fn add(&mut self, o: &Summary) {
        self.image_count = self.image_count.saturating_add(o.image_count);
        self.plane_count = self.plane_count.saturating_add(o.plane_count);
        self.table_count = self.table_count.saturating_add(o.table_count);
        self.table_rows = self.table_rows.saturating_add(o.table_rows);
        for c in &o.table_columns {
            push_unique(&mut self.table_columns, c);
        }
        for c in &o.channels {
            push_unique(&mut self.channels, c);
        }
        self.trace_count = self.trace_count.saturating_add(o.trace_count);
        self.trace_channels = self.trace_channels.saturating_add(o.trace_channels);
        self.trace_sweeps = self.trace_sweeps.saturating_add(o.trace_sweeps);
        self.trace_samples = self.trace_samples.saturating_add(o.trace_samples);
        if let Some(r) = o.sample_rate_hz {
            self.sample_rate_hz = Some(max_opt(self.sample_rate_hz, r));
        }
        self.spectra_runs = self.spectra_runs.saturating_add(o.spectra_runs);
        self.scan_count = self.scan_count.saturating_add(o.scan_count);
        for l in &o.ms_levels {
            if !self.ms_levels.contains(l) {
                self.ms_levels.push(*l);
            }
        }
        self.ms_levels.sort_unstable();
        if self.acquired_at.is_none() {
            self.acquired_at.clone_from(&o.acquired_at);
        }
    }
}

/// One walk item as crawled: a data set, or a file no reader recognises.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Record {
    /// Absolute path (non-UTF-8 names are converted lossily).
    pub path: String,
    /// The crawl root it was found under (absolute).
    pub root: String,
    /// File or directory data set.
    pub kind: ItemKind,
    /// Size in bytes: the file, or every file of a directory data set.
    pub size: u64,
    /// Modification time, microseconds since the Unix epoch (UTC); the newest file of a
    /// directory data set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtime_us: Option<i64>,
    /// Content fingerprint: xxh3-128 of the size and the first and last 64 KiB (of the largest
    /// file, plus every member's name and size, for directory data sets). 32 hex digits.
    /// Absent for unknown files larger than 64 KiB (only their head is read).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    /// Lower-case extension (`czi`, `ome.tiff` is `tiff`); empty when none.
    #[serde(default)]
    pub ext: String,
    /// Format id (`czi`, `thermo-raw`, ...); `None` when no reader recognises the item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// Format name (not journaled: filled from the registry when the tables are written).
    #[serde(skip)]
    pub format_name: Option<String>,
    /// Format family (`microscopy`, `mass-spectrometry`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    /// The format's vendor or standards body (not journaled, like `format_name`).
    #[serde(skip)]
    pub format_vendor: Option<String>,
    /// Detection confidence (`definite`, `likely`, `extension_only`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
    /// Detection note (e.g. an OIR continuation file).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detect_note: Option<String>,
    /// Format version, when the reader reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format_version: Option<String>,
    /// Files of this data set other than `path` (directory contents, or the reader's
    /// [`openreadout_core::Dataset::member_files`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<Member>,
    /// Stream series this data set belongs to (a SpikeGLX run `<dir>/<run>_g0_t0`, a Blackrock
    /// recording `<dir>/<stem>`): records with the same key become one data set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_key: Option<String>,
    /// A file an earlier data set already named as its member: only sniffed and fingerprinted,
    /// not opened (it becomes a `member` row of that data set).
    #[serde(default, skip_serializing_if = "is_default")]
    pub member_only: bool,
    /// Why it could not be opened or summarized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorInfo>,
    /// Integrity status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<CheckSummary>,
    /// Assurance level (`validated`, `partially_validated`, `unvalidated`): whether the file
    /// lies inside what its reader was validated on (docs/assurance.md).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assurance: Option<String>,
    /// The file's variant fingerprint (`assurance.fingerprint`): files with the same value are
    /// the same variant of their format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// Counts, dimensions and names.
    #[serde(default, skip_serializing_if = "is_default")]
    pub summary: Summary,
    /// Sample, instrument, method, acquisition and measurements.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experiment: Option<Experiment>,
    /// Reader notes (first few).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// Personal-data flags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pii: Vec<PiiFlag>,
    /// Relation to the previous run.
    #[serde(default, skip_serializing_if = "is_default")]
    pub change: Change,
    /// Previous path of a moved file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved_from: Option<String>,
    /// When the item was read (ISO-8601 UTC).
    #[serde(default)]
    pub indexed_at: String,
    /// `openreadout` version that read it (a new version re-reads every item).
    #[serde(default)]
    pub tool_version: String,
    /// Bytes the crawler itself read (sniffing and fingerprinting; not the readers' header
    /// reads).
    #[serde(default, skip_serializing_if = "is_default")]
    pub bytes_read: u64,
    /// Wall time spent on the item, milliseconds.
    #[serde(default, skip_serializing_if = "is_default")]
    pub elapsed_ms: f64,
}

impl Record {
    /// True when a reader recognised the item.
    pub fn is_dataset(&self) -> bool {
        self.format.is_some()
    }

    /// Total size including member files.
    pub fn total_size(&self) -> u64 {
        match self.kind {
            ItemKind::Directory => self.size,
            ItemKind::File => self
                .members
                .iter()
                .fold(self.size, |a, m| a.saturating_add(m.size)),
        }
    }

    /// Integrity status (`not_checked` when no check ran).
    pub fn check_status(&self) -> &str {
        self.check
            .as_ref()
            .map_or("not_checked", |c| c.status.as_str())
    }
}
