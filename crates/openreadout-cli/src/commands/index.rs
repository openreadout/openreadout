//! `index` and `search` (with `--health` and `--export`): a catalog of a lab share
//! (`openreadout-index`, `book/src/guides/lab-shares.md`).

use std::path::PathBuf;

use openreadout_core::{Registry, Result};
use openreadout_index::export::TableFormat;
use openreadout_index::health::{human_bytes, render_markdown};
use openreadout_index::{
    CheckMode, ExportDatasetOptions, HealthOptions, IndexManifest, IndexOptions, SearchOutput,
    SearchRequest,
};

use crate::output::{emit, fail};
use crate::ui::{self, Align, Progress};

/// `--check` of `index`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum CheckArg {
    /// Headers and structure only (fast; the default).
    #[default]
    Headers,
    /// The full `check` (may decode data; slower).
    Full,
    /// No integrity check.
    None,
}

/// Arguments of `index`.
#[derive(Debug, clap::Args)]
pub struct IndexArgs {
    /// Directories (or files) to crawl.
    #[arg(required = true, value_name = "DIR")]
    pub roots: Vec<PathBuf>,
    /// Index directory: `index.json`, `experiments.parquet`, `files.parquet`,
    /// `problems.parquet` and crawl state. Created if missing; must not be inside a root.
    #[arg(short, long, value_name = "INDEX_DIR")]
    pub output: PathBuf,
    /// Integrity check per data set.
    #[arg(long, value_enum, default_value_t)]
    pub check: CheckArg,
    /// Follow symbolic links (a directory reached twice is crawled once).
    #[arg(long)]
    pub follow_symlinks: bool,
    /// Skip entries whose name or root-relative path matches this glob (repeatable), e.g.
    /// `--exclude '*.tmp' --exclude 'scratch/**'`.
    #[arg(long, value_name = "GLOB")]
    pub exclude: Vec<String>,
    /// Stop after this many files in this session (rerun to continue).
    #[arg(long, value_name = "N")]
    pub max_files: Option<u64>,
    /// Stop after this many seconds in this session (rerun to continue).
    #[arg(long, value_name = "S")]
    pub max_seconds: Option<f64>,
    /// Discard an interrupted crawl instead of resuming it.
    #[arg(long)]
    pub restart: bool,
    /// Read every file again instead of reusing unchanged records of the previous run.
    #[arg(long)]
    pub full_rescan: bool,
    /// Do not look for personal data.
    #[arg(long)]
    pub no_pii: bool,
    /// Also print the storage health report (as `search --health` does).
    #[arg(long)]
    pub health: bool,
    #[arg(long)]
    pub json: bool,
}

/// `--tables` of `search --export`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum TablesArg {
    /// Apache Parquet.
    #[default]
    Parquet,
    /// Comma-separated values.
    Csv,
}

