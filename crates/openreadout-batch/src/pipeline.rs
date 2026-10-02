//! The whole table-mode pipeline, shared by the CLI, the MCP server and the Python package:
//! run a measure, join sample sheets, filter rows, select columns, summarize, write.

use std::path::PathBuf;

use openreadout_core::{Error, Registry, Result};

use crate::join::{JoinOptions, JoinReport, join};
use crate::measure::Measure;
use crate::output::BatchOutput;
use crate::run::{BatchRequest, BatchResult, Progress, Sink, run};
use crate::summarize::{Filter, SummarizeRequest, SummaryOutput, summarize};
use crate::table::Table;
use crate::write::{TableMeta, save};

/// Everything after the inputs.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct Pipeline {
    /// Inputs and how to run.
    pub request: BatchRequest,
    /// Sample sheets and plate layouts, joined in order.
    pub sample_sheets: Vec<PathBuf>,
    /// Worksheet of workbook sheets.
    pub worksheet: Option<String>,
    /// Join keys (`COLUMN=FIELD`); empty: chosen from the data.
    pub keys: Vec<String>,
    /// Row filters.
    pub filters: Vec<Filter>,
    /// Columns to keep (identity and error columns always stay). Ignored by `info`, which
    /// takes its fields itself.
    pub fields: Vec<String>,
    /// Group summary, when `by` is set.
    pub summarize: Option<SummarizeRequest>,
    /// Write the table (or the summary) here.
    pub output: Option<PathBuf>,
    /// Replace an existing output.
    pub overwrite: bool,
}

/// What the pipeline produced.
#[derive(Debug, Clone)]
pub struct PipelineResult {
    /// The measured, joined and filtered table.
    pub result: BatchResult,
    /// The joins.
    pub joins: Vec<JoinReport>,
    /// The summary.
    pub summary: Option<SummaryOutput>,
    /// Where the output went.
    pub written: Option<crate::write::WrittenFile>,
}

impl PipelineResult {
    /// The table a user asked for: the summary when there is one, else the rows.
    pub fn main_table(&self) -> &Table {
        self.summary
            .as_ref()
            .map_or(&self.result.table, |s| &s.table)
    }
    /// The JSON output with rows `[offset, offset + limit)` of the row table.
    pub fn output(&self, offset: u64, limit: Option<u64>) -> BatchOutput {
        let mut out = BatchOutput::new(&self.result, self.joins.clone(), offset, limit);
        if let Some(s) = &self.summary {
            out.warnings.extend(s.notes.iter().cloned());
        }
        out.summary = self.summary.clone();
        out.output = self.written.clone();
        out
    }
}

/// The journal a run with this output resumes from.
pub fn journal_path(out: &std::path::Path) -> PathBuf {
    let name = out
        .file_name()
        .map_or_else(|| "table".into(), |n| n.to_string_lossy().into_owned());
    out.with_file_name(format!(".{name}.openreadout-journal.jsonl"))
}

/// Keep the rows passing every filter.
pub fn filter_rows(res: &mut BatchResult, filters: &[Filter]) -> Result<()> {
    if filters.is_empty() {
        return Ok(());
    }
    let mut idx = Vec::new();
    for f in filters {
        let i = res.table.index_of(&f.column).ok_or_else(|| {
            Error::Usage(format!(
                "--where {}: no column `{}` (columns: {})",
                f.column,
                f.column,
                res.table
                    .columns
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        idx.push(i);
    }
    let mut rows = Vec::new();
    let mut ds = Vec::new();
    for (r, row) in std::mem::take(&mut res.table.rows).into_iter().enumerate() {
        if filters.iter().zip(&idx).all(|(f, &i)| f.keeps(&row[i])) {
            rows.push(row);
            ds.push(res.row_dataset[r]);
        }
    }
    res.table.rows = rows;
    res.row_dataset = ds;
    Ok(())
}

/// Run the pipeline.
pub fn run_pipeline(
    reg: &Registry,
    measure: &dyn Measure,
    p: &Pipeline,
    progress: Option<Progress<'_>>,
    sink: Option<Sink<'_>>,
) -> Result<PipelineResult> {
    let mut req = p.request.clone();
    req.exclude.extend(p.sample_sheets.iter().cloned());
    if let Some(o) = &p.output {
        req.exclude.push(o.clone());
        if req.journal.is_none() {
            req.journal = Some(journal_path(o));
        }
        if o.exists() && !p.overwrite {
            return Err(Error::Usage(format!(
                "{} exists; pass --overwrite to replace it",
                o.display()
            )));
        }
    }
    let opts = JoinOptions::parse_keys(&p.keys)?;
    // read the sheets first: a bad sheet fails before any file is measured
    let sheets = p
        .sample_sheets
        .iter()
        .map(|s| crate::sheet::read(s, p.worksheet.as_deref()))
        .collect::<Result<Vec<_>>>()?;
    let mut res = run(reg, measure, &req, progress, sink)?;
    let mut joins = Vec::new();
    for s in sheets {
        joins.push(join(&mut res, s, &opts)?);
    }
    filter_rows(&mut res, &p.filters)?;
    if !p.fields.is_empty() && measure.id() != "info" {
        let mut names: Vec<String> = ["path", "format"]
            .iter()
            .filter(|n| res.table.index_of(n).is_some())
            .map(|n| (*n).to_string())
            .collect();
        for f in p.fields.iter().flat_map(|f| f.split(',')).map(str::trim) {
            if !f.is_empty() && !names.iter().any(|n| n == f) {
                names.push(f.to_string());
            }
        }
        for n in ["error", "error_code"] {
            if res.table.index_of(n).is_some() && !names.iter().any(|x| x == n) {
                names.push(n.to_string());
            }
        }
        res.table.select(&names).map_err(Error::Usage)?;
    }
    let summary = match &p.summarize {
        Some(s) if !s.by.is_empty() => Some(summarize(&res.table, &res.measure, s)?),
        _ => None,
    };
    let mut out = PipelineResult {
        result: res,
        joins,
        summary,
        written: None,
    };
    if let Some(o) = &p.output {
        let meta = TableMeta {
            measure: out.result.measure.clone(),
            grain: out
                .summary
                .as_ref()
                .map_or_else(|| out.result.grain.clone(), |s| s.group_by.clone()),
        };
        out.written = Some(save(out.main_table(), o, None, &meta, p.overwrite)?);
        let _ = std::fs::remove_file(journal_path(o));
    }
    Ok(out)
}
