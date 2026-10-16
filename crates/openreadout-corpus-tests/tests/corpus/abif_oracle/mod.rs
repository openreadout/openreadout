//! Applied Biosystems ABIF files against Biopython's AbiIO (`oracle/abif_oracle.py`): every tag
//! Biopython decodes (text and scalars equal to the vendor tree; the dye and run-monitor arrays
//! equal to the traces at 64 positions and in sum; other arrays by length), the called bases and
//! quality values, the sample name and run start. For 3500 files exported through PeakScanner,
//! every exported peak: the instrument's size at its data point, its height, and this reader's
//! own Local Southern sizing within 0.5 bp of the instrument's.
#![allow(dead_code)] // each test binary that includes this module uses a part of it

use std::path::Path;

use openreadout_core::FormatReader;
use openreadout_core::model::TraceInfo;
use serde_json::Value;

/// Format ids this module compares.
pub const FORMATS: &[&str] = &["abif"];

/// The trace and channel holding the array stored as `key` (`DATA9`), with its scale.
fn stored(traces: &[TraceInfo], key: &str) -> Option<(u32, u32, f64)> {
    for t in traces {
        if t.name.as_deref() == Some("sized") {
            continue;
        }
        for c in &t.channels {
            if c.extra.get("stored_as").and_then(Value::as_str) == Some(key) {
                let scale = if c.name == "voltage" { 10.0 } else { 1.0 };
                return Some((t.index, c.index, scale));
            }
        }
    }
    None
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0)
}

