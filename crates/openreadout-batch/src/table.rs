//! The tidy table: typed cells, columns with a role and a unit, and rows assembled from the
//! name/value pairs measures produce.

use std::collections::HashMap;
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One cell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(untagged)]
pub enum Value {
    /// No value (JSON `null`, an empty CSV field).
    #[default]
    Null,
    /// `true` / `false`.
    Bool(bool),
    /// A whole number.
    Int(i64),
    /// A real number (NaN and infinities are written as `null`).
    Float(f64),
    /// Text.
    Text(String),
}

impl Value {
    /// The value as a number (integers, floats, and text that is a plain decimal number).
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(v) => Some(*v as f64),
            Value::Float(v) if v.is_finite() => Some(*v),
            Value::Text(s) => parse_number(s),
            _ => None,
        }
    }
    /// True for [`Value::Null`] (and non-finite floats).
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null) || matches!(self, Value::Float(v) if !v.is_finite())
    }
    /// The value as text: empty for null, shortest round-trip form for numbers.
    pub fn text(&self) -> String {
        match self {
            Value::Null => String::new(),
            Value::Bool(b) => b.to_string(),
            Value::Int(v) => v.to_string(),
            Value::Float(v) if !v.is_finite() => String::new(),
            Value::Float(v) => fmt_float(*v),
            Value::Text(s) => s.clone(),
        }
    }
    /// A text value, or null for `None`/empty text.
    pub fn text_opt(s: Option<&str>) -> Value {
        match s.map(str::trim) {
            Some(t) if !t.is_empty() => Value::Text(t.to_string()),
            _ => Value::Null,
        }
    }
    /// A float, or null for `None` and non-finite values.
    pub fn float_opt(v: Option<f64>) -> Value {
        match v {
            Some(x) if x.is_finite() => Value::Float(x),
            _ => Value::Null,
        }
    }
    /// An unsigned count (saturating at `i64::MAX`).
    pub fn count(v: u64) -> Value {
        Value::Int(i64::try_from(v).unwrap_or(i64::MAX))
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text())
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::float_opt(Some(v))
    }
}
impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int(v)
    }
}
impl From<u64> for Value {
    fn from(v: u64) -> Self {
        Value::count(v)
    }
}
impl From<u32> for Value {
    fn from(v: u32) -> Self {
        Value::Int(i64::from(v))
    }
}
impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}
impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::Text(v)
    }
}
impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::Text(v.to_string())
    }
}
impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Self {
        v.map_or(Value::Null, Into::into)
    }
}

/// Shortest text that parses back to the same f64; integers without a fraction.
pub fn fmt_float(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{v:.0}")
    } else {
        format!("{v}")
    }
}

/// Parse a plain decimal number (`-1.5`, `2.1e3`, `+7`, `1,000` is not one). Rejects `NaN`,
/// `inf`, thousands separators and decimal commas.
pub fn parse_number(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() || t.len() > 64 {
        return None;
    }
    let body = t.strip_prefix(['+', '-']).unwrap_or(t);
    let mut digits = 0;
    let mut seen_e = false;
    let mut seen_dot = false;
    let mut prev = ' ';
    for c in body.chars() {
        match c {
            '0'..='9' => digits += 1,
            '.' if !seen_e && !seen_dot => seen_dot = true,
            'e' | 'E' if digits > 0 && !seen_e => seen_e = true,
            '+' | '-' if matches!(prev, 'e' | 'E') => {}
            _ => return None,
        }
        prev = c;
    }
    if digits == 0 {
        return None;
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Column type, inferred from the cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ColumnType {
    /// Text.
    String,
    /// Whole numbers.
    Integer,
    /// Real numbers (a column mixing integers and reals is `float`).
    Float,
    /// `true`/`false`.
    Boolean,
}

/// What a column holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Which data set a row comes from: `path`, `format`, `files`.
    Id,
    /// The row's position within its data set (the measure's grain): `channel`, `population`,
    /// `well`, `parameter`, …
    Key,
    /// Joined from a sample sheet or plate layout.
    Annotation,
    /// Recorded in the file's own metadata (sample id, well, instrument).
    Metadata,
    /// A measured or computed value.
    Value,
    /// `error`, `error_code`.
    Error,
}

/// One column.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Column {
    /// Name (stable; units that never change are part of it, e.g. `sample_rate_hz`).
    pub name: String,
    /// Inferred type.
    #[serde(rename = "type")]
    pub kind: ColumnType,
    /// Unit of every value in the column, when fixed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// What the column holds.
    pub role: Role,
    /// One line on its meaning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// What a measure says about a column it produces (a name ending in `*` covers every column
/// with that prefix, e.g. `median:*`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnDoc {
    /// Name or `prefix*`.
    pub name: &'static str,
    /// Role (default [`Role::Value`]).
    pub role: Role,
    /// Unit, when fixed.
    pub unit: Option<&'static str>,
    /// Meaning.
    pub description: &'static str,
}

