//! Delimited text exports read as one table, against Python's `csv` module
//! (`oracle/export_tables.py`): the row count, and for every titled column the cells of up to 96
//! rows. A column is matched by its label (the vendor's header). A number must equal ours; text
//! must equal our category, or, where we read a number column, the text we keep beside it in a
//! `<name>_note` column (`No Call`); an empty cell must be NaN or empty text in ours.
#![allow(dead_code)] // each test binary that includes this module uses a part of it

use std::path::Path;

use openreadout_core::FormatReader;
use serde_json::Value;

mod qlp;

/// Format ids this module compares (`biorad-qlp` in `qlp.rs`).
pub const FORMATS: &[&str] = &["biorad-ddpcr", "biorad-qlp"];

fn reader(format: &str) -> Option<Box<dyn FormatReader>> {
    match format {
        "biorad-ddpcr" => Some(Box::new(openreadout_benchtop::DdpcrReader)),
        _ => None,
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

pub fn compare_file(format: &str, _id: &str, path: &Path, oracle: &Value) -> Result<String, String> {
    if format == "biorad-qlp" {
        return qlp::compare(path, oracle);
    }
    let r = reader(format).ok_or_else(|| format!("no reader for {format}"))?;
    let mut ds = r.open(path).map_err(|e| format!("open failed: {e}"))?;
    let info = ds.info().map_err(|e| format!("info failed: {e}"))?;
    let t = info.tables.first().ok_or("no table")?;
    let data = ds
        .read_table(0, 0, u64::MAX)
        .map_err(|e| format!("read failed: {e}"))?;
    let mut problems = Vec::new();
    if t.row_count != oracle["rows"].as_u64().unwrap_or(u64::MAX) {
        problems.push(format!("{} rows, csv {}", t.row_count, oracle["rows"]));
    }
    let categories = |k: usize| -> Option<Vec<String>> {
        t.columns[k]
            .extra
            .get("categories")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|v| v.as_str().unwrap_or_default().to_string())
                    .collect()
            })
    };
    let text_at = |k: usize, i: usize| -> Option<String> {
        let cats = categories(k)?;
        let code = *data.columns[k].get(i)?;
        cats.get(code as usize).cloned()
    };
    let mut checked = 0usize;
    let mut columns = 0usize;
    let empty = serde_json::Map::new();
    for (title, cells) in oracle["cells"].as_object().unwrap_or(&empty) {
        let Some(k) = t
            .columns
            .iter()
            .position(|c| c.label.as_deref().map(str::trim) == Some(title.trim()))
        else {
            // a column with no value in any sampled row may be left out
            if cells
                .as_array()
                .is_some_and(|a| a.iter().all(|c| c[1].is_null()))
            {
                continue;
            }
            problems.push(format!("no column titled {title:?}"));
            continue;
        };
        columns += 1;
        let note = t
            .columns
            .iter()
            .position(|c| c.name == format!("{}_note", t.columns[k].name));
        let numeric = categories(k).is_none();
        for c in cells.as_array().into_iter().flatten() {
            let i = c[0].as_u64().unwrap_or(0) as usize;
            let ok = match (&c[1], numeric) {
                (Value::Null, true) => data.columns[k].get(i).is_some_and(|v| v.is_nan()),
                (Value::Null, false) => text_at(k, i).is_some_and(|s| s.trim().is_empty()),
                (Value::Number(n), true) => {
                    let v = n.as_f64().unwrap_or(f64::NAN);
                    data.columns[k].get(i).is_some_and(|o| close(*o, v))
                }
                (Value::Number(n), false) => text_at(k, i).is_some_and(|s| {
                    s.trim().parse::<f64>().ok() == n.as_f64()
                        || s.trim() == n.to_string()
                }),
                (Value::String(s), true) => {
                    data.columns[k].get(i).is_some_and(|v| v.is_nan())
                        && note.is_none_or(|nk| text_at(nk, i).as_deref() == Some(s.trim()))
                }
                (Value::String(s), false) => text_at(k, i).as_deref() == Some(s.trim()),
                _ => false,
            };
            if !ok {
                let ours = if numeric {
                    data.columns[k].get(i).map(f64::to_string)
                } else {
                    text_at(k, i)
                };
                problems.push(format!("{title} row {i}: ours {ours:?}, csv {}", c[1]));
                break;
            }
            checked += 1;
        }
    }
    if columns == 0 {
        problems.push("no column compared".into());
    }
    if problems.is_empty() {
        Ok(format!(
            "{} rows; {checked} cells in {columns} columns equal Python's csv reading",
            t.row_count
        ))
    } else {
        Err(problems.join("; "))
    }
}
