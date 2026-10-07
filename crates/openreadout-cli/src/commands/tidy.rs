//! Table mode of the measurement commands (`--tidy`): one tidy table over many inputs, joined to
//! sample sheets and plate layouts, summarized by group (`--by`), written as CSV, TSV, JSON
//! Lines, JSON or Parquet. The work is done by `openreadout_batch::run_pipeline`; this module
//! parses the flags and prints.
//!
//! A command joins by implementing `openreadout_batch::Measure` and calling [`run`] when
//! [`wanted`] says table mode is on (see `book/src/guides/batch.md`).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use openreadout_batch::run::{BatchRequest, DatasetRef, IndexQuery};
use openreadout_batch::summarize::{Filter, SummarizeRequest, TestKind};
use openreadout_batch::table::{ColumnType, Row, Table, Value};
use openreadout_batch::write::write_delimited;
use openreadout_batch::{Measure, Pipeline, PipelineResult, run_pipeline};
use openreadout_core::{Error, Registry, Result};

use super::batch::BatchArgs;
use crate::output::{emit, fail};
use crate::ui::{self, Align, Progress};

/// Flags of table mode, shared by `info`, `stats`, `trace`, `table` and `gate`.
#[derive(Debug, Clone, Default, clap::Args)]
#[command(next_help_heading = "Batch table (many files → one table)")]
pub struct TidyArgs {
    /// One tidy table for all inputs: a row per data set (× image and channel, trace and
    /// sweep, parameter, population or well), with `path`, `format`, `error` columns. Implied
    /// by the other flags of this section, and by several inputs to trace, table and gate.
    #[arg(long)]
    pub tidy: bool,
    /// Join a sample sheet (CSV, TSV, XLSX) or plate layout (plate-map grids, 96–1536 wells)
    /// onto the rows. The key (file name, path, sample id recorded in the file, well, barcode,
    /// vial, run order) is chosen from the data and reported; repeatable.
    #[arg(long = "sample-sheet", value_name = "FILE")]
    pub sample_sheets: Vec<PathBuf>,
    /// Worksheet of an XLSX sample sheet (default: the plate-map sheets, else the first).
    #[arg(long, value_name = "NAME")]
    pub worksheet: Option<String>,
    /// Join key, `SHEET_COLUMN=FIELD` (fields: path, file, stem, sample_id, sample_name,
    /// barcode, well, position, run_order, column:NAME); repeat for a composite key.
    #[arg(long = "key", value_name = "COLUMN=FIELD")]
    pub keys: Vec<String>,
    /// Summarize by these columns (e.g. condition,dose): n, mean, sd, sem, median, min, max,
    /// CV % per group. Channels, parameters, populations and reads stay apart automatically.
    #[arg(long, value_delimiter = ',', value_name = "COLUMNS")]
    pub by: Vec<String>,
    /// Value columns to summarize (default: the measure's main values, e.g. mean and median).
    #[arg(long = "value", value_delimiter = ',', value_name = "COLUMNS")]
    pub values: Vec<String>,
    /// Average the rows of each replicate (a column such as replicate or well) before
    /// summarizing; n then counts replicates.
    #[arg(long, value_name = "COLUMN")]
    pub replicate: Option<String>,
    /// Compare each group with the control: `welch` (t-test) or `mann-whitney`.
    #[arg(long, value_name = "TEST", requires = "control")]
    pub test: Option<String>,
    /// The control group: a value of the first --by column.
    #[arg(long, value_name = "VALUE", requires = "test")]
    pub control: Option<String>,
    /// Group by exactly --by (do not add the measurement columns that vary).
    #[arg(long)]
    pub exact_by: bool,
    /// Keep only rows where COLUMN=VALUE (or COLUMN!=VALUE); repeatable, e.g.
    /// `--where parameter=FITC-A`.
    #[arg(long = "where", value_name = "COLUMN=VALUE")]
    pub filters: Vec<String>,
    /// Columns to output (identity and error columns are always kept). For `info`: index
    /// field names or aliases (see <https://openreadout.github.io/openreadout/guides/lab-shares.html>), or `all`.
    #[arg(long, value_delimiter = ',', value_name = "COLUMNS")]
    pub fields: Vec<String>,
    /// Write the table (or, with --by, the summary) to FILE: .csv, .tsv, .jsonl, .json or
    /// .parquet. Written under a temporary name, read back, then renamed; an interrupted run
    /// resumes from a journal next to it.
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,
    /// Replace an existing output file.
    #[arg(long)]
    pub overwrite: bool,
    /// Print the table (or summary) as CSV on standard output.
    #[arg(long)]
    pub csv: bool,
    /// Only data sets of these formats (format ids, e.g. fcs,czi); others are skipped and
    /// counted.
    #[arg(long = "formats", value_delimiter = ',', value_name = "IDS")]
    pub formats: Vec<String>,
    /// Add the data sets an index selects (`openreadout index`), with --query.
    #[arg(long, value_name = "INDEX_DIR")]
    pub from_index: Option<PathBuf>,
    /// Index query for --from-index (`search` syntax, e.g. "format=fcs channel~CD4").
    #[arg(long, value_name = "QUERY", requires = "from_index")]
    pub query: Option<String>,
    /// JSON: return at most N rows (the summary and counts are always complete). With -o the
    /// default is 0: the rows are in the file.
    #[arg(long, value_name = "N")]
    pub limit: Option<u64>,
    /// JSON: first row returned.
    #[arg(long, value_name = "N", default_value_t = 0)]
    pub offset: u64,
    // `--help` layout: the command's own flags that follow stay under "Options"
    #[command(flatten)]
    pub end_heading: super::help_heading::EndHeading,
}

