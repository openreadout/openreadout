//! The JSON shape of a batch table (`--tidy --json`, MCP `openreadout_batch`): a report of
//! what went in (inputs, joins), the columns with roles and units, the rows as arrays in column
//! order (a page of them when a limit is set), the group summary when one was asked for, and
//! where the full table was written.

use serde::Serialize;

use crate::join::JoinReport;
use crate::run::{BatchResult, InputReport};
use crate::summarize::SummaryOutput;
use crate::table::{Column, Value};
use crate::write::WrittenFile;

/// Output of a batch command in table mode.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct BatchOutput {
    /// Measure (`stats`, `trace`, `table`, `gate`, `info`, …).
    pub measure: String,
    /// Columns that identify a row within its data set.
    pub grain: Vec<String>,
    /// How the inputs became data sets.
    pub inputs: InputReport,
    /// Sample sheets and layouts joined, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub joins: Vec<JoinReport>,
    /// Columns of `rows`, with type, unit and role (`id`, `key`, `metadata`, `annotation`,
    /// `value`, `error`).
    pub columns: Vec<Column>,
    /// Rows in the whole table.
    pub total_rows: u64,
    /// Index of the first row in `rows`.
    pub offset: u64,
    /// Rows returned here.
    pub returned_rows: u64,
    /// True when the table has rows after the last one returned (page with `offset`, or read
    /// the full table from `output`).
    pub truncated: bool,
    /// Rows as arrays in column order.
    pub rows: Vec<Vec<Value>>,
    /// The group summary (`--by`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<SummaryOutput>,
    /// The file the full table was written to (`-o`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<WrittenFile>,
    /// Things to check: skipped inputs, grouping, join results.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl BatchOutput {
    /// Wrap a result: rows `[offset, offset + limit)` (`limit` `None`: all).
    pub fn new(res: &BatchResult, joins: Vec<JoinReport>, offset: u64, limit: Option<u64>) -> Self {
        let total = res.table.rows.len() as u64;
        let start = offset.min(total) as usize;
        let end = limit.map_or(total as usize, |l| {
            (start as u64).saturating_add(l).min(total) as usize
        });
        let mut warnings = res.warnings.clone();
        for j in &joins {
            warnings.extend(j.warnings.iter().cloned());
        }
        BatchOutput {
            measure: res.measure.clone(),
            grain: res.grain.clone(),
            inputs: res.inputs.clone(),
            joins,
            columns: res.table.columns.clone(),
            total_rows: total,
            offset: start as u64,
            returned_rows: (end - start) as u64,
            truncated: (end as u64) < total,
            rows: res.table.rows[start..end].to_vec(),
            summary: None,
            output: None,
            warnings,
        }
    }
}
