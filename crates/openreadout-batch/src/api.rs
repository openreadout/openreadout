//! Requests by name, as the MCP tools (`openreadout_batch`, `openreadout_link`) and the
//! Python package (`openreadout.batch()`, `summarize()`, `link()`) send them: one JSON shape
//! for both.
//!
//! Response size: a batch over hundreds of files must not flood an agent's context, so the MCP
//! tool answers summary-first: counts, the join report, the column list and at most `limit`
//! rows (default [`DEFAULT_BATCH_ROWS`], at most [`MAX_BATCH_ROWS`]); the summary rows (`by`)
//! are always complete, the full table goes to `output` (CSV or Parquet) when asked, and
//! `offset` pages. Python asks for every row.

// Requests are copied field by field into the library's option structs, which are
// `#[non_exhaustive]` for other crates; the same code reads the same way here.
#![allow(clippy::field_reassign_with_default)]

use std::path::PathBuf;

use openreadout_core::{Error, Registry, Result};
use serde::{Deserialize, Serialize};

use crate::link::Confidence;
use crate::measures::{MeasureSpec, build};
use crate::run::{BatchRequest, IndexQuery};
use crate::summarize::{Filter, SummarizeRequest, TestKind};
use crate::write::{TableMeta, read_table, save};
use crate::{BatchOutput, LinkOptions, LinkOutput, Pipeline, run_pipeline};

/// Rows `openreadout_batch` (MCP) returns when `limit` is not given.
pub const DEFAULT_BATCH_ROWS: u64 = 20;
/// Most rows one MCP call returns.
pub const MAX_BATCH_ROWS: u64 = 500;

