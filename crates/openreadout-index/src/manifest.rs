//! `index.json`: what an index directory holds, how it was made and what the crawl found. It is
//! also the payload of `openreadout index --json` and of the MCP tool `openreadout_index`.

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::tables::ColumnDef;
use crate::walk::WalkStats;

/// Version of the index layout (tables, columns, `index.json`). Additive changes keep it.
pub const INDEX_SCHEMA_VERSION: &str = "1";

/// File name of the manifest.
pub const MANIFEST: &str = "index.json";
/// File name of the data-set table.
pub const EXPERIMENTS_FILE: &str = "experiments.parquet";
/// File name of the file table.
pub const FILES_FILE: &str = "files.parquet";
/// File name of the problem table.
pub const PROBLEMS_FILE: &str = "problems.parquet";

/// Counters of one crawl (cumulative over resumed sessions of the same run).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CrawlStats {
    /// Walk items processed: files and directory data sets.
    pub items: u64,
    /// Files seen, counting every file inside directory data sets.
    pub files_seen: u64,
    /// Items a reader recognised (before multi-file grouping).
    pub recognised: u64,
    /// Items no reader recognises.
    pub unknown: u64,
    /// Recognised items that could not be opened or summarized.
    pub unreadable: u64,
    /// Items read for the first time.
    pub new: u64,
    /// Items read again (size, time or tool version changed).
    pub changed: u64,
    /// Items reused from the previous run without being opened.
    pub unchanged: u64,
    /// Items of the previous run that are gone.
    pub removed: u64,
    /// Bytes of every file seen.
    pub bytes_seen: u64,
    /// Bytes the crawler itself read (sniffing and fingerprints); the readers' own header reads
    /// are not counted.
    pub bytes_read: u64,
    /// Wall time of the crawl, seconds (all sessions of a resumed run).
    pub elapsed_s: f64,
    /// Sessions this run took (1 plus the number of resumes).
    pub sessions: u32,
    /// What the walk skipped.
    pub walk: WalkStats,
}

/// A count and a byte total.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct Tally {
    /// How many.
    pub count: u64,
    /// Bytes.
    pub bytes: u64,
}

impl Tally {
    /// Add one item of `bytes`.
    pub fn add(&mut self, bytes: u64) {
        self.count += 1;
        self.bytes = self.bytes.saturating_add(bytes);
    }
}

/// One table of the index.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TableManifest {
    /// File name inside the index directory.
    pub file: String,
    /// Rows.
    pub rows: u64,
    /// Columns (name, type, unit class, description). Always in `index.json`; left out of the
    /// MCP tool's answer (listed in `index.json` and `book/src/guides/lab-shares.md`).
    #[schemars(with = "Vec<serde_json::Value>")]
    #[serde(default, skip_deserializing, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<ColumnDef>,
}

/// Personal-data totals.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PiiTotals {
    /// Data sets with at least one flag.
    pub datasets_flagged: u64,
    /// Flags.
    pub flags: u64,
    /// Flags per kind (`person_name`, `email`, ...).
    pub by_kind: BTreeMap<String, u64>,
}

/// The tool that wrote the index.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ToolInfo {
    /// `openreadout`.
    pub name: String,
    /// Its version.
    pub version: String,
}

/// Crawl options that shape the index.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct IndexSettings {
    /// `headers`, `full` or `none`.
    pub check: String,
    /// Symbolic links followed.
    pub follow_symlinks: bool,
    /// `--exclude` patterns.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Personal-data detection on.
    pub pii: bool,
}

/// `index.json`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct IndexManifest {
    /// [`INDEX_SCHEMA_VERSION`].
    pub schema_version: String,
    /// The tool and version that wrote it.
    pub tool: ToolInfo,
    /// Absolute path of the index directory.
    pub index_dir: String,
    /// The directories (or files) crawled, absolute.
    pub roots: Vec<String>,
    /// Options of the crawl.
    pub settings: IndexSettings,
    /// True when the crawl walked every root to the end. False after a cap (`--max-files`,
    /// `--max-seconds`) or an interruption: the tables then hold what was read so far plus the
    /// previous run's records for the rest, and rerunning the same command continues.
    pub complete: bool,
    /// When this run started (ISO-8601 UTC).
    pub started_at: String,
    /// When the tables were written.
    pub updated_at: String,
    /// The tables.
    pub tables: BTreeMap<String, TableManifest>,
    /// Data sets (rows of `experiments.parquet`) after multi-file grouping.
    pub datasets: u64,
    /// Rows of `files.parquet`.
    pub files: u64,
    /// Bytes of every file seen.
    pub bytes: u64,
    /// Data sets and bytes per format id.
    pub formats: BTreeMap<String, Tally>,
    /// Data sets and bytes per family.
    pub families: BTreeMap<String, Tally>,
    /// Data sets per acquisition year (`unknown` when the file records no date).
    pub years: BTreeMap<String, Tally>,
    /// Files no reader recognises, per lower-case extension (`(none)` for no extension).
    pub unknown_extensions: BTreeMap<String, Tally>,
    /// Data sets per check status.
    pub check_status: BTreeMap<String, u64>,
    /// Problems per category (`integrity`, `readability`, `walk`, `pii`).
    pub problems: BTreeMap<String, u64>,
    /// Personal-data totals.
    pub pii: PiiTotals,
    /// Data sets per change (`new`, `changed`, `unchanged`, `moved`) and `removed`.
    pub changes: BTreeMap<String, u64>,
    /// Crawl counters, cumulative over resumed sessions.
    pub crawl: CrawlStats,
    /// Items per second over the crawl's wall time.
    pub files_per_s: f64,
    /// What to do next, when the crawl is not complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

/// Read `index.json` from an index directory.
pub fn read_manifest(index_dir: &Path) -> Result<IndexManifest> {
    let p = index_dir.join(MANIFEST);
    let text = std::fs::read_to_string(&p).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::Usage(format!(
                "{} is not an index directory (no {MANIFEST}); build one with `openreadout index DIR -o {}`",
                index_dir.display(),
                index_dir.display()
            ))
        } else {
            Error::io(&p, e)
        }
    })?;
    let mut m: IndexManifest = serde_json::from_str(&text)
        .map_err(|e| Error::Other(format!("{}: not a valid index manifest: {e}", p.display())))?;
    if m.schema_version != INDEX_SCHEMA_VERSION {
        return Err(Error::unsupported(
            "index",
            format!("index schema version {}", m.schema_version),
            "Rebuild the index with this version: `openreadout index ROOT -o INDEX_DIR --restart`.",
        ));
    }
    for (name, t) in &mut m.tables {
        t.columns = crate::tables_named(name).to_vec();
    }
    Ok(m)
}
