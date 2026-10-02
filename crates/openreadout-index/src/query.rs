//! The search query language.
//!
//! A query is a list of terms separated by spaces; all terms must match (AND). `OR` between
//! terms splits the query into alternatives: `a=1 b=2 OR c=3` is `(a=1 AND b=2) OR c=3`.
//!
//! | term | meaning |
//! | --- | --- |
//! | `field=value` | equal (text: case-insensitive; lists: any element; dates: within that year/month/day) |
//! | `field!=value` | not equal |
//! | `field~text` | contains (case-insensitive; lists: any element) |
//! | `field!~text` | does not contain |
//! | `field<v`, `<=`, `>`, `>=` | compare numbers (with units: `size>1GB`, `rate>=20kHz`, `pixel<0.2um`, `duration>30min`) or dates (`acquired<2020`, `acquired>=2021-06`) |
//! | `field:value` | has the term: `technique:FBbi_00000246` (`_` or `:`), or a label word (`technique:confocal`); on other fields, contains |
//! | `-term` | negation of any term |
//! | `a|b` | alternatives within one term: `format=czi|nd2|lif` |
//! | `word` | a bare word matches if the path, sample, method, measurement, channels, instrument, technique, format or operator contains it |
//! | `"a b"` | quotes keep spaces in a value: `sample~"plate 3"` |
//!
//! Fields are the columns of `experiments.parquet` plus the aliases in [`FIELDS`] (`objective`
//! compares the magnification when the value is a number, `63x`, and the objective model
//! otherwise; `param.<name>` looks inside `method.parameters`, e.g. `param.nucleus=1H`).

use openreadout_core::{Error, Result};

use crate::tables::{Cell, ColumnDef, EXPERIMENTS, Rows, UnitClass, micros_from_iso};

/// A query alias: its name, the columns it searches, and what it means.
#[derive(Debug, Clone, Copy, serde::Serialize, schemars::JsonSchema)]
pub struct FieldAlias {
    /// The name used in queries.
    pub name: &'static str,
    /// Columns searched (a term matches if any of them matches).
    pub columns: &'static [&'static str],
    /// Meaning.
    pub description: &'static str,
}

const fn fa(
    name: &'static str,
    columns: &'static [&'static str],
    description: &'static str,
) -> FieldAlias {
    FieldAlias {
        name,
        columns,
        description,
    }
}