/// Is table mode on? `several` is true for commands whose multi-input form is always a table
/// (trace, table, gate): then several inputs, a directory that is not itself a data set (a
/// Bruker experiment or ChemStation `.D` is one), a glob or `-r` turn it on too.
pub fn wanted(
    reg: &Registry,
    t: &TidyArgs,
    b: &BatchArgs,
    files: &[PathBuf],
    several: bool,
) -> bool {
    t.tidy
        || !t.sample_sheets.is_empty()
        || !t.by.is_empty()
        || t.output.is_some()
        || t.csv
        || !t.fields.is_empty()
        || t.from_index.is_some()
        || !t.filters.is_empty()
        || (several
            && (files.len() > 1
                || b.recursive
                || b.jsonl
                || files.iter().any(|f| {
                    (f.is_dir() && !openreadout_index::walk::is_dataset_dir(reg, f))
                        || (!f.exists() && openreadout_core::batch::has_glob_meta(f))
                })))
}

/// Build the pipeline from the flags.
pub fn pipeline(files: &[PathBuf], b: &BatchArgs, t: &TidyArgs) -> Result<Pipeline> {
    let mut req = BatchRequest::default();
    req.inputs = files.to_vec();
    req.recursive = b.recursive;
    req.formats = t.formats.clone();
    req.skip_unknown = b.skip_unknown;
    req.fail_fast = b.fail_fast;
    if let Some(ix) = &t.from_index {
        req.index = Some(IndexQuery {
            index_dir: ix.clone(),
            query: t.query.clone().unwrap_or_default(),
        });
    }
    let mut p = Pipeline::default();
    p.request = req;
    p.sample_sheets = t.sample_sheets.clone();
    p.worksheet = t.worksheet.clone();
    p.keys = t.keys.clone();
    p.filters = t
        .filters
        .iter()
        .map(|f| Filter::parse(f))
        .collect::<Result<_>>()?;
    p.fields = t.fields.clone();
    p.output = t.output.clone();
    p.overwrite = t.overwrite;
    if !t.by.is_empty() {
        let mut s = SummarizeRequest::default();
        s.by = t.by.clone();
        s.values = t.values.clone();
        s.replicate = t.replicate.clone();
        s.exact_by = t.exact_by;
        s.test = t
            .test
            .as_deref()
            .map(|x| {
                TestKind::parse(x)
                    .ok_or_else(|| Error::Usage(format!("--test {x}: use welch or mann-whitney")))
            })
            .transpose()?;
        s.control = t.control.clone();
        p.summarize = Some(s);
    } else if !t.values.is_empty() || t.replicate.is_some() || t.test.is_some() {
        return Err(Error::Usage(
            "--value, --replicate and --test summarize groups: pass --by COLUMN".into(),
        ));
    }
    Ok(p)
}

fn num(v: &Value) -> String {
    match v {
        Value::Float(x) if x.is_finite() => {
            if x.fract() == 0.0 && x.abs() < 1e15 {
                format!("{x:.0}")
            } else if *x != 0.0 && (x.abs() >= 1e6 || x.abs() < 1e-3) {
                format!("{x:.3e}")
            } else {
                let s = format!("{x:.4}");
                s.trim_end_matches('0').trim_end_matches('.').to_string()
            }
        }
        Value::Null => "-".into(),
        other => {
            let s = other.text();
            if s.chars().count() > 40 {
                let t: String = s.chars().take(39).collect();
                format!("{t}…")
            } else {
                s
            }
        }
    }
}

/// Rows shown in the human output before it points to -o / --csv.
const MAX_HUMAN_ROWS: usize = 40;

fn table_text(t: &Table) -> String {
    let heads: Vec<&str> = t.columns.iter().map(|c| c.name.as_str()).collect();
    let align: Vec<Align> = t
        .columns
        .iter()
        .map(|c| match c.kind {
            ColumnType::Integer | ColumnType::Float => Align::Right,
            _ => Align::Left,
        })
        .collect();
    let shown = t.rows.len().min(MAX_HUMAN_ROWS);
    let rows: Vec<Vec<String>> = t.rows[..shown]
        .iter()
        .map(|r| r.iter().map(num).collect())
        .collect();
    let mut s = ui::table(&heads, &align, &rows);
    if t.rows.len() > shown {
        s.push_str(&format!(
            "\n… {} more rows: write them with -o FILE (.csv, .parquet) or print them with --csv",
            t.rows.len() - shown
        ));
    }
    s
}