/// Arguments for `openreadout_batch`.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct BatchToolArgs {
    /// What to measure per data set.
    #[schemars(with = "MeasureName")]
    pub measure: String,
    /// Files, directories or glob patterns.
    #[serde(default)]
    pub inputs: Vec<String>,
    /// Walk sub-directories.
    #[serde(default)]
    pub recursive: bool,
    /// Also take the data sets an index query selects: the index directory …
    pub from_index: Option<String>,
    /// … and the query (search syntax, e.g. "format=fcs channel~CD4").
    pub query: Option<String>,
    /// Only these format ids (e.g. `["fcs"]`).
    #[serde(default)]
    pub formats: Vec<String>,
    /// Sample sheets (CSV/TSV/XLSX) or plate layouts (plate-map grids) to join; the key is
    /// chosen from the data (file name, path, sample id recorded in the file, well, barcode,
    /// vial, run order) and reported under joins[].
    #[serde(default)]
    pub sample_sheets: Vec<String>,
    /// Worksheet of an XLSX sheet.
    pub worksheet: Option<String>,
    /// Fixed join keys `SHEET_COLUMN=FIELD` (path, file, stem, sample_id, sample_name, barcode,
    /// well, position, run_order, column:NAME).
    #[serde(default)]
    pub keys: Vec<String>,
    /// Row filters `COLUMN=VALUE` / `COLUMN!=VALUE` (e.g. "parameter=FITC-A").
    #[serde(default, rename = "where")]
    pub filters: Vec<String>,
    /// Columns to keep (info: index field names, or `["all"]`).
    #[serde(default)]
    pub fields: Vec<String>,
    /// Summarize by these columns (e.g. `["condition"]`): n, mean, sd, sem, median, min, max,
    /// cv_percent per group; channels/parameters/populations stay apart automatically.
    #[serde(default)]
    pub by: Vec<String>,
    /// Value columns to summarize (default: the measure's main values).
    #[serde(default)]
    pub values: Vec<String>,
    /// Average rows within each replicate first.
    pub replicate: Option<String>,
    /// Group by exactly `by` (do not add the measurement columns that vary).
    #[serde(default)]
    pub exact_by: bool,
    /// `welch` or `mann-whitney` against `control` (a value of the first `by` column).
    pub test: Option<String>,
    /// The control group.
    pub control: Option<String>,
    /// Write the full table (or the summary, with `by`) to this .csv/.tsv/.jsonl/.json/.parquet
    /// file (verified; never overwrites without `overwrite`).
    pub output: Option<String>,
    /// Replace an existing output file.
    #[serde(default)]
    pub overwrite: bool,
    /// Rows to return (default 20, max 500; 0 with `output`: the rows are in the file).
    pub limit: Option<u64>,
    /// First row returned.
    #[serde(default)]
    pub offset: u64,
    /// stats: only this image.
    pub image: Option<u32>,
    /// stats: pyramid level (0 = full resolution).
    #[serde(default)]
    pub level: u32,
    /// stats per `well` or `field`: only these wells (`C05`).
    #[serde(default)]
    pub wells: Vec<String>,
    /// stats: plane selection (`c=0`, `z=2-5`).
    #[serde(default)]
    pub select: Vec<String>,
    /// stats: row per `channel` (default), `image`, `plane`, `well` or `field` (multi-well
    /// plates).
    pub per: Option<String>,
    /// stats: maximum-intensity projection first, along `z` or `t` (with `select`, e.g.
    /// `c=1`: the MIP of that channel); rows are the projections' statistics.
    pub mip: Option<String>,
    /// trace: only this trace.
    pub trace: Option<u32>,
    /// trace: only this sweep.
    pub sweep: Option<u32>,
    /// trace: only these channels.
    #[serde(default)]
    pub channels: Vec<u32>,
    /// table/gate: FCS data set index.
    pub table: Option<u32>,
    /// table: only these parameters ($PnN or $PnS).
    #[serde(default)]
    pub parameters: Vec<String>,
    /// table (FCS): compensate `auto`, `fcs` or `gating`.
    pub compensate: Option<String>,
    /// table (FCS): transform (`logicle`, `arcsinh-cofactor:5`, `workspace`, …).
    pub transform: Option<String>,
    /// gate: FlowJo workspace (.wsp).
    pub workspace: Option<String>,
    /// gate: Gating-ML 2.0 file.
    pub gatingml: Option<String>,
    /// gate: workspace sample (default: matched per file).
    pub sample: Option<String>,
    /// gate: only these populations and their descendants.
    #[serde(default)]
    pub populations: Vec<String>,
    /// gate: parameters whose median per population is reported (`Comp-NAME` = compensated).
    #[serde(default)]
    pub medians: Vec<String>,
    /// Options of the analysis measures: the arguments of that analysis's MCP tool (e.g.
    /// `{"mz": [195.0877], "ppm": 10}` for openreadout_peaks; assay: `{"analysis": "curve"}`
    /// plus the openreadout_assay_curve arguments), plus `rows` to pick the record list (peaks: peak|compound|chromatogram;
    /// nmr-peaks: peak|integral|spectrum; ephys-features: sweep|cell|spike; qpcr:
    /// record|rq|standard_curve; assay: wells|samples|compounds|kinetics|growth|quality).
    #[serde(default)]
    pub options: serde_json::Map<String, serde_json::Value>,
}

/// What to measure per data set.
#[derive(Debug, Clone, Copy, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum MeasureName {
    /// pixel statistics per image × channel (per=well for screening plates)
    Stats,
    /// statistics per trace × sweep × channel
    Trace,
    /// FCS: events, median, mean, sd, min, max per parameter; plate reads: one row per well
    Table,
    /// one row of header metadata per data set (fields)
    Info,
    /// one row per MS scan header (options: the openreadout_scans filters)
    Scans,
    /// openreadout_peaks (its arguments as options)
    Peaks,
    /// openreadout_chromatogram (its arguments as options)
    Chromatogram,
    /// a plate-reader assay: options `analysis` (wells, curve, dose-response, kinetics, growth, qc) and the arguments of that openreadout_assay_* tool
    Assay,
    /// openreadout_nmr_peaks (its arguments as options)
    NmrPeaks,
    /// openreadout_ephys_features (its arguments as options)
    EphysFeatures,
    /// openreadout_spikes (its arguments as options)
    Spikes,
    /// openreadout_qpcr (its arguments as options)
    Qpcr,
    /// population counts, percentages and medians (workspace or gatingml)
    Gate,
}

