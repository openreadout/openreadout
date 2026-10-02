//! `openreadout_batch` (one measure over many files) and `openreadout_link` (files of the
//! same sample).

use openreadout_batch::api as batch;
use openreadout_core::Error;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData as McpError, schemars, tool, tool_router};
use serde::{Deserialize, Serialize};

use super::object_output;
use crate::{InstrumentServer, mcp_err, ok_json};

/// Arguments for `openreadout_batch`.
#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct BatchArgs {
    /// What to measure per data set.
    #[schemars(with = "openreadout_batch::api::MeasureName")]
    pub measure: String,
    /// Files, directories or glob patterns (summarize: the one table file).
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
    /// The measure's options. Analyses: the openreadout_analyze options of that kind, plus
    /// `rows` (which record list becomes rows: peaks peak|compound|chromatogram|band|region,
    /// nmr-peaks peak|integral|spectrum, ephys-features sweep|cell|spike, qpcr
    /// record|rq|standard_curve, assay wells|samples|compounds|kinetics|growth|quality).
    /// stats: image, select, level, per (channel|image|plane|well|field), wells, mip (z|t).
    /// trace: trace, sweep, channels. table: table, parameters, compensate, transform,
    /// workspace or gatingml, sample. gate: workspace or gatingml, sample, populations,
    /// medians, table. info: fields. spectra: the openreadout_spectra filters.
    #[serde(default)]
    pub options: serde_json::Map<String, serde_json::Value>,
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
}

/// The library request of an `openreadout_batch` call: the options of the built-in measures
/// (stats, trace, table, gate, info) become the request's own fields, after a check that
/// each is one the measure takes; analyses keep them as `options`.
fn batch_request(a: BatchArgs) -> Result<batch::BatchToolArgs, McpError> {
    let builtin = matches!(
        a.measure.as_str(),
        "stats" | "trace" | "table" | "gate" | "info"
    );
    let mut v =
        serde_json::to_value(&a).map_err(|e| McpError::internal_error(e.to_string(), None))?;
    if builtin && let Some(o) = v.as_object_mut() {
        openreadout_batch::measures::spec_from_options(&a.measure, a.options.clone())
            .map_err(|e| mcp_err(&e))?;
        o.remove("options");
        for (k, x) in a.options {
            o.insert(k, x);
        }
    }
    serde_json::from_value(v).map_err(|e| mcp_err(&Error::Usage(format!("batch request: {e}"))))
}

#[tool_router(router = batch_router, vis = "pub(super)")]
impl InstrumentServer {
    #[tool(
        name = "openreadout_batch",
        annotations(title = "Measure many files as one table", read_only_hint = false, destructive_hint = true, idempotent_hint = true, open_world_hint = false),
        output_schema = object_output::<batch::BatchToolOutput>(),
        description = "One measure over many files as one tidy table, optionally joined to sample sheets or plate maps (the join key is chosen from the data and reported in joins[] with unmatched rows) and summarized by group (by; test against a control). Analyses take the same options as openreadout_analyze. Inputs: files, directories, globs or an index query. Returns at most limit rows (page with offset; output writes them all to a file); a failing file is a row with error. measure=summarize regroups a table written by an earlier output."
    )]
    pub(crate) async fn batch(
        &self,
        Parameters(a): Parameters<BatchArgs>,
    ) -> Result<CallToolResult, McpError> {
        let registry = self.registry;
        let a = batch_request(a)?;
        let out =
            tokio::task::spawn_blocking(move || batch::run_batch_or_summary(&registry(), a, true))
                .await
                .map_err(|e| McpError::internal_error(format!("batch task failed: {e}"), None))?
                .map_err(|e| mcp_err(&e))?;
        ok_json(&out)
    }

    #[tool(
        name = "openreadout_link",
        annotations(title = "Link files of the same sample", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::common::schema_for_output::<openreadout_batch::LinkOutput>(),
        description = "Groups files that measured the same sample across instruments and formats (sample id, barcode, plate well + plate, a conversion naming its source, the same acquisition or the same spectra), from headers; every link has its evidence and a confidence (high/medium/low; links below min_confidence are listed in weak_links)."
    )]
    pub(crate) async fn link(
        &self,
        Parameters(a): Parameters<batch::LinkToolArgs>,
    ) -> Result<CallToolResult, McpError> {
        let registry = self.registry;
        let out = tokio::task::spawn_blocking(move || batch::run_link(&registry(), a))
            .await
            .map_err(|e| McpError::internal_error(format!("link task failed: {e}"), None))?
            .map_err(|e| mcp_err(&e))?;
        ok_json(&out)
    }
}
