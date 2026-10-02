//! `table --tidy`: tables summarized per data set.
//!
//! - **FCS** (and every other table without plate wells): one row per data set × table ×
//!   parameter (column): `events` (finite values), `median`, `mean`, `sd`, `min`, `max`. FCS
//!   values are scale values (`$PnE`/`$PnG` applied), optionally compensated and transformed
//!   like `table --compensate/--transform`.
//! - **Plate reads** (tables with `row` and `col` columns): one row per well and read, with the
//!   well named `A01`, so plate layouts join onto it.

use openreadout_core::{Error, Result};
use openreadout_fcs::analysis::{TableOptions, median, processed_rows};

use crate::measure::{Item, Measure};
use crate::table::{ColumnDoc, Row, Value};
use crate::well;

/// Table summaries.
#[derive(Debug, Clone, Default)]
pub struct TableMeasure {
    /// Only this table (FCS data set, plate read block); default: every table.
    pub table: Option<u32>,
    /// Only these parameters/columns (`$PnN` or `$PnS`); default: all.
    pub parameters: Vec<String>,
    /// FCS processing (compensation, transforms); populations are not allowed here (use
    /// `gate --tidy`).
    pub fcs: TableOptions,
    /// Text form of the FCS options, for the journal fingerprint.
    pub options_text: String,
}

/// Summary of one column's values.
fn summary(r: &mut Row, mut v: Vec<f64>) {
    v.retain(|x| x.is_finite());
    let n = v.len();
    r.set("events", n as u64);
    if n == 0 {
        return;
    }
    let mean = v.iter().sum::<f64>() / n as f64;
    let sd = (n > 1)
        .then(|| (v.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / (n - 1) as f64).sqrt());
    let min = v.iter().copied().fold(f64::INFINITY, f64::min);
    let max = v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    r.set("median", Value::float_opt(median(&mut v)));
    r.set("mean", mean);
    r.set("sd", Value::float_opt(sd));
    r.set("min", min);
    r.set("max", max);
}

impl TableMeasure {
    fn wanted(&self, name: &str, label: Option<&str>) -> bool {
        self.parameters.is_empty()
            || self
                .parameters
                .iter()
                .any(|p| p == name || Some(p.as_str()) == label)
    }
}