impl ColumnDoc {
    /// A value column.
    pub const fn value(name: &'static str, description: &'static str) -> Self {
        ColumnDoc {
            name,
            role: Role::Value,
            unit: None,
            description,
        }
    }
    /// A value column with a unit.
    pub const fn unit(name: &'static str, unit: &'static str, description: &'static str) -> Self {
        ColumnDoc {
            name,
            role: Role::Value,
            unit: Some(unit),
            description,
        }
    }
    /// A key (grain) column.
    pub const fn key(name: &'static str, description: &'static str) -> Self {
        ColumnDoc {
            name,
            role: Role::Key,
            unit: None,
            description,
        }
    }
    /// A metadata column.
    pub const fn meta(name: &'static str, description: &'static str) -> Self {
        ColumnDoc {
            name,
            role: Role::Metadata,
            unit: None,
            description,
        }
    }
    fn matches(&self, name: &str) -> bool {
        match self.name.strip_suffix('*') {
            Some(prefix) => name.starts_with(prefix),
            None => self.name == name,
        }
    }
}

/// One row as a measure builds it: ordered name/value pairs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Row {
    /// Cells in column order.
    pub cells: Vec<(String, Value)>,
}

impl Row {
    /// An empty row.
    pub fn new() -> Self {
        Row::default()
    }
    /// Append (or replace) a cell.
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<Value>) -> &mut Self {
        let name = name.into();
        let value = value.into();
        if let Some(c) = self.cells.iter_mut().find(|(n, _)| *n == name) {
            c.1 = value;
        } else {
            self.cells.push((name, value));
        }
        self
    }
    /// Builder form of [`Row::set`].
    #[must_use]
    pub fn with(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        self.set(name, value);
        self
    }
    /// The value of `name`, if set.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.cells.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }
}

/// A table: columns and rows of cells in column order.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Table {
    /// Columns.
    pub columns: Vec<Column>,
    /// Rows; each has one cell per column.
    pub rows: Vec<Vec<Value>>,
}