/// Arguments of `search`.
#[derive(Debug, clap::Args)]
pub struct SearchArgs {
    /// Index directory (from `openreadout index DIR -o INDEX_DIR`).
    #[arg(value_name = "INDEX_DIR")]
    pub index: PathBuf,
    /// Query, e.g. `"objective=63x channel~GFP acquired<2020 format=czi size>1GB"`. Terms are
    /// ANDed; `OR` separates alternatives; `field=v`, `!=`, `~` (contains), `<`, `<=`, `>`,
    /// `>=`, `field:TERM_ID`, `-term` negates, `a|b` alternatives, bare words search paths,
    /// samples, channels and descriptions. Empty: everything. See <https://openreadout.github.io/openreadout/guides/lab-shares.html>.
    #[arg(value_name = "QUERY", default_value = "")]
    pub query: String,
    /// Sort by a field (`size`, `acquired`, `-size` for descending). Default: path order.
    #[arg(long, value_name = "FIELD", allow_hyphen_values = true)]
    pub sort: Option<String>,
    /// Most results (0 = all; default 50); with `--export`, most data sets exported (default
    /// all).
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,
    /// Fields to return, comma-separated (`path,format,sample,operator`), or `all`.
    #[arg(long, value_name = "FIELDS", value_delimiter = ',')]
    pub fields: Vec<String>,
    /// One JSON object per result and line (no envelope).
    #[arg(long)]
    pub jsonl: bool,
    #[arg(long)]
    pub json: bool,
    /// Storage health report of the whole index instead of results: truncated/corrupt files,
    /// unreadable formats, duplicates, the same experiment stored twice, files at risk, totals,
    /// personal data.
    #[arg(
        long,
        help_heading = "Storage health (--health)",
        conflicts_with = "export"
    )]
    pub health: bool,
    /// With `--health`: do not confirm duplicate candidates by hashing their content (no file
    /// is read).
    #[arg(long, requires = "health", help_heading = "Storage health (--health)")]
    pub no_hash: bool,
    /// With `--health`: most entries listed per section (counts are complete).
    #[arg(
        long,
        value_name = "N",
        default_value_t = 100,
        help_heading = "Storage health (--health)"
    )]
    pub max_list: usize,
    /// With `--health`: also write the Markdown report to this file.
    #[arg(
        short,
        long,
        value_name = "FILE",
        requires = "health",
        help_heading = "Storage health (--health)"
    )]
    pub output: Option<PathBuf>,
    /// Export the data sets the query selects as an ML-ready dataset into this directory:
    /// images to OME-Zarr, tables/traces/spectra to Parquet (or CSV), metadata JSON and a
    /// datasheet. Resumable (a rerun continues) and verified.
    #[arg(long, value_name = "OUT", help_heading = "Dataset export (--export)")]
    pub export: Option<PathBuf>,
    /// With `--export`: file format for tables, traces and spectra.
    #[arg(
        long,
        value_enum,
        default_value_t,
        help_heading = "Dataset export (--export)"
    )]
    pub tables: TablesArg,
    /// With `--export`: replace values flagged as personal data with stable salted hashes
    /// (needs a salt: `--salt-file` or OPENREADOUT_REDACT_SALT).
    #[arg(long, requires = "export", help_heading = "Dataset export (--export)")]
    pub redact: bool,
    /// File holding the redaction salt (never printed or stored).
    #[arg(
        long,
        value_name = "FILE",
        requires = "redact",
        help_heading = "Dataset export (--export)"
    )]
    pub salt_file: Option<PathBuf>,
    /// With `--export`: licence of the whole export (SPDX id); default: the nearest LICENSE
    /// file of each source.
    #[arg(
        long,
        value_name = "SPDX",
        requires = "export",
        help_heading = "Dataset export (--export)"
    )]
    pub license: Option<String>,
    /// With `--export`: do not export images (metadata, tables, traces and spectra only).
    #[arg(long, requires = "export", help_heading = "Dataset export (--export)")]
    pub no_images: bool,
}

