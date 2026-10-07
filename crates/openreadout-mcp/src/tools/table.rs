//! `openreadout_table`: rows of a table, with FCS compensation, transforms and gates.

use std::path::{Path, PathBuf};

use openreadout_core::Error;
use openreadout_core::model::TableSlice;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData as McpError, schemars, tool, tool_router};
use serde::Deserialize;

use crate::{InstrumentServer, mcp_err, ok_json};

/// Arguments for `openreadout_table`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TableArgs {
    /// Absolute or working-directory-relative path to the instrument file.
    pub file: String,
    /// Table index (FCS data set or plate read; see openreadout_info → tables[]). Default 0.
    #[serde(default)]
    pub table: u32,
    /// Zero-based first row. Default 0.
    #[serde(default)]
    pub first_row: u64,
    /// Maximum rows to return. Default 100 (fewer for wide tables: about 5000 values), capped at
    /// 10000.
    pub max_rows: Option<u64>,
    /// FCS only: compensate the values: `auto` (the gating file's matrix when one is given,
    /// else the file's $SPILLOVER/$SPILL/SPILL), `fcs` or `gating`. Values become FCS scale values
    /// first ($PnE, $PnG, $TIMESTEP).
    pub compensate: Option<String>,
    /// FCS only: transform after compensation: `logicle`, `arcsinh`, `hyperlog`, `log`, `linear`,
    /// `biex`, `flowjo-log`, `arcsinh-cofactor`, with parameters as `NAME:K=V,…`
    /// (`logicle:T=262144,W=0.5,M=4.5,A=0`, `arcsinh-cofactor:5`), or `workspace` for the FlowJo
    /// sample's per-parameter transforms.
    pub transform: Option<String>,
    /// FCS only: parameters ($PnN) to transform; default every fluorescence parameter.
    #[serde(default)]
    pub parameters: Vec<String>,
    /// FCS only: FlowJo workspace (.wsp) for compensate=gating, transform=workspace and populations.
    pub workspace: Option<String>,
    /// FCS only: Gating-ML 2.0 file (same uses as workspace).
    pub gatingml: Option<String>,
    /// Workspace sample name or id (default: matched by file name or $FIL).
    pub sample: Option<String>,
    /// FCS only: populations (paths like `/Lymphocytes/Singlets` or unique names) whose 0/1
    /// membership is appended as `gate:<path>` columns.
    #[serde(default)]
    pub populations: Vec<String>,
    /// Row conditions `COLUMN OP NUMBER` (OP: > >= < <= == !=; COLUMN: $PnN, $PnS or
    /// `gate:<path>`), all of which must hold, e.g. `["FITC-A > 1000"]`. Tested on the values
    /// returned (after compensate/transform when given) over the whole table;
    /// `filter.matched_rows` counts every match and first_row/max_rows page through them.
    #[serde(default)]
    pub filter: Vec<String>,
    /// Return only the count (`filter.matched_rows`, `total_rows`, `percent`), no rows.
    #[serde(default)]
    pub count: bool,
    /// true: refuse (an error with exit_code 6 and a hint) values this file's assurance does not
    /// validate. Default: the server's setting (OPENREADOUT_STRICT; off).
    #[serde(default)]
    pub strict: Option<bool>,
}

/// Most rows `openreadout_table` returns in one call.
pub const MAX_TABLE_ROWS: u64 = 10_000;

/// Values `openreadout_table` returns when `max_rows` is not given (rows = this / columns,
/// at most 100).
pub const MCP_TABLE_CELLS: usize = 5_000;

#[tool_router(router = table_router, vis = "pub(super)")]
impl InstrumentServer {
    #[tool(
        name = "openreadout_table",
        annotations(title = "Read table rows", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::common::schema_for_output::<TableSlice>(),
        description = "Rows of a table {columns, labels, rows, total_rows, truncated}: FCS events ($PnN columns), plate reads (well, row, col, read, wavelength_nm, time_s, value), event, spike or peak tables (including the vendor's own integration results). Values are raw; FCS can be compensated, transformed and given 0/1 population columns from a workspace or Gating-ML file. filter with count=true counts the events meeting conditions."
    )]
    pub(crate) fn table(
        &self,
        Parameters(a): Parameters<TableArgs>,
    ) -> Result<CallToolResult, McpError> {
        let reg = self.reg_for(a.strict);
        let mut opts = openreadout_fcs::analysis::TableOptions::default();
        opts.gating_file = a
            .workspace
            .as_ref()
            .or(a.gatingml.as_ref())
            .map(PathBuf::from);
        opts.sample = a.sample.clone();
        opts.populations = a.populations.clone();
        opts.compensation = match a.compensate.as_deref() {
            None => None,
            Some("auto") => Some(openreadout_fcs::analysis::CompensationChoice::Auto),
            Some("fcs") => Some(openreadout_fcs::analysis::CompensationChoice::File),
            Some("gating" | "workspace" | "gatingml") => {
                Some(openreadout_fcs::analysis::CompensationChoice::GatingFile)
            }
            Some(other) => {
                return Err(mcp_err(&Error::Usage(format!(
                    "compensate must be auto, fcs or gating, not {other}"
                ))));
            }
        };
        opts.transform = match a.transform.as_deref() {
            None => None,
            Some("workspace" | "gating") => {
                Some(openreadout_fcs::analysis::TransformChoice::GatingFile)
            }
            Some(spec) => Some(openreadout_fcs::analysis::TransformChoice::Uniform {
                transform: openreadout_fcs::analysis::parse_transform_spec(spec)
                    .map_err(|e| mcp_err(&e))?,
                parameters: a.parameters.clone(),
            }),
        };
        let want = a.max_rows.unwrap_or(100).min(MAX_TABLE_ROWS);
        let slice = if a.filter.is_empty() && !a.count {
            openreadout_fcs::analysis::table_slice(
                &reg,
                Path::new(&a.file),
                a.table,
                a.first_row,
                want,
                &opts,
            )
        } else {
            openreadout_fcs::filter::parse_conditions(&a.filter).and_then(|conds| {
                openreadout_fcs::filter::filtered_slice(
                    &reg,
                    Path::new(&a.file),
                    a.table,
                    a.first_row,
                    if a.count { 0 } else { want },
                    &opts,
                    &conds,
                )
            })
        }
        .map_err(|e| mcp_err(&e))?;
        let mut slice = slice;
        // Wide tables (spectral cytometers: hundreds of parameters): without an explicit
        // max_rows keep the answer to about MCP_TABLE_CELLS values; `truncated` and first_row
        // page through the rest.
        if a.max_rows.is_none() {
            let cols = slice.columns.len().max(1);
            let keep = (MCP_TABLE_CELLS / cols).max(1);
            if slice.rows.len() > keep {
                slice.rows.truncate(keep);
                slice.truncated = true;
            }
        }
        ok_json(&slice)
    }
}