/// Query field aliases (every column name also works as a field).
pub const FIELDS: &[FieldAlias] = &[
    fa(
        "sample",
        &[
            "sample_id",
            "sample_name",
            "sample_well",
            "sample_barcode",
            "sample_position",
        ],
        "sample id, name, well, barcode or position",
    ),
    fa("well", &["sample_well"], "plate well"),
    fa("barcode", &["sample_barcode"], "plate or tube barcode"),
    fa(
        "instrument",
        &["instrument_vendor", "instrument_model", "instrument_serial"],
        "instrument vendor, model or serial",
    ),
    fa(
        "vendor",
        &["instrument_vendor", "format_vendor"],
        "instrument or format vendor",
    ),
    fa("model", &["instrument_model"], "instrument model"),
    fa("serial", &["instrument_serial"], "instrument serial number"),
    fa(
        "software",
        &["instrument_software", "instrument_software_version"],
        "acquisition software and version",
    ),
    fa(
        "device",
        &["instrument_kind_id", "instrument_kind_label"],
        "OBI device term (`device:OBI_0400169`, `device~confocal`)",
    ),
    fa(
        "method",
        &["method_name"],
        "method, protocol or experiment name",
    ),
    fa(
        "technique",
        &["technique_id", "technique_label"],
        "technique term (CHMO, FBbi, OBI)",
    ),
    fa("assay", &["assay_id", "assay_label"], "OBI assay term"),
    fa(
        "term",
        &["terms", "term_labels"],
        "any term in the experiment",
    ),
    fa(
        "acquired",
        &["started_at"],
        "acquisition start (date or partial date)",
    ),
    fa("date", &["started_at"], "acquisition start"),
    fa("year", &["acquired_year"], "acquisition year"),
    fa(
        "duration",
        &["duration_s"],
        "length of the recorded data (`duration>10min`)",
    ),
    fa("what", &["what"], "measurement descriptions"),
    fa(
        "channel",
        &["channels", "table_columns"],
        "channel names, fluorophores, FCS parameter names and labels",
    ),
    fa(
        "objective",
        &["objective_magnification", "objective"],
        "magnification when the value is a number (`63x`), else the objective model",
    ),
    fa("na", &["objective_na"], "objective numerical aperture"),
    fa(
        "pixel",
        &["physical_size_x_um"],
        "pixel size (`pixel<0.1um`)",
    ),
    fa("z_step", &["physical_size_z_um"], "z-step"),
    fa("x", &["size_x"], "image width, pixels"),
    fa("y", &["size_y"], "image height, pixels"),
    fa("z", &["size_z"], "z-slices"),
    fa("c", &["size_c"], "channels"),
    fa("t", &["size_t"], "time points"),
    fa("images", &["image_count"], "images"),
    fa("planes", &["plane_count"], "planes"),
    fa("rows", &["table_rows"], "table rows"),
    fa(
        "events",
        &["table_rows"],
        "flow-cytometry events (table rows)",
    ),
    fa("columns", &["table_columns"], "table column names"),
    fa("traces", &["trace_count"], "traces"),
    fa("rate", &["sample_rate_hz"], "sample rate (`rate>=20kHz`)"),
    fa("sweeps", &["trace_sweeps"], "sweeps"),
    fa("scans", &["scan_count"], "mass spectra"),
    fa("ms_level", &["ms_levels"], "MS levels present"),
    fa("rt", &["rt_max_s"], "last retention time"),
    fa("size", &["size_bytes"], "bytes (`size>1GB`)"),
    fa("modified", &["mtime"], "file modification time"),
    fa("status", &["check_status"], "integrity status"),
    fa("check", &["check_status"], "integrity status"),
    fa(
        "problem",
        &["check_codes", "error_code"],
        "finding or error code",
    ),
    fa(
        "pii",
        &["pii_kinds"],
        "personal-data kind flagged (`pii=true` for any)",
    ),
    fa("risk", &["preservation"], "`open`, `vendor` or `legacy`"),
];

/// Columns a bare word searches.
pub const TEXT_COLUMNS: &[&str] = &[
    "path",
    "sample_id",
    "sample_name",
    "method_name",
    "what",
    "channels",
    "instrument_model",
    "technique_label",
    "format",
    "operator",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Eq,
    Contains,
    Lt,
    Le,
    Gt,
    Ge,
    Has,
}

#[derive(Debug, Clone)]
enum Target {
    Columns(Vec<&'static str>),
    Objective,
    Param(String),
    Text,
    PiiAny,
}

#[derive(Debug, Clone)]
struct Clause {
    negate: bool,
    target: Target,
    op: Op,
    values: Vec<String>,
}

/// A parsed query.
#[derive(Debug, Clone, Default)]
pub struct Query {
    groups: Vec<Vec<Clause>>,
    text: String,
}

fn column(name: &str) -> Option<&'static ColumnDef> {
    EXPERIMENTS.iter().find(|c| c.name == name)
}

/// Resolve a field name to the columns it searches (aliases, then column names).
pub fn resolve_field(name: &str) -> Option<Vec<&'static str>> {
    let lower = name.to_ascii_lowercase();
    if let Some(a) = FIELDS.iter().find(|a| a.name == lower) {
        return Some(a.columns.to_vec());
    }
    column(&lower).map(|c| vec![c.name])
}

fn tokenize(q: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for ch in q.chars() {
        match quote {
            Some(qc) if ch == qc => quote = None,
            Some(_) => cur.push(ch),
            None if ch == '"' || ch == '\'' => {
                quote = Some(ch);
                started = true;
            }
            None if ch.is_whitespace() => {
                if started || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                started = false;
            }
            None => cur.push(ch),
        }
    }
    if quote.is_some() {
        return Err(Error::Usage(format!("unbalanced quote in query: {q}")));
    }
    if started || !cur.is_empty() {
        out.push(cur);
    }
    Ok(out)
}

