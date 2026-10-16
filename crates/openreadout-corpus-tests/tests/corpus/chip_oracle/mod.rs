//! Agilent Bioanalyzer and TapeStation exports against an independent pass in Python
//! (`oracle/chip_oracle.py`): each well's signal values at their own positions (migration time or
//! run distance), RIN/RINe/DIN, and the instrument's size of every peak against this reader's size
//! axis at the peak's position (Bioanalyzer within 0.05 bp, TapeStation within 3 bp or 3 %).
#![allow(dead_code)] // each test binary that includes this module uses a part of it

use std::path::Path;

use openreadout_core::Registry;
use serde_json::Value;

/// Format ids this module compares.
pub const FORMATS: &[&str] = &["agilent-bioanalyzer-xml", "agilent-tapestation"];

/// Linear interpolation of `y` over `x` (any order) at `at`.
#[allow(clippy::many_single_char_names)]
fn interp(x: &[f64], y: &[f64], at: f64) -> Option<f64> {
    let mut p: Vec<(f64, f64)> = x.iter().copied().zip(y.iter().copied()).collect();
    p.sort_by(|a, b| a.0.total_cmp(&b.0));
    let k = p.partition_point(|q| q.0 <= at);
    if k == 0 || k >= p.len() {
        return None;
    }
    let (a, b) = (p[k - 1], p[k]);
    Some(a.1 + (at - a.0) / (b.0 - a.0) * (b.1 - a.1))
}

pub fn compare_file(format: &str, path: &Path, oracle: &Value) -> Result<String, String> {
    let reg = Registry::new()
        .with(Box::new(openreadout_ce::BioanalyzerReader))
        .with(Box::new(openreadout_ce::TapeStationReader));
    let (_, mut ds) = reg.open(path).map_err(|e| format!("open failed: {e}"))?;
    let info = ds.info().map_err(|e| format!("info failed: {e}"))?;
    if info.format.id != format {
        return Err(format!("read as {}", info.format.id));
    }
    let mut problems = Vec::new();
    let (mut values, mut peaks, mut wells) = (0usize, 0usize, 0usize);
    let bp_tight = format == "agilent-bioanalyzer-xml";
    for w in oracle["wells"].as_array().into_iter().flatten() {
        let well = w["well"].as_str().unwrap_or("");
        let Some(t) = info
            .traces
            .iter()
            .find(|t| t.extra.get("well").and_then(Value::as_str) == Some(well))
        else {
            if w["signal"].is_object() && !w["peaks"].as_array().is_some_and(Vec::is_empty) {
                problems.push(format!("well {well}: no trace"));
            }
            continue;
        };
        wells += 1;
        for k in ["rin", "rine", "din"] {
            if let Some(v) = w[k].as_f64() {
                let ours = t
                    .extra
                    .get(if k == "din" { "din" } else { "rin" })
                    .and_then(Value::as_f64);
                if ours.is_none_or(|o| (o - v).abs() > 1e-6) {
                    problems.push(format!("well {well}: {k} {ours:?} vs {v}"));
                }
            }
        }
        let tr = ds
            .read_trace(t.index, 0, 0, u64::MAX)
            .map_err(|e| format!("read: {e}"))?;
        let (size, sig, pos) = (&tr.channels[0], &tr.channels[1], &tr.channels[2]);
        // signal values at their own positions, where the trace covers them
        for p in w["signal"]["at"].as_array().into_iter().flatten() {
            let (x, v) = (p[0].as_f64().unwrap_or(f64::NAN), p[1].as_f64());
            if let Some(i) = pos.iter().position(|q| (q - x).abs() < 1e-6) {
                let ok = v.is_some_and(|v| (sig[i] - v).abs() <= 1e-6 * v.abs().max(1.0));
                if !ok {
                    problems.push(format!("well {well} at {x}: {} vs {v:?}", sig[i]));
                    break;
                }
                values += 1;
            }
        }
        for p in w["peaks"].as_array().into_iter().flatten() {
            let (x, want) = (
                p[0].as_f64().unwrap_or(f64::NAN),
                p[1].as_f64().unwrap_or(f64::NAN),
            );
            let Some(ours) = interp(pos, size, x) else {
                continue;
            };
            let tol = if bp_tight {
                0.05 + 1e-4 * want
            } else {
                3.0f64.max(0.03 * want)
            };
            if (ours - want).abs() > tol {
                problems.push(format!(
                    "well {well}: peak at {x} sized {ours:.2}, instrument {want}"
                ));
                break;
            }
            peaks += 1;
        }
    }
    if problems.is_empty() {
        Ok(format!(
            "{wells} wells, {values} values, {peaks} peak sizes"
        ))
    } else {
        Err(problems.join("; "))
    }
}