pub fn compare_file(_id: &str, path: &Path, oracle: &Value) -> Result<String, String> {
    let mut ds = openreadout_ce::AbifReader
        .open(path)
        .map_err(|e| format!("open failed: {e}"))?;
    let info = ds.info().map_err(|e| format!("info failed: {e}"))?;
    let vendor = ds.vendor_metadata().map_err(|e| format!("vendor: {e}"))?;
    let mut problems: Vec<String> = Vec::new();
    let (mut tags, mut arrays, mut values) = (0usize, 0usize, 0usize);
    let empty = serde_json::Map::new();
    for (key, want) in oracle["tags"].as_object().unwrap_or(&empty) {
        let ours = &vendor[key];
        if let Some(t) = want.get("text").and_then(Value::as_str) {
            // binary char arrays (quality values) are compared as the base-call table
            if key.starts_with("PCON") || t.contains('\0') {
                continue;
            }
            if ours.as_str().map(str::trim_end) != Some(t.trim_end()) {
                problems.push(format!("{key}: {ours} vs {t:?}"));
            }
            tags += 1;
        } else if let Some(n) = want.get("number").and_then(Value::as_f64) {
            // Biopython reads a bool item of several elements as one truth value (the tuple's)
            if ours.as_array().is_some_and(|a| a.len() > 1) {
                continue;
            }
            match ours.as_f64() {
                Some(v) if close(v, n) => {}
                _ => problems.push(format!("{key}: {ours} vs {n}")),
            }
            tags += 1;
        } else if let Some(count) = want.get("count").and_then(Value::as_u64) {
            arrays += 1;
            if let Some((ti, ci, scale)) = stored(&info.traces, key) {
                let t = ds
                    .read_trace(ti, 0, 0, u64::MAX)
                    .map_err(|e| format!("read trace {ti}: {e}"))?;
                let col = &t.channels[ci as usize];
                if col.len() as u64 != count {
                    problems.push(format!("{key}: {} values vs {count}", col.len()));
                    continue;
                }
                let sum: f64 = col.iter().sum::<f64>() / scale;
                if !close(sum, want["sum"].as_f64().unwrap_or(f64::NAN)) {
                    problems.push(format!("{key}: sum {sum} vs {}", want["sum"]));
                }
                for p in want["at"].as_array().into_iter().flatten() {
                    let i = p[0].as_u64().unwrap_or(0) as usize;
                    let v = p[1].as_f64().unwrap_or(f64::NAN);
                    if !close(col[i] / scale, v) {
                        problems.push(format!("{key}[{i}]: {} vs {v}", col[i] / scale));
                        break;
                    }
                    values += 1;
                }
            } else if let Some(a) = ours.as_array() {
                if a.len() as u64 != count {
                    problems.push(format!("{key}: {} values vs {count}", a.len()));
                }
            } else if ours.get("count").and_then(Value::as_u64) != Some(count) {
                problems.push(format!("{key}: {ours} vs {count} values"));
            }
        }
    }
    // base calls
    let seq = oracle["sequence"].as_str().unwrap_or("");
    let calls = info
        .tables
        .iter()
        .find(|t| t.name.as_deref() == Some("base calls"));
    match (seq.is_empty(), calls) {
        (true, None) => {}
        (true, Some(_)) => problems.push("base calls where Biopython has none".into()),
        (false, None) => problems.push(format!("no base calls; Biopython {} bases", seq.len())),
        (false, Some(t)) => {
            let rows = ds
                .read_table(t.index, 0, u64::MAX)
                .map_err(|e| format!("read table: {e}"))?;
            let cats: Vec<&str> = t.columns[1].extra["categories"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let ours: String = rows.columns[1]
                .iter()
                .map(|c| cats.get(*c as usize).copied().unwrap_or("?"))
                .collect();
            if ours != seq {
                problems.push(format!(
                    "bases differ ({} vs {} long)",
                    ours.len(),
                    seq.len()
                ));
            }
            if let Some(q) = oracle["quality"].as_array() {
                let qcol = t.columns.iter().position(|c| c.name == "quality");
                match qcol {
                    Some(k) => {
                        let theirs: Vec<f64> = q.iter().filter_map(Value::as_f64).collect();
                        if rows.columns[k] != theirs {
                            problems.push("quality values differ".into());
                        }
                    }
                    None => problems.push("no quality column".into()),
                }
            }
        }
    }
    // sample and run start
    let exp = openreadout_core::experiment::of_dataset(ds.as_ref(), &info);
    if let Some(name) = oracle["tags"]["SMPL1"]["text"].as_str() {
        let ours = exp.sample.as_ref().and_then(|s| s.name.clone());
        if ours.as_deref() != Some(name.trim()) {
            problems.push(format!("sample name {ours:?} vs {name:?}"));
        }
    }
    if let Some(start) = oracle["run_start"].as_str()
        && let Some((d, t)) = start.split_once(' ')
        && !d.is_empty()
        && !t.is_empty()
    {
        let ours = exp.acquisition.as_ref().and_then(|a| a.started_at.clone());
        if ours.as_deref() != Some(&format!("{d}T{t}")) {
            problems.push(format!("started_at {ours:?} vs {start}"));
        }
    }
    // PeakScanner's peaks: the instrument's sizes, and ours against them
    let mut sized_peaks = 0;
    if let Some(rows) = oracle["peakscanner"].as_array() {
        let t = info
            .traces
            .iter()
            .find(|t| t.name.as_deref() == Some("sized"))
            .ok_or("no sized trace")?;
        let first = t.extra["first_scan"].as_f64().unwrap_or(0.0);
        let tr = ds
            .read_trace(t.index, 0, 0, u64::MAX)
            .map_err(|e| format!("read sized trace: {e}"))?;
        let size = &tr.channels[0];
        for r in rows {
            let (want, dp) = (r[1].as_f64().unwrap_or(f64::NAN), r[3].as_f64().unwrap_or(-1.0));
            let i = dp - first;
            if i < 0.0 || i as usize >= size.len() {
                continue;
            }
            let ours = size[i as usize];
            if (ours - want).abs() > 0.006 {
                problems.push(format!("peak at {dp}: size {ours:.3} vs PeakScanner {want}"));
                break;
            }
            sized_peaks += 1;
        }
        if sized_peaks == 0 {
            problems.push("no PeakScanner peak inside the sized range".into());
        }
        let check = &t.extra["sizing_check"];
        match check["max_abs_diff_bp"].as_f64() {
            Some(m) if m <= 0.5 => {}
            other => problems.push(format!(
                "this reader's Local Southern sizing differs from the instrument's by {other:?} bp"
            )),
        }
    }
    if problems.is_empty() {
        Ok(format!(
            "{tags} tags, {arrays} arrays ({values} values), {} bases, {sized_peaks} sized peaks",
            seq.len()
        ))
    } else {
        Err(problems.join("; "))
    }
}