fn parse_term(tok: &str) -> Result<Clause> {
    let (negate, body) = match tok.strip_prefix('-').or_else(|| tok.strip_prefix('!')) {
        Some(rest) if !rest.is_empty() => (true, rest),
        _ => (false, tok),
    };
    let name_len = body
        .char_indices()
        .take_while(|(i, c)| c.is_ascii_alphanumeric() || *c == '_' || (*c == '.' && *i > 0))
        .count();
    let (name, rest) = body.split_at(name_len);
    let ops: [(&str, Op, bool); 9] = [
        ("<=", Op::Le, false),
        (">=", Op::Ge, false),
        ("!=", Op::Eq, true),
        ("!~", Op::Contains, true),
        ("=", Op::Eq, false),
        ("~", Op::Contains, false),
        ("<", Op::Lt, false),
        (">", Op::Gt, false),
        (":", Op::Has, false),
    ];
    let found = (!name.is_empty())
        .then(|| ops.iter().find(|(s, _, _)| rest.starts_with(s)))
        .flatten();
    let Some((sym, op, neg_op)) = found else {
        // A bare word.
        return Ok(Clause {
            negate,
            target: Target::Text,
            op: Op::Contains,
            values: vec![body.to_string()],
        });
    };
    let value = &rest[sym.len()..];
    if value.is_empty() {
        return Err(Error::Usage(format!(
            "no value after `{name}{sym}` in the query"
        )));
    }
    let lname = name.to_ascii_lowercase();
    let target = if lname == "objective" {
        Target::Objective
    } else if let Some(p) = lname.strip_prefix("param.") {
        Target::Param(p.to_string())
    } else if lname == "pii"
        && matches!(
            value.to_ascii_lowercase().as_str(),
            "true" | "yes" | "any" | "false" | "no" | "none"
        )
    {
        Target::PiiAny
    } else if let Some(cols) = resolve_field(&lname) {
        Target::Columns(cols)
    } else {
        let mut known: Vec<&str> = FIELDS.iter().map(|a| a.name).collect();
        known.extend(EXPERIMENTS.iter().map(|c| c.name));
        return Err(Error::Usage(format!(
            "unknown query field `{name}`; fields: {}, param.<name> (see `openreadout search --help` or https://openreadout.github.io/openreadout/guides/lab-shares.html)",
            known.join(", ")
        )));
    };
    Ok(Clause {
        negate: negate ^ neg_op,
        target,
        op: *op,
        values: value.split('|').map(str::to_string).collect(),
    })
}

impl Query {
    /// Parse a query. The empty query matches everything.
    pub fn parse(q: &str) -> Result<Query> {
        let mut groups = vec![Vec::new()];
        for tok in tokenize(q)? {
            if tok == "OR" {
                groups.push(Vec::new());
                continue;
            }
            if tok == "AND" {
                continue;
            }
            groups
                .last_mut()
                .expect("one group")
                .push(parse_term(&tok)?);
        }
        if groups.len() > 1 && groups.iter().any(Vec::is_empty) {
            return Err(Error::Usage("`OR` needs terms on both sides".into()));
        }
        Ok(Query {
            groups,
            text: q.to_string(),
        })
    }

    /// The query text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Columns the query reads.
    pub fn columns(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        for c in self.groups.iter().flatten() {
            let cols: Vec<&'static str> = match &c.target {
                Target::Columns(v) => v.clone(),
                Target::Objective => vec!["objective_magnification", "objective"],
                Target::Param(_) => vec!["parameters_json"],
                Target::Text => TEXT_COLUMNS.to_vec(),
                Target::PiiAny => vec!["pii_count"],
            };
            for col in cols {
                if !out.contains(&col) {
                    out.push(col);
                }
            }
        }
        out
    }

    /// Does row `row` match?
    pub fn matches(&self, rows: &Rows, row: usize) -> bool {
        self.groups
            .iter()
            .any(|g| g.iter().all(|c| clause_matches(c, rows, row)))
    }
}

fn clause_matches(c: &Clause, rows: &Rows, row: usize) -> bool {
    let hit = c.values.iter().any(|v| value_matches(c, v, rows, row));
    hit != c.negate
}

