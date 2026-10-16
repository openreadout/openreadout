//! Small in-memory tables shared by the readers: named columns of numbers (or text, stored as
//! codes into a category list), turned into `TableInfo` for `info` and sliced for `read_table`.

use std::collections::BTreeMap;

use openreadout_core::model::{ColumnInfo, Table, TableInfo};
use serde_json::{Value, json};

/// One column.
#[derive(Debug, Clone, Default)]
pub struct Col {
    /// Column name.
    pub name: String,
    /// Unit, when the values have one.
    pub unit: Option<&'static str>,
    /// Values (codes into `categories` for text columns).
    pub values: Vec<f64>,
    /// Labels of a text column.
    pub categories: Option<Vec<String>>,
    /// dtype reported for the column.
    pub dtype: &'static str,
}

impl Col {
    /// A numeric column (`float64`).
    pub fn num(name: impl Into<String>, unit: Option<&'static str>) -> Self {
        Col {
            name: name.into(),
            unit,
            dtype: "float64",
            ..Col::default()
        }
    }
    /// An integer column (`int64`; values are whole numbers).
    pub fn int(name: impl Into<String>) -> Self {
        Col {
            name: name.into(),
            dtype: "int64",
            ..Col::default()
        }
    }
    /// A text column.
    pub fn text(name: impl Into<String>) -> Self {
        Col {
            name: name.into(),
            dtype: "uint32",
            categories: Some(Vec::new()),
            ..Col::default()
        }
    }
    /// Push a number (`None` becomes NaN).
    pub fn push(&mut self, v: Option<f64>) {
        self.values.push(v.unwrap_or(f64::NAN));
    }
    /// Push a text value.
    pub fn push_text(&mut self, t: &str) {
        let cats = self.categories.get_or_insert_with(Vec::new);
        let code = if let Some(i) = cats.iter().position(|c| c == t) {
            i
        } else {
            cats.push(t.to_string());
            cats.len() - 1
        };
        self.values.push(code as f64);
    }
}

/// A table held in memory.
#[derive(Debug, Clone, Default)]
pub struct MemTable {
    /// Table name.
    pub name: &'static str,
    /// Columns.
    pub cols: Vec<Col>,
    /// `extra` of the table's `TableInfo`.
    pub extra: BTreeMap<String, Value>,
}

impl MemTable {
    /// A named table with these columns.
    pub fn new(name: &'static str, cols: Vec<Col>) -> Self {
        MemTable {
            name,
            cols,
            extra: BTreeMap::new(),
        }
    }

    /// Rows.
    pub fn rows(&self) -> u64 {
        self.cols.first().map_or(0, |c| c.values.len() as u64)
    }

    /// The column called `name`.
    pub fn col(&mut self, name: &str) -> &mut Col {
        let i = self
            .cols
            .iter()
            .position(|c| c.name == name)
            .unwrap_or_else(|| panic!("no column {name}"));
        &mut self.cols[i]
    }

    /// `TableInfo` for `info`.
    pub fn info(&self, index: u32) -> TableInfo {
        TableInfo {
            index,
            name: Some(self.name.to_string()),
            row_count: self.rows(),
            columns: self
                .cols
                .iter()
                .enumerate()
                .map(|(k, c)| ColumnInfo {
                    index: k as u32,
                    name: c.name.clone(),
                    label: None,
                    dtype: c.dtype.to_string(),
                    unit: c.unit.map(str::to_string),
                    range: None,
                    extra: c
                        .categories
                        .as_ref()
                        .map(|cats| BTreeMap::from([("categories".to_string(), json!(cats))]))
                        .unwrap_or_default(),
                })
                .collect(),
            extra: self.extra.clone(),
        }
    }

    /// Rows `[first, first + max)`.
    pub fn slice(&self, index: u32, first: u64, max: u64) -> Table {
        let n = self.rows();
        let a = first.min(n) as usize;
        let b = first.saturating_add(max).min(n) as usize;
        Table {
            table: index,
            first_row: first,
            columns: self.cols.iter().map(|c| c.values[a..b].to_vec()).collect(),
        }
    }
}

/// Round to a sensible number of significant digits for JSON summaries.
pub fn tidy(v: f64, digits: i32) -> f64 {
    if !v.is_finite() || v == 0.0 {
        return v;
    }
    let mag = v.abs().log10().floor() as i32;
    let p = 10f64.powi(digits - 1 - mag);
    (v * p).round() / p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_columns_and_slices() {
        let mut t = MemTable::new("t", vec![Col::text("s"), Col::num("v", Some("%"))]);
        for (s, v) in [("a", 1.0), ("b", 2.0), ("a", 3.0)] {
            t.col("s").push_text(s);
            t.col("v").push(Some(v));
        }
        let i = t.info(0);
        assert_eq!(i.columns[0].extra["categories"], json!(["a", "b"]));
        let s = t.slice(0, 1, 5);
        assert_eq!(s.columns[0], vec![1.0, 0.0]);
        assert_eq!(tidy(95.709_320_06, 4), 95.71);
    }
}
