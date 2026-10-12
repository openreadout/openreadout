//! The SRM/MRM transitions of a run as one table, so `info` can state how many there are and
//! `table` can list them. Readers build the rows from what the file records (method transitions,
//! or the scan records of each transition) and say how they counted.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::model::{ColumnInfo, Table, TableInfo};

/// One transition: a precursor (Q1), its product (Q3) and how it was measured.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Transition {
    /// The reader's own id of the transition (a scan method, an experiment), if it has one.
    pub id: Option<i64>,
    /// Precursor (Q1) m/z.
    pub precursor_mz: Option<f64>,
    /// Product (Q3) m/z; `None` when one record holds several products (`products` > 1).
    pub product_mz: Option<f64>,
    /// Products measured under this id (1 for a single transition).
    pub products: u32,
    /// Collision energy (V, as the file records it).
    pub collision_energy: Option<f64>,
    /// `positive`, `negative` or `unknown`.
    pub polarity: &'static str,
    /// Compound or transition name, if the method gives one.
    pub name: Option<String>,
    /// The retention-time window in minutes: scheduled, or first and last scan.
    pub rt_min: Option<[f64; 2]>,
}

/// The column names of the transitions table, in order.
pub const TRANSITION_COLUMNS: [&str; 8] = [
    "id",
    "precursor_mz",
    "product_mz",
    "products",
    "collision_energy",
    "polarity",
    "rt_start_min",
    "rt_end_min",
];

/// Name of the transitions table in `tables[]`.
pub const TRANSITIONS_TABLE: &str = "MRM transitions";

fn polarities(rows: &[Transition]) -> Vec<&'static str> {
    let mut p: Vec<&'static str> = rows.iter().map(|r| r.polarity).collect();
    p.sort_unstable();
    p.dedup();
    p
}

fn names(rows: &[Transition]) -> Vec<String> {
    rows.iter()
        .filter_map(|r| r.name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Number of transitions: every product of every row.
pub fn transition_count(rows: &[Transition]) -> u64 {
    rows.iter().map(|r| u64::from(r.products.max(1))).sum()
}

/// The run-level summary for `spectra[].extra.mrm_transitions`: the count, the distinct
/// precursor/product pairs (when every row is one transition), how the reader counted, and the
/// index of the table that lists them.
pub fn transition_summary(rows: &[Transition], counted_as: &str, table: u32) -> Value {
    let mut out = serde_json::Map::new();
    out.insert("count".into(), json!(transition_count(rows)));
    if rows.iter().all(|r| r.products <= 1) {
        let pairs: BTreeSet<(&str, Option<u64>, Option<u64>)> = rows
            .iter()
            .map(|r| {
                (
                    r.polarity,
                    r.precursor_mz.map(f64::to_bits),
                    r.product_mz.map(f64::to_bits),
                )
            })
            .collect();
        out.insert("precursor_product_pairs".into(), json!(pairs.len()));
    }
    out.insert("counted_as".into(), json!(counted_as));
    out.insert("table".into(), json!(table));
    Value::Object(out)
}

/// `tables[]` entry for the transitions; `id_name` says what the id column holds.
pub fn transition_table_info(rows: &[Transition], index: u32, id_name: &str) -> TableInfo {
    let col = |i: usize, label: &str, unit: Option<&str>| ColumnInfo {
        index: i as u32,
        name: TRANSITION_COLUMNS[i].into(),
        label: Some(label.into()),
        dtype: "float64".into(),
        unit: unit.map(Into::into),
        range: None,
        extra: BTreeMap::new(),
    };
    let mut columns = vec![
        col(0, id_name, None),
        col(1, "precursor (Q1) m/z", Some("m/z")),
        col(
            2,
            "product (Q3) m/z; empty when the row holds several products",
            Some("m/z"),
        ),
        col(3, "products measured under this id", None),
        col(4, "collision energy", Some("V")),
        col(5, "polarity (index into categories)", None),
        col(6, "window start", Some("min")),
        col(7, "window end", Some("min")),
    ];
    columns[5]
        .extra
        .insert("categories".into(), json!(polarities(rows)));
    let names = names(rows);
    if !names.is_empty() {
        columns.push(ColumnInfo {
            index: 8,
            name: "name".into(),
            label: Some("compound or transition name (index into categories)".into()),
            dtype: "float64".into(),
            unit: None,
            range: None,
            extra: BTreeMap::from([("categories".to_string(), json!(names))]),
        });
    }
    TableInfo {
        index,
        name: Some(TRANSITIONS_TABLE.into()),
        row_count: rows.len() as u64,
        columns,
        extra: BTreeMap::new(),
    }
}

/// Rows `first_row..first_row + max_rows` of the transitions table.
pub fn transition_table(rows: &[Transition], index: u32, first_row: u64, max_rows: u64) -> Table {
    let pols = polarities(rows);
    let names = names(rows);
    let start = usize::try_from(first_row)
        .unwrap_or(usize::MAX)
        .min(rows.len());
    let end = start
        .saturating_add(usize::try_from(max_rows).unwrap_or(usize::MAX))
        .min(rows.len());
    let n_cols = if names.is_empty() { 8 } else { 9 };
    let mut columns = vec![Vec::with_capacity(end - start); n_cols];
    let opt = |v: Option<f64>| v.unwrap_or(f64::NAN);
    for r in &rows[start..end] {
        let pol = pols.iter().position(|p| *p == r.polarity);
        let vals = [
            r.id.map_or(f64::NAN, |i| i as f64),
            opt(r.precursor_mz),
            opt(r.product_mz),
            f64::from(r.products),
            opt(r.collision_energy),
            pol.map_or(f64::NAN, |i| i as f64),
            r.rt_min.map_or(f64::NAN, |w| w[0]),
            r.rt_min.map_or(f64::NAN, |w| w[1]),
        ];
        for (c, v) in vals.into_iter().enumerate() {
            columns[c].push(v);
        }
        if n_cols == 9 {
            let k = r
                .name
                .as_ref()
                .and_then(|n| names.iter().position(|x| x == n));
            columns[8].push(k.map_or(f64::NAN, |i| i as f64));
        }
    }
    Table {
        table: index,
        first_row: start as u64,
        columns,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_rows_products_and_pairs() {
        let t = |q1: f64, q3: f64, id: i64| Transition {
            id: Some(id),
            precursor_mz: Some(q1),
            product_mz: Some(q3),
            products: 1,
            polarity: "positive",
            rt_min: Some([1.0, 2.0]),
            ..Transition::default()
        };
        // the same pair in two windows counts twice
        let rows = vec![t(100.0, 50.0, 1), t(100.0, 50.0, 2), t(120.0, 60.0, 3)];
        let s = transition_summary(&rows, "test", 0);
        assert_eq!(s["count"], 3);
        assert_eq!(s["precursor_product_pairs"], 2);
        let info = transition_table_info(&rows, 0, "scan method");
        assert_eq!(info.row_count, 3);
        assert_eq!(info.columns.len(), 8);
        let tab = transition_table(&rows, 0, 1, 10);
        assert_eq!(tab.first_row, 1);
        assert_eq!(tab.columns[0], vec![2.0, 3.0]);
        assert_eq!(tab.columns[5], vec![0.0, 0.0]);
        let mut multi = t(200.0, 0.0, 4);
        multi.product_mz = None;
        multi.products = 4;
        let s = transition_summary(&[multi], "test", 0);
        assert_eq!(s["count"], 4);
        assert!(s.get("precursor_product_pairs").is_none());
        assert!(transition_table(&rows, 0, 9, 10).columns[0].is_empty());
    }
}