#[allow(clippy::many_single_char_names)]
fn value_matches(c: &Clause, v: &str, rows: &Rows, row: usize) -> bool {
    match &c.target {
        Target::Columns(cols) => cols
            .iter()
            .any(|col| cell_matches(&rows.get(row, col), column(col), c.op, v)),
        Target::Text => TEXT_COLUMNS
            .iter()
            .any(|col| cell_matches(&rows.get(row, col), column(col), Op::Contains, v)),
        Target::Objective => {
            let num = v.trim_end_matches(['x', 'X']);
            if num.parse::<f64>().is_ok() {
                cell_matches(
                    &rows.get(row, "objective_magnification"),
                    column("objective_magnification"),
                    c.op,
                    v,
                )
            } else {
                cell_matches(&rows.get(row, "objective"), column("objective"), c.op, v)
            }
        }
        Target::PiiAny => {
            let any = matches!(rows.get(row, "pii_count"), Cell::U64(n) if n > 0);
            let want = matches!(v.to_ascii_lowercase().as_str(), "true" | "yes" | "any");
            any == want
        }
        Target::Param(name) => {
            let Cell::Str(json) = rows.get(row, "parameters_json") else {
                return false;
            };
            let Ok(serde_json::Value::Object(m)) = serde_json::from_str::<serde_json::Value>(&json)
            else {
                return false;
            };
            let Some(q) = m
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|x| x.1)
            else {
                return false;
            };
            let value = q.get("value").unwrap_or(q);
            let cells: Vec<Cell> = match value {
                serde_json::Value::Array(a) => a.iter().map(json_cell).collect(),
                other => vec![json_cell(other)],
            };
            cells.iter().any(|cell| cell_matches(cell, None, c.op, v))
        }
    }
}

fn json_cell(v: &serde_json::Value) -> Cell {
    match v {
        serde_json::Value::Number(n) => n.as_f64().map_or(Cell::Null, Cell::F64),
        serde_json::Value::String(s) => Cell::Str(s.clone()),
        serde_json::Value::Bool(b) => Cell::Bool(*b),
        serde_json::Value::Null => Cell::Null,
        other => Cell::Str(other.to_string()),
    }
}

/// Normalize a term id: `FBbi_00000246` → `FBBI:00000246`.
fn term_norm(s: &str) -> String {
    let s = s.trim().to_ascii_uppercase();
    match s.split_once(['_', ':']) {
        Some((p, local))
            if !p.is_empty()
                && p.chars().all(|c| c.is_ascii_alphabetic())
                && !local.is_empty()
                && local.chars().all(|c| c.is_ascii_alphanumeric()) =>
        {
            format!("{p}:{local}")
        }
        _ => s,
    }
}

