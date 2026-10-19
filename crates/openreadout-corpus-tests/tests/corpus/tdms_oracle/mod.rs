//! National Instruments TDMS files against npTDMS (`oracle/tdms_oracle.py`, run as a black box):
//! every channel's value count, first values, 64 sampled values, the last value and the sum of
//! the finite values, as npTDMS scales them; string channels by their count.
#![allow(dead_code)] // each test binary that includes this module uses a part of it
#![allow(clippy::float_cmp)] // infinities compare exactly

use std::path::Path;

use openreadout_core::FormatReader;
use serde_json::Value;

/// Format ids this module compares.
pub const FORMATS: &[&str] = &["ni-tdms"];

/// An oracle value: a number, or `nan` / `inf` / `-inf` as text.
fn oval(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => match s.as_str() {
            "nan" => Some(f64::NAN),
            "inf" => Some(f64::INFINITY),
            "-inf" => Some(f64::NEG_INFINITY),
            _ => None,
        },
        _ => None,
    }
}

fn same(a: f64, b: f64, scale: f64, timestamp: bool) -> bool {
    if a.is_nan() || b.is_nan() {
        return a.is_nan() && b.is_nan();
    }
    if a.is_infinite() || b.is_infinite() {
        return a == b;
    }
    let tol = if timestamp {
        1e-6
    } else {
        1e-12 * scale.max(a.abs()).max(b.abs()).max(1e-300)
    };
    (a - b).abs() <= tol
}

/// Compare one corpus file with its oracle.
pub fn compare_file(_id: &str, path: &Path, o: &Value) -> Result<String, String> {
    let mut ds = openreadout_tdms::TdmsReader
        .open(path)
        .map_err(|e| format!("open failed: {e}"))?;
    let info = ds.info().map_err(|e| e.to_string())?;
    let mut problems = Vec::new();
    let mut compared = 0usize;
    let mut values = 0usize;
    let vendor = ds.vendor_metadata().map_err(|e| e.to_string())?;
    let empty = Vec::new();
    let oracle_numeric = o["channels"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter(|c| c["string"].as_bool() != Some(true))
        .count();
    let ours_numeric: usize = info.traces.iter().map(|t| t.channels.len()).sum();
    if ours_numeric != oracle_numeric {
        problems.push(format!(
            "{ours_numeric} numeric channels, npTDMS {oracle_numeric}"
        ));
    }
    for c in o["channels"].as_array().unwrap_or(&empty) {
        let group = c["group"].as_str().unwrap_or("");
        let name = c["channel"].as_str().unwrap_or("");
        let count = c["count"].as_u64().unwrap_or(0);
        if c["string"].as_bool() == Some(true) {
            let path = format!(
                "/'{}'/'{}'",
                group.replace('\'', "''"),
                name.replace('\'', "''")
            );
            let ours = &vendor["objects"][&path];
            let first: Vec<&str> = ours["first_values"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let want: Vec<&str> = c["first_strings"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            if ours["value_count"].as_u64() != Some(count) {
                problems.push(format!(
                    "string channel {path}: {:?} values, npTDMS {count}",
                    ours["value_count"]
                ));
            } else if first.iter().zip(&want).any(|(a, b)| a != b) {
                problems.push(format!(
                    "string channel {path}: first strings {first:?}, npTDMS {want:?}"
                ));
            } else {
                compared += 1;
            }
            continue;
        }
        let found = info.traces.iter().find_map(|t| {
            let g = t.extra.get("group").and_then(Value::as_str);
            (g == Some(group))
                .then(|| {
                    t.channels
                        .iter()
                        .position(|ch| ch.name == name)
                        .map(|k| (t, k))
                })
                .flatten()
        });
        let Some((t, k)) = found else {
            problems.push(format!("channel {group}/{name}: not found"));
            continue;
        };
        let timestamp = c["dtype"].as_str() == Some("TimeStamp");
        if t.sample_count != count {
            problems.push(format!(
                "channel {group}/{name}: {} values, npTDMS {count}",
                t.sample_count
            ));
            continue;
        }
        let unit = c["unit"].as_str().filter(|u| !u.trim().is_empty());
        if !timestamp && unit.is_some() && t.channels[k].unit.as_deref() != unit {
            problems.push(format!(
                "channel {group}/{name}: unit {:?}, npTDMS {unit:?}",
                t.channels[k].unit
            ));
        }
        if let Some(inc) = c["wf_increment"].as_f64()
            && inc > 0.0
            && (t.sample_rate_hz * inc - 1.0).abs() > 1e-9
        {
            problems.push(format!(
                "channel {group}/{name}: {} Hz, npTDMS increment {inc}",
                t.sample_rate_hz
            ));
        }
        let scale = c["sum_abs"].as_f64().unwrap_or(0.0) / count.max(1) as f64;
        let mut check = |i: u64, want: f64, what: &str, problems: &mut Vec<String>| {
            let got = ds
                .read_trace(t.index, 0, i, 1)
                .map_err(|e| e.to_string())
                .and_then(|tr| {
                    tr.channels
                        .get(k)
                        .and_then(|v| v.first())
                        .copied()
                        .ok_or_else(|| "no value".into())
                });
            match got {
                Ok(g) if same(g, want, scale, timestamp) => {}
                Ok(g) => problems.push(format!(
                    "channel {group}/{name} {what} {i}: {g}, npTDMS {want}"
                )),
                Err(e) => problems.push(format!("channel {group}/{name} {what} {i}: {e}")),
            }
        };
        let before = problems.len();
        for (i, v) in c["first"].as_array().unwrap_or(&empty).iter().enumerate() {
            if let Some(w) = oval(v) {
                check(i as u64, w, "value", &mut problems);
                values += 1;
            }
        }
        for s in c["sampled"].as_array().unwrap_or(&empty) {
            if let (Some(i), Some(w)) = (s[0].as_u64(), oval(&s[1])) {
                check(i, w, "value", &mut problems);
                values += 1;
            }
        }
        if let Some(w) = c["sum"].as_f64() {
            // the sum of the finite values, read in blocks
            let mut sum = 0.0;
            let mut first = 0u64;
            while first < count {
                let tr = ds
                    .read_trace(t.index, 0, first, 1 << 20)
                    .map_err(|e| e.to_string())?;
                let v = &tr.channels[k];
                sum += v.iter().filter(|x| x.is_finite()).sum::<f64>();
                first += v.len().max(1) as u64;
            }
            let tol = 1e-9 * c["sum_abs"].as_f64().unwrap_or(w.abs()).max(1e-300)
                + if timestamp { 1e-6 * count as f64 } else { 0.0 };
            if (sum - w).abs() > tol {
                problems.push(format!("channel {group}/{name}: sum {sum}, npTDMS {w}"));
            }
        }
        if problems.len() == before {
            compared += 1;
        }
    }
    if problems.is_empty() {
        Ok(format!(
            "{compared} channels ({values} values, counts and sums) agree with npTDMS"
        ))
    } else {
        problems.truncate(12);
        Err(problems.join("; "))
    }
}