/// Run a batch request. `paged`: answer summary-first as the MCP tool does (at most `limit`
/// rows, default [`DEFAULT_BATCH_ROWS`], capped at [`MAX_BATCH_ROWS`]); otherwise every row
/// unless `limit` says otherwise.
pub fn run_batch(reg: &Registry, a: BatchToolArgs, paged: bool) -> Result<BatchOutput> {
    let gating = a.workspace.clone().or_else(|| a.gatingml.clone());
    let spec = {
        let mut s = MeasureSpec::default();
        s.measure = a.measure.clone();
        s.image = a.image;
        s.level = a.level;
        s.wells = a.wells.clone();
        s.select = a.select.clone();
        s.per = a.per.clone();
        s.mip = a.mip.clone();
        s.trace = a.trace;
        s.sweep = a.sweep;
        s.channels = a.channels.clone();
        s.table = a.table;
        s.parameters = a.parameters.clone();
        s.compensate = a.compensate.clone();
        s.transform = a.transform.clone();
        s.gating_file = gating.as_ref().map(PathBuf::from);
        s.sample = a.sample.clone();
        s.populations = a.populations.clone();
        s.medians = a.medians.clone();
        s.fields = a.fields.clone();
        s.options = a.options.clone();
        s
    };
    let measure = build(&spec)?;
    let mut req = BatchRequest::default();
    req.inputs = a.inputs.iter().map(PathBuf::from).collect();
    if let Some(g) = &gating {
        req.exclude.push(PathBuf::from(g));
    }
    req.recursive = a.recursive;
    req.formats = a.formats.clone();
    if let Some(ix) = &a.from_index {
        req.index = Some(IndexQuery {
            index_dir: PathBuf::from(ix),
            query: a.query.clone().unwrap_or_default(),
        });
    }
    let mut p = Pipeline::default();
    p.request = req;
    p.sample_sheets = a.sample_sheets.iter().map(PathBuf::from).collect();
    p.worksheet = a.worksheet.clone();
    p.keys = a.keys.clone();
    p.filters = a
        .filters
        .iter()
        .map(|f| Filter::parse(f))
        .collect::<Result<_>>()?;
    p.fields = a.fields.clone();
    p.output = a.output.as_ref().map(PathBuf::from);
    p.overwrite = a.overwrite;
    if !a.by.is_empty() {
        let mut s = SummarizeRequest::default();
        s.by = a.by.clone();
        s.values = a.values.clone();
        s.replicate = a.replicate.clone();
        s.control = a.control.clone();
        s.exact_by = a.exact_by;
        s.test = test_kind(a.test.as_deref())?;
        p.summarize = Some(s);
    }
    let r = run_pipeline(reg, measure.as_ref(), &p, None, None)?;
    let limit = if paged {
        Some(
            a.limit
                .unwrap_or(if r.written.is_some() {
                    0
                } else {
                    DEFAULT_BATCH_ROWS
                })
                .min(MAX_BATCH_ROWS),
        )
    } else {
        a.limit
    };
    let mut out = r.output(a.offset, limit);
    if paged && out.truncated && out.output.is_none() {
        out.warnings.push(format!(
            "{} of {} rows returned: page with offset, or pass output=\"table.csv\" (or .parquet) to write them all",
            out.returned_rows, out.total_rows
        ));
    }
    Ok(out)
}

fn test_kind(t: Option<&str>) -> Result<Option<TestKind>> {
    t.map(|x| {
        TestKind::parse(x)
            .ok_or_else(|| Error::Usage(format!("test `{x}`: use welch or mann-whitney")))
    })
    .transpose()
}