/// Parse a number with an optional unit into the column's base unit.
pub fn parse_quantity(v: &str, unit: Option<UnitClass>) -> Option<f64> {
    let v = v.trim();
    let split = v
        .char_indices()
        .find(|(i, c)| {
            !(c.is_ascii_digit()
                || *c == '.'
                || ((*c == '-' || *c == '+') && *i == 0)
                || *c == 'e' && *i > 0 && v[..*i].chars().all(|d| d.is_ascii_digit() || d == '.'))
        })
        .map_or(v.len(), |(i, _)| i);
    let (num, suffix) = v.split_at(split);
    let n: f64 = num.parse().ok()?;
    let s = suffix.trim().to_lowercase();
    if s.is_empty() {
        return Some(n);
    }
    let f = match unit? {
        UnitClass::Bytes => match s.as_str() {
            "b" => 1.0,
            "k" | "kb" => 1e3,
            "m" | "mb" => 1e6,
            "g" | "gb" => 1e9,
            "t" | "tb" => 1e12,
            "p" | "pb" => 1e15,
            "kib" => 1024.0,
            "mib" => 1024f64.powi(2),
            "gib" => 1024f64.powi(3),
            "tib" => 1024f64.powi(4),
            "pib" => 1024f64.powi(5),
            _ => return None,
        },
        UnitClass::Seconds => match s.as_str() {
            "us" | "µs" => 1e-6,
            "ms" => 1e-3,
            "s" | "sec" => 1.0,
            "m" | "min" => 60.0,
            "h" | "hr" | "hours" => 3600.0,
            "d" | "day" | "days" => 86_400.0,
            _ => return None,
        },
        UnitClass::Hertz => match s.as_str() {
            "hz" => 1.0,
            "khz" => 1e3,
            "mhz" => 1e6,
            "ghz" => 1e9,
            _ => return None,
        },
        UnitClass::Micrometres => match s.as_str() {
            "nm" => 1e-3,
            "um" | "µm" | "μm" | "micron" | "microns" => 1.0,
            "mm" => 1e3,
            "a" | "å" => 1e-4,
            _ => return None,
        },
        UnitClass::Magnification => match s.as_str() {
            "x" => 1.0,
            _ => return None,
        },
    };
    Some(n * f)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// A date or partial date as a range `[start, end)` in microseconds: `2020`, `2020-05`,
/// `2020-05-03`, or a full ISO-8601 timestamp (one second).
#[allow(clippy::many_single_char_names)]
pub fn date_range(v: &str) -> Option<(i64, i64)> {
    const DAY: i64 = 86_400_000_000;
    let v = v.trim();
    if v.len() >= 19 {
        let t = micros_from_iso(v)?;
        return Some((t, t + 1_000_000));
    }
    let parts: Vec<&str> = v.split('-').collect();
    let num = |s: &str| s.parse::<i64>().ok();
    match parts.as_slice() {
        [y] if y.len() == 4 => {
            let y = num(y)?;
            Some((
                days_from_civil(y, 1, 1) * DAY,
                days_from_civil(y + 1, 1, 1) * DAY,
            ))
        }
        [y, m] if y.len() == 4 => {
            let (y, m) = (num(y)?, num(m)?);
            if !(1..=12).contains(&m) {
                return None;
            }
            let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
            Some((
                days_from_civil(y, m, 1) * DAY,
                days_from_civil(ny, nm, 1) * DAY,
            ))
        }
        [y, m, d] if y.len() == 4 => {
            let (y, m, d) = (num(y)?, num(m)?, num(d.get(..2).unwrap_or(d))?);
            if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
                return None;
            }
            let s = days_from_civil(y, m, d) * DAY;
            Some((s, s + DAY))
        }
        _ => None,
    }
}

fn cmp_num(x: f64, op: Op, y: f64) -> bool {
    let eq = (x - y).abs() <= 1e-9 * x.abs().max(y.abs()).max(1.0);
    match op {
        Op::Eq | Op::Has => eq,
        Op::Lt => x < y && !eq,
        Op::Le => x < y || eq,
        Op::Gt => x > y && !eq,
        Op::Ge => x > y || eq,
        Op::Contains => false,
    }
}

fn cmp_str(cell: &str, op: Op, v: &str, term_column: bool) -> bool {
    let a = cell.to_lowercase();
    let b = v.to_lowercase();
    match op {
        Op::Eq => a == b || (term_column && term_norm(cell) == term_norm(v)),
        Op::Contains => a.contains(&b),
        Op::Has => {
            if term_column && term_norm(cell) == term_norm(v) {
                return true;
            }
            a.contains(&b)
        }
        Op::Lt | Op::Le | Op::Gt | Op::Ge => {
            if let (Ok(x), Ok(y)) = (cell.trim().parse::<f64>(), v.trim().parse::<f64>()) {
                return cmp_num(x, op, y);
            }
            match op {
                Op::Lt => a < b,
                Op::Le => a <= b,
                Op::Gt => a > b,
                _ => a >= b,
            }
        }
    }
}

