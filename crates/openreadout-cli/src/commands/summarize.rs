//! `summarize` (group statistics of a table file written by `--tidy -o` or `batch -o`) and
//! `link` (files that measured the same sample).

use std::fmt::Write as _;
use std::path::PathBuf;

use openreadout_batch::link::Confidence;
use openreadout_batch::summarize::{Filter, SummarizeRequest, SummaryOutput, TestKind};
use openreadout_batch::write::{TableMeta, WrittenFile, read_table, save};
use openreadout_batch::{LinkOptions, LinkOutput};
use openreadout_core::{Error, Registry, Result};
use serde::Serialize;

use crate::output::{emit, fail};
use crate::ui;

/// Arguments of `summarize`.
#[derive(Debug, clap::Args)]
pub struct SummarizeArgs {
    /// A table written by `--tidy -o` or `batch -o` (or any CSV, TSV, JSON Lines, JSON or
    /// Parquet table), or the `--tidy --json` output saved to a file.
    #[arg(value_name = "TABLE")]
    pub table: PathBuf,
    /// Summarize by these columns (comma-separated or repeated): n, mean, sd, sem, median, min,
    /// max and CV % per group; channels, parameters and populations stay apart unless
    /// `--exact-by`.
    #[arg(long, required = true, value_delimiter = ',', value_name = "COLUMNS")]
    pub by: Vec<String>,
    /// Value columns to summarize (default: the measure's main values).
    #[arg(long = "value", value_delimiter = ',', value_name = "COLUMNS")]
    pub values: Vec<String>,
    /// Average rows within each value of this column first (technical replicates).
    #[arg(long, value_name = "COLUMN")]
    pub replicate: Option<String>,
    /// `welch` or `mann-whitney`: compare every group with `--control`.
    #[arg(long, value_name = "TEST", requires = "control")]
    pub test: Option<String>,
    /// The control group: a value of the first `--by` column.
    #[arg(long, value_name = "VALUE", requires = "test")]
    pub control: Option<String>,
    /// Keep only rows where COLUMN equals (`=`) or differs from (`!=`) VALUE. Repeatable.
    #[arg(long = "where", value_name = "COLUMN=VALUE")]
    pub filters: Vec<String>,
    /// Group by exactly `--by`.
    #[arg(long)]
    pub exact_by: bool,
    /// Also write the summary to this .csv/.tsv/.jsonl/.json/.parquet file (verified).
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,
    /// Replace an existing output file.
    #[arg(long)]
    pub overwrite: bool,
    /// Print the summary table as CSV.
    #[arg(long, conflicts_with = "json")]
    pub csv: bool,
    #[arg(long)]
    pub json: bool,
}

/// Output of `summarize`.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct SummarizeOutput {
    /// The table summarized.
    pub input: String,
    /// The summary.
    pub summary: SummaryOutput,
    /// Where it was written (`-o`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<WrittenFile>,
}

fn request(a: &SummarizeArgs) -> Result<SummarizeRequest> {
    let mut r = SummarizeRequest::default();
    r.by = a.by.clone();
    r.values = a.values.clone();
    r.replicate = a.replicate.clone();
    r.control = a.control.clone();
    r.exact_by = a.exact_by;
    r.test = a
        .test
        .as_deref()
        .map(|t| {
            TestKind::parse(t)
                .ok_or_else(|| Error::Usage(format!("--test {t}: use welch or mann-whitney")))
        })
        .transpose()?;
    r.filters = a
        .filters
        .iter()
        .map(|f| Filter::parse(f))
        .collect::<Result<_>>()?;
    Ok(r)
}

pub fn run(a: &SummarizeArgs) -> i32 {
    let res = (|| -> Result<SummarizeOutput> {
        let (t, measure) = read_table(&a.table)?;
        let s = openreadout_batch::summarize(&t, measure.as_deref().unwrap_or(""), &request(a)?)?;
        let output = match &a.output {
            Some(o) => Some(save(
                &s.table,
                o,
                None,
                &TableMeta {
                    measure: measure.unwrap_or_default(),
                    grain: s.group_by.clone(),
                },
                a.overwrite,
            )?),
            None => None,
        };
        Ok(SummarizeOutput {
            input: a.table.display().to_string(),
            summary: s,
            output,
        })
    })();
    match res {
        Ok(o) if a.csv => {
            let mut out = std::io::stdout().lock();
            match openreadout_batch::write::write_delimited(&o.summary.table, &mut out, ',') {
                Ok(()) => 0,
                Err(e) => fail(false, &Error::io("<stdout>", e)),
            }
        }
        Ok(o) => emit(a.json, &o, render_summary),
        Err(e) => fail(a.json, &e),
    }
}

