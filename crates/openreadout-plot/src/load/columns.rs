//! Histograms of a column of a file's tables (any reader): read lengths of a FASTQ or BAM file,
//! read durations of a nanopore file, tile densities of a sequencing run.

use openreadout_core::Result;
use serde_json::Value;

use super::{Ctx, usage};
use crate::model::{Geom, Quantity, Series, Source};
use crate::spec::{MarkKind, SeriesSpec};

/// Most rows read from one table.
const MAX_ROWS: u64 = 5_000_000;

/// A histogram of column `x` (default `length` for read files) of the first table that has it.
pub(super) fn histogram(ctx: &mut Ctx<'_>, spec: &SeriesSpec, file: &str) -> Result<Vec<Series>> {
    let (_, mut ds, info) = ctx.open(file)?;
    let x = match &spec.x {
        Some(x) => x.clone(),
        None if info.format.family == "sequencing" => "length".to_string(),
        None => {
            return Err(usage(format!(
                "{file}: give x, the table column to bin; columns: {}",
                list_columns(&info)
            )));
        }
    };
    let Some((t, c)) = info.tables.iter().find_map(|t| {
        t.columns
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(&x))
            .map(|c| (t, c))
    }) else {
        return Err(usage(format!(
            "{file}: no table column named {x}; columns: {}",
            list_columns(&info)
        )));
    };
    if t.columns[c].extra.contains_key("categories") {
        return Err(usage(format!("{file}: column {x} holds text, not numbers")));
    }
    let rows = t.row_count.min(MAX_ROWS);
    ctx.spend(rows as usize)?;
    let tab = ds.read_table(t.index, 0, rows)?;
    let values: Vec<f64> = tab
        .columns
        .get(c)
        .map(|v| v.iter().copied().filter(|x| x.is_finite()).collect())
        .unwrap_or_default();
    if t.row_count > rows {
        ctx.warnings.push(format!(
            "{}: binned the first {rows} of {} rows",
            super::file_label(file),
            t.row_count
        ));
    }
    if t.extra.get("read_count_exact").and_then(Value::as_bool) == Some(false) {
        ctx.notes.push(format!(
            "{}: the row count of this file is estimated; every row read was binned",
            super::file_label(file)
        ));
    }
    let col = &t.columns[c];
    let table_name = t
        .name
        .clone()
        .unwrap_or_else(|| format!("table {}", t.index));
    let yname = if info.format.family == "sequencing" && table_name == "reads" {
        "reads"
    } else {
        "rows"
    };
    Ok(vec![
        Series::new(
            col.name.clone(),
            Geom::Hist(values),
            MarkKind::Area,
            Source::file(file, format!("{table_name}, column {}", col.name)),
        )
        .axes(
            Quantity::new(col.name.replace('_', " "), col.unit.as_deref()),
            Quantity::new(yname, None),
        ),
    ])
}

fn list_columns(info: &openreadout_core::FileInfo) -> String {
    info.tables
        .iter()
        .flat_map(|t| t.columns.iter().map(|c| c.name.clone()))
        .collect::<Vec<_>>()
        .join(", ")
}
