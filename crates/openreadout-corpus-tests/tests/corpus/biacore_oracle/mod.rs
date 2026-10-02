//! Cytiva Biacore `.blr` files against their ground truth (`oracle/biacore_oracle.py`), for every
//! entry through one code path (`tests/corpus/`):
//!
//! - the control software's report points (`RPoint Table`): the reader's `report_points` table
//!   carries the same `AbsResp` values, and each is recomputed as the mean of the reader's
//!   sensorgram over the closed window [Time - Window/2, Time + Window/2]: to 1e-6 RU for curves
//!   stored at 10 Hz; at 1 Hz (the software averaged data the file does not keep) within the
//!   window's spread;
//! - allotropy (MIT) as a black box: the same curves per cycle and flow cell, the same lengths,
//!   and the same times and responses at the oracle's samples;
//! - the `Environment`/`Chip` facts: instrument id, software and version, model, user, run type,
//!   cycle and flow-cell counts, chip id;
//! - evaluation files (`.bme`): every fit of the evaluation items as the oracle parses the item XML
//!   (sample, curves, model, Chi², each parameter's first value and standard error, the number of
//!   curves), in the same order.
#![allow(dead_code)] // each test binary that includes this module uses a part of it
#![allow(
    clippy::many_single_char_names,
    clippy::float_cmp,
    clippy::if_not_else,
    clippy::semicolon_if_nothing_returned
)] // comparison code: short names for x, y, t; exact equality where values are copied

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::FormatReader;
use serde_json::Value;

/// Format ids this module compares.
pub const FORMATS: &[&str] = &["cytiva-biacore-blr", "cytiva-biacore-bme"];

/// Window edges: report times are float32 (99.9000015 s), so a sample on the edge (97.4 s) is
/// inside the software's closed window; well below half a sample (0.05 s at 10 Hz).
const EDGE: f64 = 1e-4;

struct Curve {
    index: u32,
    rate: f64,
    start: f64,
    n: u64,
    irregular: bool,
}