fn cell_matches(cell: &Cell, def: Option<&ColumnDef>, op: Op, v: &str) -> bool {
    let unit = def.and_then(|d| d.unit);
    let term_column = def.is_some_and(|d| d.name.ends_with("_id") || d.name == "terms");
    match cell {
        Cell::Null => false,
        Cell::Str(s) => cmp_str(s, op, v, term_column),
        Cell::List(l) => l.iter().any(|s| cmp_str(s, op, v, term_column)),
        Cell::Bool(b) => {
            let want = matches!(v.to_ascii_lowercase().as_str(), "true" | "yes" | "1");
            matches!(op, Op::Eq | Op::Has) && *b == want
        }
        Cell::U64(_) | Cell::I64(_) | Cell::F64(_) => {
            let x = match cell {
                Cell::U64(n) => *n as f64,
                Cell::I64(n) => *n as f64,
                Cell::F64(n) => *n,
                _ => unreachable!("numeric cell"),
            };
            if op == Op::Contains {
                return cell.to_text().contains(v);
            }
            parse_quantity(v, unit).is_some_and(|y| cmp_num(x, op, y))
        }
        Cell::IntList(l) => {
            if op == Op::Contains {
                return l.iter().any(|x| x.to_string().contains(v));
            }
            parse_quantity(v, unit).is_some_and(|y| l.iter().any(|&x| cmp_num(x as f64, op, y)))
        }
        Cell::Time(t) => {
            if op == Op::Contains {
                return cell.to_text().contains(v);
            }
            let Some((s, e)) = date_range(v) else {
                return false;
            };
            match op {
                Op::Eq | Op::Has => *t >= s && *t < e,
                Op::Lt => *t < s,
                Op::Le => *t < e,
                Op::Gt => *t >= e,
                Op::Ge => *t >= s,
                Op::Contains => false,
            }
        }
    }
}

/// Resolve a sort field (`-size` for descending) to a column and direction.
pub fn sort_key(field: &str) -> Result<(&'static str, bool)> {
    let (desc, name) = match field.strip_prefix('-') {
        Some(n) => (true, n),
        None => (false, field),
    };
    let lname = name.to_ascii_lowercase();
    let col = match lname.as_str() {
        "objective" => Some("objective_magnification"),
        _ => resolve_field(&lname).and_then(|c| c.first().copied()),
    };
    col.map(|c| (c, desc))
        .ok_or_else(|| Error::Usage(format!("unknown sort field `{name}`")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_and_dates() {
        assert_eq!(parse_quantity("1GB", Some(UnitClass::Bytes)), Some(1e9));
        assert_eq!(
            parse_quantity("2 KiB", Some(UnitClass::Bytes)),
            Some(2048.0)
        );
        assert_eq!(
            parse_quantity("20kHz", Some(UnitClass::Hertz)),
            Some(20_000.0)
        );
        assert_eq!(
            parse_quantity("150nm", Some(UnitClass::Micrometres)),
            Some(0.15)
        );
        assert_eq!(
            parse_quantity("63x", Some(UnitClass::Magnification)),
            Some(63.0)
        );
        assert_eq!(
            parse_quantity("10min", Some(UnitClass::Seconds)),
            Some(600.0)
        );
        assert_eq!(parse_quantity("1e3", None), Some(1000.0));
        assert_eq!(parse_quantity("abc", None), None);
        let (s, e) = date_range("2020").unwrap();
        assert_eq!(
            crate::tables::iso_from_micros(s),
            "2020-01-01T00:00:00.000Z"
        );
        assert_eq!(
            crate::tables::iso_from_micros(e),
            "2021-01-01T00:00:00.000Z"
        );
        let (s, e) = date_range("2020-12").unwrap();
        assert_eq!(
            crate::tables::iso_from_micros(s),
            "2020-12-01T00:00:00.000Z"
        );
        assert_eq!(
            crate::tables::iso_from_micros(e),
            "2021-01-01T00:00:00.000Z"
        );
        assert!(date_range("2020-13").is_none());
        assert_eq!(term_norm("FBbi_00000246"), "FBBI:00000246");
        assert_eq!(term_norm("fbbi:00000246"), "FBBI:00000246");
    }

    #[test]
    fn parse_errors_are_usage_errors() {
        assert!(Query::parse("objective=63x channel~GFP acquired<2020").is_ok());
        assert!(Query::parse("technique:FBbi_00000246 format=czi size>1GB sample~A12").is_ok());
        assert!(
            Query::parse("colour=red")
                .unwrap_err()
                .to_string()
                .contains("unknown query field")
        );
        assert!(Query::parse("format=").is_err());
        assert!(Query::parse("sample~\"a b").is_err());
        assert!(Query::parse("OR format=czi").is_err());
        let q = Query::parse("format=czi|nd2 -status=ok GFP").unwrap();
        let cols = q.columns();
        assert!(
            cols.contains(&"format") && cols.contains(&"check_status") && cols.contains(&"path")
        );
    }
}