impl Table {
    /// Index of the column called `name`.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }
    /// The cell of `row` in column `name` (null when the column is absent).
    pub fn get(&self, row: usize, name: &str) -> &Value {
        static NULL: Value = Value::Null;
        self.index_of(name)
            .and_then(|i| self.rows.get(row).and_then(|r| r.get(i)))
            .unwrap_or(&NULL)
    }
    /// Keep only these columns (in this order); unknown names are an error.
    pub fn select(&mut self, names: &[String]) -> Result<(), String> {
        let mut idx = Vec::new();
        for n in names {
            match self.index_of(n) {
                Some(i) => idx.push(i),
                None => {
                    return Err(format!(
                        "no column `{n}`; the table has: {}",
                        self.columns
                            .iter()
                            .map(|c| c.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
        }
        self.columns = idx.iter().map(|&i| self.columns[i].clone()).collect();
        for r in &mut self.rows {
            *r = idx.iter().map(|&i| r[i].clone()).collect();
        }
        Ok(())
    }
    /// Re-infer every column's type from its cells (numbers in text cells stay text).
    pub fn infer_types(&mut self) {
        for (i, c) in self.columns.iter_mut().enumerate() {
            c.kind = infer(self.rows.iter().map(|r| &r[i]));
        }
    }
}

/// The type of a column holding these cells.
pub fn infer<'a>(cells: impl Iterator<Item = &'a Value>) -> ColumnType {
    let (mut int, mut float, mut boolean, mut text) = (false, false, false, false);
    for v in cells {
        match v {
            Value::Null => {}
            Value::Int(_) => int = true,
            Value::Float(_) => float = true,
            Value::Bool(_) => boolean = true,
            Value::Text(_) => text = true,
        }
    }
    if text || (boolean && (int || float)) {
        ColumnType::String
    } else if float {
        ColumnType::Float
    } else if int {
        ColumnType::Integer
    } else if boolean {
        ColumnType::Boolean
    } else {
        ColumnType::String
    }
}

/// Assemble rows into a table: columns in first-seen order (a column first seen in a later
/// row goes after the columns of its role that came before it), typed from their cells,
/// described by `docs`. `role_of` gives the role of columns no doc covers.
pub fn assemble(rows: Vec<Row>, docs: &[ColumnDoc], role_of: &dyn Fn(&str) -> Role) -> Table {
    let mut names: Vec<String> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for r in &rows {
        for (n, _) in &r.cells {
            if !at.contains_key(n) {
                at.insert(n.clone(), names.len());
                names.push(n.clone());
            }
        }
    }
    let role = |n: &str| {
        docs.iter()
            .find(|d| d.matches(n))
            .map_or_else(|| role_of(n), |d| d.role)
    };
    // stable order by role group, first-seen within a group
    let rank = |r: Role| match r {
        Role::Id => 0,
        Role::Key => 1,
        Role::Metadata => 2,
        Role::Annotation => 3,
        Role::Value => 4,
        Role::Error => 5,
    };
    let mut order: Vec<usize> = (0..names.len()).collect();
    order.sort_by_key(|&i| (rank(role(&names[i])), i));
    let pos: HashMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(p, &i)| (names[i].as_str(), p))
        .collect();
    let mut out_rows = Vec::with_capacity(rows.len());
    for r in rows {
        let mut cells = vec![Value::Null; names.len()];
        for (n, v) in r.cells {
            if let Some(&p) = pos.get(n.as_str()) {
                cells[p] = v;
            }
        }
        out_rows.push(cells);
    }
    let columns = order
        .iter()
        .enumerate()
        .map(|(p, &i)| {
            let n = &names[i];
            let doc = docs.iter().find(|d| d.matches(n));
            Column {
                name: n.clone(),
                kind: infer(out_rows.iter().map(|r| &r[p])),
                unit: doc.and_then(|d| d.unit.map(str::to_string)),
                role: role(n),
                description: doc.map(|d| d.description.to_string()),
            }
        })
        .collect();
    Table {
        columns,
        rows: out_rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_parse_strictly() {
        assert_eq!(parse_number(" 1.5 "), Some(1.5));
        assert_eq!(parse_number("-2e3"), Some(-2000.0));
        assert_eq!(parse_number("1,000"), None);
        assert_eq!(parse_number("NaN"), None);
        assert_eq!(parse_number("."), None);
        assert_eq!(parse_number("1.2.3"), None);
        assert_eq!(parse_number("A1"), None);
    }

    #[test]
    fn assemble_orders_by_role_then_first_seen_and_types_columns() {
        let docs = [
            ColumnDoc {
                name: "path",
                role: Role::Id,
                unit: None,
                description: "file",
            },
            ColumnDoc::key("channel", "channel"),
            ColumnDoc::unit("rate_hz", "Hz", "rate"),
            ColumnDoc {
                name: "error",
                role: Role::Error,
                unit: None,
                description: "why",
            },
        ];
        let rows = vec![
            Row::new()
                .with("path", "a")
                .with("mean", 1.5)
                .with("channel", 0u32),
            Row::new()
                .with("path", "b")
                .with("error", "bad")
                .with("rate_hz", 10u32)
                .with("mean", Value::Int(2)),
        ];
        let t = assemble(rows, &docs, &|_| Role::Value);
        let names: Vec<&str> = t.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["path", "channel", "mean", "rate_hz", "error"]);
        assert_eq!(t.columns[2].kind, ColumnType::Float);
        assert_eq!(t.columns[3].unit.as_deref(), Some("Hz"));
        assert_eq!(t.rows[1][1], Value::Null);
        assert_eq!(t.get(1, "error"), &Value::Text("bad".into()));
    }
}
