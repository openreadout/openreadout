//! `index`, `search`, `health` and `export-dataset`: a catalog of a lab share
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
    #[arg(long)]
    pub json: bool,
}

/// `--tables` of `export-dataset`.
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
    /// Most results (0 = all; default 50).
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
}

/// Arguments of `health`.
#[derive(Debug, clap::Args)]
pub struct HealthArgs {
    /// Index directory (from `openreadout index DIR -o INDEX_DIR`).
    #[arg(value_name = "INDEX_DIR")]
    pub index: PathBuf,
    /// Do not confirm duplicate candidates by hashing their content (no file is read).
    #[arg(long)]
    pub no_hash: bool,
    /// Most entries listed per section (counts are complete).
    #[arg(long, value_name = "N", default_value_t = 100)]
    pub max_list: usize,
    /// Also write the Markdown report to this file.
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,
    #[arg(long)]
    pub json: bool,
}

/// Arguments of `export-dataset`.
#[derive(Debug, clap::Args)]
pub struct ExportDatasetArgs {
    /// Index directory (from `openreadout index DIR -o INDEX_DIR`).
    #[arg(value_name = "INDEX_DIR")]
    pub index: PathBuf,
    /// The data sets to export, as a `search` query (`""` for everything).
    #[arg(value_name = "QUERY")]
    pub query: String,
    /// Directory to export into: images to OME-Zarr, tables, traces and spectra to Parquet (or
    /// CSV), metadata JSON and a datasheet. A rerun continues where the last one stopped.
    #[arg(value_name = "OUT")]
    pub output: PathBuf,
    /// Most data sets exported. Default: all.
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,
    /// File format for tables, traces and spectra.
    #[arg(long, value_enum, default_value_t)]
    pub tables: TablesArg,
    /// Replace values flagged as personal data with stable salted hashes (needs a salt:
    /// `--salt-file` or OPENREADOUT_REDACT_SALT).
    #[arg(long)]
    pub redact: bool,
    /// File holding the redaction salt (never printed or stored).
    #[arg(long, value_name = "FILE", requires = "redact")]
    pub salt_file: Option<PathBuf>,
    /// Licence of the whole export (SPDX id); default: the nearest LICENSE file of each source.
    #[arg(long, value_name = "SPDX")]
    pub license: Option<String>,
    /// Do not export images (metadata, tables, traces and spectra only).
    #[arg(long)]
    pub no_images: bool,
    #[arg(long)]
    pub json: bool,
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
        Ok(m) => emit(a.json, &m, render_manifest),
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
pub fn search(a: &SearchArgs) -> i32 {
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

/// `openreadout health`.
pub fn health(a: &HealthArgs) -> i32 {
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

fn export_options(a: &ExportDatasetArgs) -> Result<ExportDatasetOptions> {
    let mut o = ExportDatasetOptions::default();
    o.query.clone_from(&a.query);
    o.output.clone_from(&a.output);
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

/// `openreadout export-dataset`.
pub fn export_dataset(reg: &Registry, a: &ExportDatasetArgs) -> i32 {
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