pub fn compare_file(_id: &str, path: &Path, oracle: &Value) -> Result<String, String> {
    let mut ds = if oracle["format"].as_str() == Some("cytiva-biacore-bme") {
        openreadout_biophys::BiacoreEvaluationReader.open(path)
    } else {
        openreadout_biophys::BiacoreReader.open(path)
    }
    .map_err(|e| format!("open failed: {e}"))?;
    let info = ds.info().map_err(|e| format!("info failed: {e}"))?;
    let mut problems = Vec::new();
    let mut done = Vec::new();
    let empty = Vec::new();

    // curves by (cycle, flow cell)
    let mut curves: BTreeMap<(u64, String), Curve> = BTreeMap::new();
    for t in &info.traces {
        let (Some(c), Some(fc)) = (
            t.extra.get("cycle").and_then(Value::as_u64),
            t.extra.get("flow_cell").and_then(Value::as_str),
        ) else {
            problems.push(format!("trace {} has no cycle / flow cell", t.index));
            continue;
        };
        if curves
            .insert(
                (c, fc.to_string()),
                Curve {
                    index: t.index,
                    rate: t.sample_rate_hz,
                    start: t.start_s.unwrap_or(0.0),
                    n: t.sample_count,
                    irregular: t.channels.len() > 1,
                },
            )
            .is_some()
        {
            problems.push(format!("two curves for cycle {c} flow cell {fc}"));
        }
    }
    // the table carries the same AbsResp values
    if let Some(ti) = info
        .tables
        .iter()
        .find(|t| t.name.as_deref() == Some("report_points"))
    {
        let col = ti.columns.iter().position(|c| c.name == "AbsResp");
        let table = ds
            .read_table(ti.index, 0, u64::MAX)
            .map_err(|e| format!("table failed: {e}"))?;
        match col {
            Some(k) => {
                let ours = &table.columns[k];
                let theirs: Vec<f64> = oracle["report_points"]
                    .as_array()
                    .unwrap_or(&empty)
                    .iter()
                    .filter_map(|r| r["abs_resp"].as_f64())
                    .collect();
                if ours.len() != oracle["report_points"].as_array().map_or(0, Vec::len)
                    || ours.iter().zip(&theirs).any(|(a, b)| (a - b).abs() > 1e-9)
                {
                    problems.push(format!(
                        "report_points table: {} rows / values differ from the RPoint Table's {}",
                        ours.len(),
                        oracle["report_points"].as_array().map_or(0, Vec::len)
                    ));
                } else {
                    done.push("report_points table equals the RPoint Table".into());
                }
            }
            None => problems.push("report_points table has no AbsResp column".into()),
        }
    } else if oracle["report_points"]
        .as_array()
        .is_some_and(|v| !v.is_empty())
    {
        problems.push("no report_points table".into());
    }

    let mut cache: BTreeMap<u32, (Vec<f64>, Vec<f64>)> = BTreeMap::new();
    let mut samples = |c: &Curve| -> Result<(Vec<f64>, Vec<f64>), String> {
        if let Some(v) = cache.get(&c.index) {
            return Ok(v.clone());
        }
        let tr = ds
            .read_trace(c.index, 0, 0, u64::MAX)
            .map_err(|e| format!("trace {} read failed: {e}", c.index))?;
        let y = tr.channels.last().cloned().unwrap_or_default();
        let t = if c.irregular {
            tr.channels[0].clone()
        } else {
            (0..y.len()).map(|i| c.start + i as f64 / c.rate).collect()
        };
        cache.insert(c.index, (t.clone(), y.clone()));
        Ok((t, y))
    };

    // report points
    let rps = oracle["report_points"].as_array().unwrap_or(&empty);
    let (mut exact, mut approx, mut bad) = (0, 0, 0);
    for rp in rps {
        let (Some(cy), Some(fc), Some(time), Some(w), Some(abs)) = (
            rp["cycle"].as_u64(),
            rp["fc"].as_str(),
            rp["time"].as_f64(),
            rp["window"].as_f64(),
            rp["abs_resp"].as_f64(),
        ) else {
            continue;
        };
        let Some(c) = curves.get(&(cy, fc.to_string())) else {
            problems.push(format!(
                "report point on cycle {cy} flow cell {fc}: no such curve"
            ));
            bad += 1;
            continue;
        };
        let (t, y) = samples(c)?;
        let (lo, hi) = (time - w / 2.0, time + w / 2.0);
        let sel: Vec<f64> = t
            .iter()
            .zip(&y)
            .filter(|(t, _)| **t >= lo - EDGE && **t <= hi + EDGE)
            .map(|(_, y)| *y)
            .collect();
        if sel.is_empty() {
            problems.push(format!(
                "report point cycle {cy} Fc {fc} at {time} s: no samples in its window"
            ));
            bad += 1;
            continue;
        }
        let mean = sel.iter().sum::<f64>() / sel.len() as f64;
        if c.rate >= 5.0 {
            if (mean - abs).abs() <= 1e-6 {
                exact += 1;
            } else {
                bad += 1;
                if bad <= 5 {
                    problems.push(format!(
                        "report point cycle {cy} Fc {fc} at {time} s: mean {mean} vs AbsResp {abs}"
                    ));
                }
            }
        } else {
            let spread = sel.iter().copied().fold(f64::NEG_INFINITY, f64::max)
                - sel.iter().copied().fold(f64::INFINITY, f64::min);
            if (mean - abs).abs() <= spread.max(1.0) {
                approx += 1;
            } else {
                bad += 1;
                if bad <= 5 {
                    problems.push(format!("1 Hz report point cycle {cy} Fc {fc} at {time} s: mean {mean} vs AbsResp {abs} (spread {spread})"));
                }
            }
        }
    }
    if bad == 0 && !rps.is_empty() {
        done.push(format!(
            "{exact} report points recomputed exactly, {approx} within the window's spread (1 Hz)"
        ));
    }
    // allotropy's curves
    if let Some(a) = oracle["allotropy"].as_object() {
        let max_cycle = a["max_cycles"].as_u64().unwrap_or(u64::MAX);
        let list = a["curves"].as_array().unwrap_or(&empty);
        let (mut ok, mut differ) = (0, 0);
        for cv in list {
            let (Some(cy), Some(fc), Some(n)) = (
                cv["cycle"].as_u64(),
                cv["flow_cell"].as_str(),
                cv["n"].as_u64(),
            ) else {
                continue;
            };
            let Some(c) = curves.get(&(cy, fc.to_string())) else {
                problems.push(format!(
                    "allotropy has cycle {cy} flow cell {fc}; the reader has no such curve"
                ));
                continue;
            };
            if c.n != n {
                problems.push(format!(
                    "cycle {cy} Fc {fc}: {} samples, allotropy {n}",
                    c.n
                ));
                continue;
            }
            let (t, y) = samples(c)?;
            let mut this_bad = 0;
            for s in cv["samples"].as_array().unwrap_or(&empty) {
                let (Some(i), Some(time), Some(v)) = (s[0].as_u64(), s[1].as_f64(), s[2].as_f64())
                else {
                    continue;
                };
                let i = i as usize;
                let (ot, oy) = (
                    t.get(i).copied().unwrap_or(f64::NAN),
                    y.get(i).copied().unwrap_or(f64::NAN),
                );
                // allotropy gives float32 times; the reader's grid is exact
                if (ot - time).abs() > 1e-4 * time.abs().max(1.0)
                    || (oy - v).abs() > 1e-9 * v.abs().max(1.0)
                {
                    this_bad += 1;
                }
            }
            if this_bad > 0 {
                differ += 1;
                problems.push(format!(
                    "cycle {cy} Fc {fc}: {this_bad} samples differ from allotropy"
                ));
            } else {
                ok += 1;
            }
        }
        // the reader has no curve allotropy lacks, within the cycles it covered
        let theirs: Vec<(u64, String)> = list
            .iter()
            .filter_map(|c| Some((c["cycle"].as_u64()?, c["flow_cell"].as_str()?.to_string())))
            .collect();
        for k in curves.keys().filter(|k| k.0 <= max_cycle) {
            if !theirs.contains(k) {
                problems.push(format!(
                    "the reader has cycle {} Fc {}; allotropy does not",
                    k.0, k.1
                ));
            }
        }
        if differ == 0 {
            done.push(format!("{ok} curves equal allotropy's"));
        }
    }

    // run facts
    let exp = ds.experiment().ok_or("no experiment facts")?;
    let env = &oracle["environment"];
    let ins = exp.instrument.clone().unwrap_or_default();
    let acq = exp.acquisition.clone().unwrap_or_default();
    let method = exp.method.clone().unwrap_or_default();
    let mut fact = |what: &str, ours: Option<String>, theirs: Option<String>| {
        if ours != theirs {
            problems.push(format!("{what}: {ours:?}, file says {theirs:?}"));
        }
    };
    let s = |v: &Value| v.as_str().map(str::to_string);
    fact("serial", ins.serial.clone(), s(&env["InstrumentId"]));
    fact("software", ins.software.clone(), s(&env["Application"]));
    fact(
        "software version",
        ins.software_version.clone(),
        s(&env["Version"]),
    );
    fact("operator", acq.operator.clone(), s(&env["UserName"]));
    fact("method name", method.name.clone(), s(&env["RunType"]));
    fact(
        "model",
        ins.model.clone(),
        s(&env["ProcessingUnit"]).map(|u| u.replacen("Biacore", "Biacore ", 1).replace("  ", " ")),
    );
    let param = |k: &str| method.parameters.get(k).and_then(|q| q.value.as_f64());
    if param("cycles") != oracle["cycles"].as_f64() {
        problems.push(format!(
            "cycles {:?}, file has {}",
            param("cycles"),
            oracle["cycles"]
        ));
    }
    if param("flow_cells")
        != oracle["chip"]["NoFcs"]
            .as_str()
            .and_then(|v| v.parse().ok())
    {
        problems.push(format!(
            "flow cells {:?}, chip says {}",
            param("flow_cells"),
            oracle["chip"]["NoFcs"]
        ));
    }
    let vendor = ds
        .vendor_metadata()
        .map_err(|e| format!("vendor failed: {e}"))?;
    if vendor["biacore"]["chip"]["Id"] != oracle["chip"]["Id"] {
        problems.push(format!(
            "chip id {}, file says {}",
            vendor["biacore"]["chip"]["Id"], oracle["chip"]["Id"]
        ));
    }
    if let Some(chip) = oracle["allotropy"]["chip"]["identifier"].as_str()
        && vendor["biacore"]["chip"]["Id"].as_str() != Some(chip)
    {
        problems.push(format!("chip id differs from allotropy's {chip}"));
    }
    done.push("run facts match".into());
    // evaluation fits
    if let Some(fits) = oracle["fits"].as_array() {
        match compare_fits(ds.as_mut(), &info, fits) {
            Ok(msg) => done.push(msg),
            Err(msg) => problems.push(msg),
        }
    }
    if problems.is_empty() {
        Ok(done.join(", "))
    } else {
        Err(problems.join("; "))
    }
}