fn render_manifest(m: &IndexManifest) -> String {
    let c = &m.crawl;
    let mut s = format!(
        "{} {}: {} data sets, {} files ({}) in {} item(s) of {}\n",
        if m.complete {
            "indexed"
        } else {
            "partly indexed"
        },
        m.index_dir,
        m.datasets,
        m.files,
        human_bytes(m.bytes),
        c.items,
        m.roots.join(", ")
    );
    s.push_str(&format!(
        "crawl: {:.1} s, {:.0} items/s, {} new, {} changed, {} unchanged, {} removed, {} unrecognised, {} unreadable; {} read by the crawler\n",
        c.elapsed_s,
        m.files_per_s,
        c.new,
        c.changed,
        c.unchanged,
        c.removed,
        c.unknown,
        c.unreadable,
        human_bytes(c.bytes_read)
    ));
    let rows: Vec<Vec<String>> = m
        .formats
        .iter()
        .map(|(k, t)| vec![k.clone(), t.count.to_string(), human_bytes(t.bytes)])
        .collect();
    s.push('\n');
    s.push_str(&ui::table(
        &["format", "data sets", "bytes"],
        &[Align::Left, Align::Right, Align::Right],
        &rows,
    ));
    s.push('\n');
    if !m.check_status.is_empty() {
        s.push_str(&format!(
            "\ncheck: {}\n",
            m.check_status
                .iter()
                .map(|(k, v)| format!("{k} {v}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if m.pii.flags > 0 {
        s.push_str(&format!(
            "personal data: {} flags in {} data sets ({})\n",
            m.pii.flags,
            m.pii.datasets_flagged,
            m.pii
                .by_kind
                .iter()
                .map(|(k, v)| format!("{k} {v}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(n) = &m.next {
        s.push_str(&format!("note: {n}\n"));
    }
    s.push_str(&format!(
        "\nnext: openreadout search {} \"format=czi acquired<2020\"   ·   openreadout search {} --health",
        m.index_dir, m.index_dir
    ));
    s
}

/// `openreadout index`.
#[allow(clippy::many_single_char_names)]
pub fn index(reg: &Registry, a: &IndexArgs) -> i32 {
    let mut o = IndexOptions::default();
    o.roots.clone_from(&a.roots);
    o.index_dir.clone_from(&a.output);
    o.check = match a.check {
        CheckArg::Headers => CheckMode::Headers,
        CheckArg::Full => CheckMode::Full,
        CheckArg::None => CheckMode::None,
    };
    o.follow_symlinks = a.follow_symlinks;
    o.exclude.clone_from(&a.exclude);
    o.max_files = a.max_files;
    o.max_seconds = a.max_seconds;
    o.restart = a.restart;
    o.full_rescan = a.full_rescan;
    o.pii = !a.no_pii;
    let bar = Progress::spinner("indexed");
    let cb = |p: &openreadout_index::CrawlProgress| {
        bar.set(
            p.stats.items,
            p.last.as_deref().map(|l| {
                std::path::Path::new(l)
                    .file_name()
                    .map_or(l, |n| n.to_str().unwrap_or(l))
            }),
        );
    };
    let res = openreadout_index::index(reg, &o, Some(&cb));
    bar.finish();
    match res {
        Ok(m) => {
            if a.health {
                let mut ho = HealthOptions::default();
                ho.max_list = 50;
                match openreadout_index::health(&a.output, &ho) {
                    Ok(h) if !a.json => {
                        emit(false, &m, render_manifest);
                        emit(false, &h, render_markdown);
                    }
                    Ok(h) => {
                        let v = serde_json::json!({"index": m, "health": h});
                        emit(true, &v, |_| String::new());
                    }
                    Err(e) => return fail(a.json, &e),
                }
                0
            } else {
                emit(a.json, &m, render_manifest)
            }
        }
        Err(e) => fail(a.json, &e),
    }
}

/// The directory every path lies under (paths are shown relative to it).
fn common_dir(o: &SearchOutput) -> Option<std::path::PathBuf> {
    let mut paths = o
        .results
        .iter()
        .filter_map(|r| r.get("path").and_then(|p| p.as_str()));
    let mut common = std::path::Path::new(paths.next()?).parent()?.to_path_buf();
    for p in paths {
        while !std::path::Path::new(p).starts_with(&common) {
            common = common.parent()?.to_path_buf();
        }
    }
    (common.components().count() > 1).then_some(common)
}

fn render_search(o: &SearchOutput) -> String {
    let cols: Vec<&str> = o.fields.iter().map(String::as_str).collect();
    let base = common_dir(o);
    let rows: Vec<Vec<String>> = o
        .results
        .iter()
        .map(|r| {
            cols.iter()
                .map(|c| {
                    let v = r.get(*c).cloned().unwrap_or_default();
                    let text = match (&v, *c) {
                        (serde_json::Value::Null, _) => String::new(),
                        (serde_json::Value::Number(n), "size_bytes") => {
                            human_bytes(n.as_u64().unwrap_or(0))
                        }
                        (serde_json::Value::String(s), "started_at") => {
                            s.get(..10).unwrap_or(s).to_string()
                        }
                        (serde_json::Value::String(s), "path") => base
                            .as_ref()
                            .and_then(|b| std::path::Path::new(s).strip_prefix(b).ok())
                            .map_or_else(|| s.clone(), |r| r.display().to_string()),
                        (serde_json::Value::String(s), _) => s.clone(),
                        (serde_json::Value::Array(a), _) => a
                            .iter()
                            .map(|x| x.as_str().map_or_else(|| x.to_string(), str::to_string))
                            .collect::<Vec<_>>()
                            .join(", "),
                        (other, _) => other.to_string(),
                    };
                    let max = if *c == "path" { 120 } else { 48 };
                    if text.chars().count() > max {
                        let t: String = text.chars().take(max - 1).collect();
                        format!("{t}…")
                    } else {
                        text
                    }
                })
                .collect()
        })
        .collect();
    let aligns: Vec<Align> = cols
        .iter()
        .map(|c| {
            if matches!(*c, "size_bytes" | "objective_magnification") {
                Align::Right
            } else {
                Align::Left
            }
        })
        .collect();
    let mut s = ui::table(&cols, &aligns, &rows);
    s.push_str(&ui::paint(
        ui::DIM,
        format!(
            "\n{} of {} matching data sets{}{}",
            o.returned,
            o.total,
            base.as_ref()
                .filter(|_| cols.contains(&"path"))
                .map(|b| format!(" (paths under {})", b.display()))
                .unwrap_or_default(),
            if o.index_complete {
                ""
            } else {
                " (the index is from an incomplete crawl)"
            }
        ),
    ));
    s
}

/// `openreadout search`.
pub fn search(reg: &Registry, a: &SearchArgs) -> i32 {
    if a.health {
        return health(a);
    }
    if a.export.is_some() {
        return export_dataset(reg, a);
    }
    let mut r = SearchRequest::default();
    r.query.clone_from(&a.query);
    r.sort.clone_from(&a.sort);
    r.limit = a.limit;
    r.fields.clone_from(&a.fields);
    match openreadout_index::search(&a.index, &r) {
        Ok(o) if a.jsonl => {
            for x in &o.results {
                println!("{}", serde_json::Value::Object(x.clone()));
            }
            0
        }
        Ok(o) => emit(a.json, &o, render_search),
        Err(e) => fail(a.json || a.jsonl, &e),
    }
}

/// `openreadout search --health`.
fn health(a: &SearchArgs) -> i32 {
    let mut o = HealthOptions::default();
    o.hash_duplicates = !a.no_hash;
    o.max_list = a.max_list;
    let h = match openreadout_index::health(&a.index, &o) {
        Ok(h) => h,
        Err(e) => return fail(a.json, &e),
    };
    if let Some(p) = &a.output
        && let Err(e) = std::fs::write(p, render_markdown(&h))
    {
        return fail(a.json, &openreadout_core::Error::io(p, e));
    }
    emit(a.json, &h, render_markdown)
}

fn render_export(r: &openreadout_index::ExportDatasetReport) -> String {
    let mut s = format!(
        "exported {} data sets to {} ({} now, {} resumed, {} skipped); verified={}\n",
        r.datasets.len(),
        r.output,
        r.exported_now,
        r.resumed,
        r.skipped.len(),
        r.verified
    );
    for (k, v) in &r.counts {
        s.push_str(&format!("  {k}: {v}\n"));
    }
    for sk in &r.skipped {
        s.push_str(&format!("  skipped {}: {}\n", sk.source, sk.reason));
    }
    s.push_str(&format!("datasheet: {}/DATASHEET.md", r.output));
    s
}

fn export_options(a: &SearchArgs) -> Result<ExportDatasetOptions> {
    let mut o = ExportDatasetOptions::default();
    o.query.clone_from(&a.query);
    o.output = a.export.clone().unwrap_or_default();
    o.tables = match a.tables {
        TablesArg::Parquet => TableFormat::Parquet,
        TablesArg::Csv => TableFormat::Csv,
    };
    if a.redact {
        let salt = openreadout_index::pii::load_salt(a.salt_file.as_deref())?;
        o.redact = Some(openreadout_index::pii::Redactor::new(salt));
    }
    o.license.clone_from(&a.license);
    o.limit = a.limit;
    o.no_images = a.no_images;
    Ok(o)
}

/// `openreadout search --export`.
fn export_dataset(reg: &Registry, a: &SearchArgs) -> i32 {
    let o = match export_options(a) {
        Ok(o) => o,
        Err(e) => return fail(a.json, &e),
    };
    let bar: std::sync::OnceLock<Progress> = std::sync::OnceLock::new();
    let cb = |done: u64, total: u64, path: &str| {
        bar.get_or_init(|| Progress::new(total, "data sets")).set(
            done,
            std::path::Path::new(path)
                .file_name()
                .and_then(|n| n.to_str()),
        );
    };
    let res = openreadout_index::export_dataset(reg, &a.index, &o, Some(&cb));
    if let Some(b) = bar.get() {
        b.finish();
    }
    match res {
        Ok(r) => {
            let code = emit(a.json, &r, render_export);
            if r.verified && r.skipped.is_empty() {
                code
            } else {
                1
            }
        }
        Err(e) => fail(a.json, &e),
    }
}
