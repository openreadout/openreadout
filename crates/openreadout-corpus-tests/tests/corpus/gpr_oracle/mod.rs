//! GenePix Results `.gpr` files against pandas (`oracle/gpr_oracle.py`): row and column counts,
//! titles, every numeric column at 64 rows (`Error` as NaN), the `Name` column, and the
//! `Creator`/`Scanner` records in the experiment model.
#![allow(dead_code)] // each test binary that includes this module uses a part of it

use std::path::Path;

use openreadout_core::FormatReader;
use serde_json::Value;

/// Format ids this module compares.
pub const FORMATS: &[&str] = &["genepix-gpr"];

pub fn compare_file(_id: &str, path: &Path, oracle: &Value) -> Result<String, String> {
    let mut ds = openreadout_biophys::GprReader
        .open(path)
        .map_err(|e| format!("open failed: {e}"))?;
    let info = ds.info().map_err(|e| format!("info failed: {e}"))?;
    let t = info.tables.first().ok_or("no feature table")?;
    let mut problems = Vec::new();
    let empty = Vec::new();
    if t.row_count != oracle["rows"].as_u64().unwrap_or(0) {
        problems.push(format!("{} rows, pandas {}", t.row_count, oracle["rows"]));
    }
    let titles: Vec<&str> = t.columns.iter().map(|c| c.name.as_str()).collect();
    let theirs: Vec<&str> = oracle["columns"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter_map(Value::as_str)
        .collect();
    if titles != theirs {
        problems.push("column titles differ".into());
    }
    let table = ds
        .read_table(0, 0, u64::MAX)
        .map_err(|e| format!("read failed: {e}"))?;
    let mut checked = 0;
    for (name, samples) in oracle["numeric"].as_object().into_iter().flatten() {
        let Some(k) = titles.iter().position(|t| t == name) else {
            continue;
        };
        if t.columns[k].dtype != "float64" {
            problems.push(format!("column {name} is not numeric"));
            continue;
        }
        for s in samples.as_array().unwrap_or(&empty) {
            let i = s[0].as_u64().unwrap_or(0) as usize;
            let ours = table.columns[k].get(i).copied().unwrap_or(f64::NAN);
            let ok = match s[1].as_f64() {
                Some(v) => (ours - v).abs() <= 1e-9 * v.abs().max(1.0),
                None => ours.is_nan(),
            };
            if !ok {
                problems.push(format!("{name} row {i}: {ours} vs {}", s[1]));
                break;
            }
            checked += 1;
        }
    }
    if let Some(k) = titles.iter().position(|t| *t == "Name") {
        let labels: Vec<String> = t.columns[k].extra["categories"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        for s in oracle["names"].as_array().unwrap_or(&empty) {
            let i = s[0].as_u64().unwrap_or(0) as usize;
            let ours = table.columns[k]
                .get(i)
                .and_then(|c| labels.get(*c as usize))
                .map(|x| x.trim().to_string());
            if ours.as_deref() != s[1].as_str().map(str::trim) {
                problems.push(format!("Name row {i}: {ours:?} vs {}", s[1]));
                break;
            }
        }
    }
    let exp = ds.experiment().ok_or("no experiment facts")?;
    let ins = exp.instrument.unwrap_or_default();
    if let Some(c) = oracle["creator"].as_str() {
        let ours = format!(
            "{} {}",
            ins.software.clone().unwrap_or_default(),
            ins.software_version.clone().unwrap_or_default()
        );
        if ours.trim() != c.trim() {
            problems.push(format!("software {ours:?}, Creator {c:?}"));
        }
    }
    if let Some(s) = oracle["scanner"].as_str()
        && !s.starts_with(ins.model.as_deref().unwrap_or("?"))
    {
        problems.push(format!("model {:?}, Scanner {s:?}", ins.model));
    }
    if problems.is_empty() {
        Ok(format!(
            "{} rows, {} columns, {checked} numeric cells and the names equal pandas'; Creator and Scanner match",
            t.row_count,
            titles.len()
        ))
    } else {
        Err(problems.join("; "))
    }
}