/// Our `fits` table against the oracle's parse of the item XML.
fn compare_fits(
    ds: &mut dyn openreadout_core::Dataset,
    info: &openreadout_core::FileInfo,
    fits: &[Value],
) -> Result<String, String> {
    let Some(ti) = info
        .tables
        .iter()
        .find(|t| t.name.as_deref() == Some("fits"))
    else {
        return if fits.is_empty() {
            Ok("no fits, as the oracle".into())
        } else {
            Err(format!("no fits table; the oracle has {} fits", fits.len()))
        };
    };
    if ti.row_count != fits.len() as u64 {
        return Err(format!(
            "{} fits, the oracle has {}",
            ti.row_count,
            fits.len()
        ));
    }
    let table = ds
        .read_table(ti.index, 0, u64::MAX)
        .map_err(|e| format!("fits table: {e}"))?;
    let col = |name: &str| ti.columns.iter().position(|c| c.name == name);
    let text = |k: usize, row: usize| -> Option<String> {
        let v = *table.columns[k].get(row)?;
        let cats = ti.columns[k].extra.get("categories")?.as_array()?;
        cats.get(v as usize)?.as_str().map(str::to_string)
    };
    let close = |a: f64, b: Option<f64>| match b {
        Some(b) => (a - b).abs() <= 1e-12 * b.abs().max(1e-300) || a == b,
        None => a.is_nan(),
    };
    let mut bad = Vec::new();
    let mut cells = 0usize;
    for (row, f) in fits.iter().enumerate() {
        for (ours, theirs) in [("sample", "sample"), ("curve", "curve"), ("model", "model")] {
            cells += 1;
            let got = col(ours).and_then(|k| text(k, row)).unwrap_or_default();
            if got != f[theirs].as_str().unwrap_or("") {
                bad.push(format!("fit {row} {ours}: {got:?} vs {}", f[theirs]));
            }
        }
        for (ours, v) in [
            ("chi2", f["chi2"].as_f64()),
            ("curves", f["curves"].as_f64()),
        ] {
            cells += 1;
            let got = col(ours).map_or(f64::NAN, |k| table.columns[k][row]);
            if !close(got, v) {
                bad.push(format!("fit {row} {ours}: {got} vs {v:?}"));
            }
        }
        for (name, pair) in f["params"].as_object().into_iter().flatten() {
            for (suffix, v) in [("", pair[0].as_f64()), ("_se", pair[1].as_f64())] {
                cells += 1;
                let got =
                    col(&format!("{name}{suffix}")).map_or(f64::NAN, |k| table.columns[k][row]);
                if !close(got, v) {
                    bad.push(format!("fit {row} {name}{suffix}: {got} vs {v:?}"));
                }
            }
        }
    }
    if bad.is_empty() {
        Ok(format!(
            "{} fits, {cells} cells equal the item XML's",
            fits.len()
        ))
    } else {
        let n = bad.len();
        bad.truncate(5);
        Err(format!("{n} fit cells differ: {}", bad.join("; ")))
    }
}