impl Measure for TableMeasure {
    fn id(&self) -> &'static str {
        "table"
    }
    fn grain(&self) -> Vec<String> {
        vec![
            "table".into(),
            "parameter".into(),
            "well".into(),
            "read".into(),
        ]
    }
    fn columns(&self) -> Vec<ColumnDoc> {
        vec![
            ColumnDoc::key(
                "table",
                "table index (FCS data set, plate read block), from 0",
            ),
            ColumnDoc::key("table_name", "table name as recorded"),
            ColumnDoc::key("parameter", "column name (FCS $PnN)"),
            ColumnDoc::key("label", "column label (FCS $PnS: marker or dye)"),
            ColumnDoc::key("well", "plate well, as A01"),
            ColumnDoc::key("read", "plate read (1-based, per table)"),
            ColumnDoc::value("row", "plate row, from 1"),
            ColumnDoc::value("col", "plate column, from 1"),
            ColumnDoc::unit("wavelength_nm", "nm", "read wavelength"),
            ColumnDoc::unit("time_s", "s", "time of a kinetic read"),
            ColumnDoc::value("value", "the plate reader's value for the well"),
            ColumnDoc::value("events", "finite values (FCS: events) summarized"),
            ColumnDoc::value("median", "median (mean of the two middle values when even)"),
            ColumnDoc::value("mean", "mean"),
            ColumnDoc::value("sd", "sample standard deviation (n − 1)"),
            ColumnDoc::value("min", "smallest value"),
            ColumnDoc::value("max", "largest value"),
        ]
    }
    fn fingerprint(&self) -> String {
        format!(
            "table:{:?}:{:?}:{}",
            self.table, self.parameters, self.options_text
        )
    }
    fn rows(&self, it: &mut Item<'_>) -> Result<Vec<Row>> {
        if it.info.tables.is_empty() {
            return Err(super::not_applicable(
                "table",
                it.info,
                "tables (FCS events, plate reads, spike or peak tables)",
                "Use `stats --tidy` for images and `trace --tidy` for signals.",
            ));
        }
        let tables: Vec<_> = it
            .info
            .tables
            .iter()
            .filter(|t| self.table.is_none_or(|w| w == t.index))
            .cloned()
            .collect();
        if tables.is_empty() {
            return Err(Error::Usage(format!(
                "table {} not found (the file has {} tables)",
                self.table.unwrap_or(0),
                it.info.tables.len()
            )));
        }
        let fcs = it.info.format.id == openreadout_fcs::FORMAT_ID;
        if !fcs && self.fcs.is_active() {
            return Err(Error::Usage(format!(
                "compensation and transforms apply to FCS files; this is {}",
                it.info.format.id
            )));
        }
        let mut rows = Vec::new();
        for t in &tables {
            let base = |r: Row| {
                let mut r = r;
                if let Some(n) = &t.name {
                    r.set("table_name", n.as_str());
                }
                r
            };
            if fcs {
                let p = processed_rows(it.path, t.index, 0, t.row_count, &self.fcs)?;
                for (i, (name, col)) in p.names.iter().zip(p.columns).enumerate() {
                    if name.starts_with("gate:") {
                        continue;
                    }
                    let label = p.labels.get(i).cloned().flatten();
                    if !self.wanted(name, label.as_deref()) {
                        continue;
                    }
                    let mut r = base(Row::new().with("table", t.index));
                    r.set("parameter", name.as_str());
                    r.set("label", Value::text_opt(label.as_deref()));
                    summary(&mut r, col);
                    rows.push(r);
                }
                continue;
            }
            let data = it.dataset.read_table(t.index, 0, t.row_count)?;
            let pos = |n: &str| t.columns.iter().position(|c| c.name == n);
            if let (Some(rc), Some(cc)) = (pos("row"), pos("col").or_else(|| pos("column"))) {
                // plate read: one row per table row, the well named A01
                let n = data.columns.first().map_or(0, Vec::len);
                for i in 0..n {
                    let cell = |c: usize| data.columns.get(c).and_then(|v| v.get(i)).copied();
                    let w = match (cell(rc), cell(cc)) {
                        (Some(r), Some(c)) if r >= 1.0 && c >= 1.0 => Some(well::Well {
                            row: r as u32 - 1,
                            col: c as u32 - 1,
                        }),
                        _ => None,
                    };
                    let mut r = base(Row::new().with("table", t.index));
                    r.set("well", Value::text_opt(w.map(well::Well::name).as_deref()));
                    for (ci, col) in t.columns.iter().enumerate() {
                        if col.name == "well" {
                            continue;
                        }
                        let v = cell(ci);
                        let v = match col.name.as_str() {
                            "row" | "col" | "column" | "read" => {
                                v.map_or(Value::Null, |x| Value::Int(x as i64))
                            }
                            _ => Value::float_opt(v),
                        };
                        r.set(col.name.clone(), v);
                    }
                    rows.push(r);
                }
                continue;
            }
            for (ci, col) in t.columns.iter().enumerate() {
                if !self.wanted(&col.name, col.label.as_deref()) {
                    continue;
                }
                let mut r = base(Row::new().with("table", t.index));
                r.set("parameter", col.name.as_str());
                r.set("label", Value::text_opt(col.label.as_deref()));
                summary(&mut r, data.columns.get(ci).cloned().unwrap_or_default());
                rows.push(r);
            }
        }
        if rows.is_empty() && !self.parameters.is_empty() {
            return Err(Error::Usage(format!(
                "none of the parameters {} is in this file",
                self.parameters.join(", ")
            )));
        }
        Ok(rows)
    }
}