fn render_summary(o: &SummarizeOutput) -> String {
    let s = &o.summary;
    let heads: Vec<&str> = s.table.columns.iter().map(|c| c.name.as_str()).collect();
    let align: Vec<ui::Align> = s
        .table
        .columns
        .iter()
        .map(|c| match c.kind {
            openreadout_batch::ColumnType::Integer | openreadout_batch::ColumnType::Float => {
                ui::Align::Right
            }
            _ => ui::Align::Left,
        })
        .collect();
    let rows: Vec<Vec<String>> = s
        .table
        .rows
        .iter()
        .map(|r| {
            r.iter()
                .map(|v| match v {
                    openreadout_batch::Value::Float(x) => {
                        if x.fract() == 0.0 && x.abs() < 1e15 {
                            format!("{x:.0}")
                        } else if *x != 0.0 && (x.abs() >= 1e6 || x.abs() < 1e-3) {
                            format!("{x:.3e}")
                        } else {
                            format!("{x:.4}")
                        }
                    }
                    openreadout_batch::Value::Null => "-".into(),
                    other => other.text(),
                })
                .collect()
        })
        .collect();
    let mut out = format!(
        "{}: grouped by {} ({} rows used, {} left out)\n",
        o.input,
        s.group_by.join(", "),
        s.rows_used,
        s.rows_excluded
    );
    out.push_str(&ui::table(&heads, &align, &rows));
    for n in &s.notes {
        let _ = write!(out, "\n{} {n}", ui::paint(ui::WARN, "note:"));
    }
    if let Some(w) = &o.output {
        let _ = write!(out, "\nwrote {} ({} rows)", w.path, w.rows);
    }
    out
}

/// Arguments of `link`.
#[derive(Debug, clap::Args)]
pub struct LinkArgs {
    /// Directories (walked recursively), files or glob patterns.
    #[arg(required_unless_present = "from_index", value_name = "PATH")]
    pub paths: Vec<PathBuf>,
    /// Only the top level of directory arguments.
    #[arg(long)]
    pub no_recursive: bool,
    /// Links below this confidence are listed but do not join groups: low, medium (default),
    /// high.
    #[arg(long, default_value = "medium", value_name = "LEVEL")]
    pub min_confidence: String,
    /// An identifier shared by more data sets than this counts as weak evidence.
    #[arg(long, default_value_t = 12, value_name = "N")]
    pub max_shared: usize,
    /// Only data sets of these formats.
    #[arg(long = "formats", value_delimiter = ',', value_name = "IDS")]
    pub formats: Vec<String>,
    /// Add the data sets an index selects, with --query.
    #[arg(long, value_name = "INDEX_DIR")]
    pub from_index: Option<PathBuf>,
    /// Index query for --from-index.
    #[arg(long, value_name = "QUERY", requires = "from_index")]
    pub query: Option<String>,
    #[arg(long)]
    pub json: bool,
}

pub fn run_link(reg: &Registry, a: &LinkArgs) -> i32 {
    let res = (|| -> Result<LinkOutput> {
        let mut o = LinkOptions::default();
        o.request.inputs = a.paths.clone();
        o.request.recursive = !a.no_recursive;
        o.request.formats = a.formats.clone();
        if let Some(ix) = &a.from_index {
            o.request.index = Some(openreadout_batch::IndexQuery {
                index_dir: ix.clone(),
                query: a.query.clone().unwrap_or_default(),
            });
        }
        o.min_confidence = Confidence::parse(&a.min_confidence).ok_or_else(|| {
            Error::Usage(format!(
                "--min-confidence {}: use low, medium or high",
                a.min_confidence
            ))
        })?;
        o.max_shared = a.max_shared;
        openreadout_batch::link(reg, &o)
    })();
    match res {
        Ok(o) => emit(a.json, &o, render_link),
        Err(e) => fail(a.json, &e),
    }
}

fn render_link(o: &LinkOutput) -> String {
    let mut s = format!(
        "{} data sets: {} groups, {} unlinked\n",
        o.inputs.datasets,
        o.groups.len(),
        o.unlinked
    );
    for g in &o.groups {
        let _ = writeln!(
            s,
            "\n{} {} confidence{}",
            ui::paint(ui::BOLD, format!("group {}", g.group)),
            format!("{:?}", g.confidence).to_lowercase(),
            g.sample
                .as_deref()
                .map(|x| format!(", sample {x}"))
                .unwrap_or_default()
        );
        for m in &g.members {
            let _ = writeln!(s, "  {} ({})", m.path, m.format.as_deref().unwrap_or("?"));
        }
        for l in &g.links {
            for e in &l.evidence {
                let _ = writeln!(
                    s,
                    "    {} [{}] {}",
                    ui::paint(ui::DIM, &e.kind),
                    format!("{:?}", e.confidence).to_lowercase(),
                    e.detail
                );
            }
        }
        for c in &g.conflicts {
            let _ = writeln!(s, "  {} {c}", ui::paint(ui::WARN, "conflict:"));
        }
    }
    if !o.weak_links.is_empty() {
        let _ = writeln!(
            s,
            "\n{} weak link(s) not grouped (see --json → weak_links)",
            o.weak_links.len()
        );
    }
    for i in &o.shared_identifiers {
        let _ = writeln!(
            s,
            "{} `{}` ({}, {} data sets): {}",
            ui::paint(ui::WARN, "note:"),
            i.value,
            i.field,
            i.datasets,
            i.reason
        );
    }
    for n in &o.notes {
        let _ = writeln!(s, "{} {n}", ui::paint(ui::WARN, "note:"));
    }
    s.trim_end().to_string()
}