fn render(r: &PipelineResult) -> String {
    let mut s = String::new();
    let inp = &r.result.inputs;
    s.push_str(&ui::paint(
        ui::DIM,
        format!(
            "{} data sets ({} ok, {} failed){}{}\n",
            inp.datasets,
            inp.ok,
            inp.failed,
            if inp.skipped_unknown > 0 {
                format!(", {} non-instrument files skipped", inp.skipped_unknown)
            } else {
                String::new()
            },
            if inp.grouped_members > 0 {
                format!(", {} member files grouped", inp.grouped_members)
            } else {
                String::new()
            }
        ),
    ));
    if let Some(sm) = &r.summary {
        s.push_str(&format!(
            "summary by {} of {} ({} rows used, {} left out)\n",
            sm.group_by.join(", "),
            sm.values.join(", "),
            sm.rows_used,
            sm.rows_excluded
        ));
    }
    s.push_str(&table_text(r.main_table()));
    for j in &r.joins {
        s.push_str(&format!(
            "\n{} joined on {}: {}/{} rows, {}/{} data sets annotated (added: {})",
            Path::new(&j.sheet.path).file_name().map_or_else(
                || j.sheet.path.clone(),
                |f| f.to_string_lossy().into_owned()
            ),
            j.key
                .iter()
                .map(|k| format!("`{}` = {}", k.sheet_column, k.field.name()))
                .collect::<Vec<_>>()
                .join(" + "),
            j.matched_rows,
            j.rows,
            j.matched_datasets,
            j.datasets,
            j.annotations.join(", ")
        ));
        for w in &j.warnings {
            s.push_str(&format!("\n{} {w}", ui::paint(ui::WARN, "note:")));
        }
    }
    let mut notes: Vec<&String> = r
        .result
        .warnings
        .iter()
        .filter(|w| !w.starts_with("joined "))
        .collect();
    if let Some(sm) = &r.summary {
        notes.extend(sm.notes.iter());
    }
    for w in notes {
        s.push_str(&format!("\n{} {w}", ui::paint(ui::WARN, "note:")));
    }
    if let Some(w) = &r.written {
        s.push_str(&format!(
            "\nwrote {} ({} rows × {} columns, {}, {} bytes, verified={})",
            w.path,
            w.rows,
            w.columns,
            w.format.name(),
            w.bytes,
            w.verified
        ));
    }
    s
}

/// Run `measure` in table mode and print (JSON envelope, JSON Lines, CSV or text). Exit 0
/// when the table was built (failed inputs are rows with `error`), else the error's code.
pub fn run(
    reg: &Registry,
    measure: &dyn Measure,
    files: &[PathBuf],
    b: &BatchArgs,
    t: &TidyArgs,
    json: bool,
) -> i32 {
    let p = match pipeline(files, b, t) {
        Ok(p) => p,
        Err(e) => return fail(json, &e),
    };
    // JSON Lines without joins, filters or summaries streams rows as data sets finish
    let stream = b.jsonl
        && t.sample_sheets.is_empty()
        && t.by.is_empty()
        && t.filters.is_empty()
        && t.fields.is_empty()
        && t.output.is_none();
    let streamer = |_d: &DatasetRef, rows: &[Row]| {
        let mut out = std::io::stdout().lock();
        for r in rows {
            let o: serde_json::Map<String, serde_json::Value> = r
                .cells
                .iter()
                .map(|(n, v)| (n.clone(), serde_json::to_value(v).unwrap_or_default()))
                .collect();
            if let Ok(line) = serde_json::to_string(&o) {
                let _ = writeln!(out, "{line}");
            }
        }
    };
    let bar: Mutex<Option<Progress>> = Mutex::new(None);
    let cb = |done: u64, total: u64, path: &str| {
        if let Ok(mut g) = bar.lock() {
            g.get_or_insert_with(|| Progress::new(total, "data sets"))
                .set(done, Some(path));
        }
    };
    let res = run_pipeline(
        reg,
        measure,
        &p,
        Some(&cb),
        if stream { Some(&streamer) } else { None },
    );
    if let Ok(g) = bar.lock()
        && let Some(pr) = g.as_ref()
    {
        pr.finish();
    }
    let r = match res {
        Ok(r) => r,
        Err(e) => return fail(json || b.jsonl, &e),
    };
    if stream {
        return 0;
    }
    if b.jsonl {
        let mut out = std::io::stdout().lock();
        let _ = openreadout_batch::write::write_jsonl(r.main_table(), &mut out);
        return 0;
    }
    if t.csv {
        let mut out = std::io::stdout().lock();
        if let Err(e) = write_delimited(r.main_table(), &mut out, ',') {
            return fail(false, &Error::io("<stdout>", e));
        }
        return 0;
    }
    if json {
        let limit = t.limit.or(if r.written.is_some() { Some(0) } else { None });
        return emit(true, &r.output(t.offset, limit), |_| String::new());
    }
    if !ui::quiet() {
        anstream::println!("{}", render(&r));
    }
    0
}