/// A summary request (`openreadout_summarize`; Python `summarize()`).
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct SummarizeToolArgs {
    /// A table file written by an openreadout_batch `output` (or any CSV, TSV, JSON Lines,
    /// JSON or Parquet table).
    pub table: String,
    /// Group by these columns.
    pub by: Vec<String>,
    /// Value columns (default: the measure's main values).
    #[serde(default)]
    pub values: Vec<String>,
    /// Average rows within each replicate first.
    pub replicate: Option<String>,
    /// `welch` or `mann-whitney` against `control`.
    pub test: Option<String>,
    /// The control group (a value of the first `by` column).
    pub control: Option<String>,
    /// Row filters `COLUMN=VALUE` / `COLUMN!=VALUE`.
    #[serde(default, rename = "where")]
    pub filters: Vec<String>,
    /// Group by exactly `by`.
    #[serde(default)]
    pub exact_by: bool,
    /// Write the summary to this file.
    pub output: Option<String>,
    /// Replace an existing output file.
    #[serde(default)]
    pub overwrite: bool,
}

/// A summary of a saved table.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct SummarizeToolOutput {
    /// The table summarized.
    pub input: String,
    /// The summary.
    pub summary: crate::SummaryOutput,
    /// Where it was written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<crate::write::WrittenFile>,
}

/// Summarize a saved table.
pub fn run_summarize(a: SummarizeToolArgs) -> Result<SummarizeToolOutput> {
    let (t, measure) = read_table(std::path::Path::new(&a.table))?;
    let mut s = SummarizeRequest::default();
    s.by = a.by.clone();
    s.values = a.values.clone();
    s.replicate = a.replicate.clone();
    s.control = a.control.clone();
    s.exact_by = a.exact_by;
    s.test = test_kind(a.test.as_deref())?;
    s.filters = a
        .filters
        .iter()
        .map(|f| Filter::parse(f))
        .collect::<Result<_>>()?;
    let summary = crate::summarize(&t, measure.as_deref().unwrap_or(""), &s)?;
    let output = match &a.output {
        Some(o) => Some(save(
            &summary.table,
            std::path::Path::new(o),
            None,
            &TableMeta {
                measure: measure.unwrap_or_default(),
                grain: summary.group_by.clone(),
            },
            a.overwrite,
        )?),
        None => None,
    };
    Ok(SummarizeToolOutput {
        input: a.table,
        summary,
        output,
    })
}

/// Arguments for `openreadout_link`.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct LinkToolArgs {
    /// Directories (walked recursively), files or glob patterns.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Only the top level of directories.
    #[serde(default)]
    pub no_recursive: bool,
    /// `low`, `medium` (default) or `high`: weaker links are listed in weak_links, not grouped.
    pub min_confidence: Option<String>,
    /// Only these format ids.
    #[serde(default)]
    pub formats: Vec<String>,
    /// Also take the data sets an index query selects: the index directory …
    pub from_index: Option<String>,
    /// … and the query.
    pub query: Option<String>,
}

/// Run `openreadout_link`.
pub fn run_link(reg: &Registry, a: LinkToolArgs) -> Result<LinkOutput> {
    let mut o = LinkOptions::default();
    o.request.inputs = a.paths.iter().map(PathBuf::from).collect();
    o.request.recursive = !a.no_recursive;
    o.request.formats = a.formats;
    if let Some(ix) = a.from_index {
        o.request.index = Some(IndexQuery {
            index_dir: PathBuf::from(ix),
            query: a.query.unwrap_or_default(),
        });
    }
    if let Some(c) = a.min_confidence.as_deref() {
        o.min_confidence = Confidence::parse(c)
            .ok_or_else(|| Error::Usage(format!("min_confidence `{c}`: low, medium or high")))?;
    }
    crate::link(reg, &o)
}
