//! Agilent Seahorse XF `.asyr` files against `oracle/seahorse_oracle.py`, a second
//! implementation of our notes (no independent reader or vendor export exists): wells, groups,
//! background wells, injections, reading and measurement counts, corrected emissions at 64
//! points per analyte, and the background wells' flat O2 signal (the physical check of the well
//! order). A match is self-consistency (`independent: false` in the results), not validation.
#![allow(dead_code)] // each test binary that includes this module uses a part of it
#![allow(clippy::if_not_else)] // comparison code

use std::path::Path;

use openreadout_core::FormatReader;
use serde_json::Value;

/// Format ids this module compares.
pub const FORMATS: &[&str] = &["agilent-seahorse-asyr"];

fn cats(info: &openreadout_core::TableInfo, col: usize, codes: &[f64]) -> Vec<String> {
    let labels: Vec<String> = info.columns[col].extra["categories"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    codes
        .iter()
        .map(|c| labels.get(*c as usize).cloned().unwrap_or_default())
        .collect()
}

#[allow(clippy::too_many_lines)]
pub fn compare_file(_id: &str, path: &Path, oracle: &Value) -> Result<String, String> {
    let mut ds = openreadout_biophys::SeahorseReader
        .open(path)
        .map_err(|e| format!("open failed: {e}"))?;
    let info = ds.info().map_err(|e| format!("info failed: {e}"))?;
    let mut problems = Vec::new();
    let empty = Vec::new();
    let strs = |v: &Value| -> Vec<String> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .map(|x| x.as_str().unwrap_or("").to_string())
                    .collect()
            })
            .unwrap_or_default()
    };
    // wells table: names, groups, background flags
    let wt = ds
        .read_table(0, 0, u64::MAX)
        .map_err(|e| format!("wells table: {e}"))?;
    let wells = cats(&info.tables[0], 0, &wt.columns[0]);
    if wells != strs(&oracle["wells"]) {
        problems.push("well names differ".to_string());
    }
    if cats(&info.tables[0], 3, &wt.columns[3]) != strs(&oracle["groups"]) {
        problems.push("well groups differ".to_string());
    }
    let bg: Vec<String> = wells
        .iter()
        .zip(&wt.columns[4])
        .filter(|(_, b)| **b > 0.5)
        .map(|(w, _)| w.clone())
        .collect();
    if bg != strs(&oracle["background"]) {
        problems.push(format!("background wells {bg:?}"));
    }
    // injections
    let it = ds
        .read_table(2, 0, u64::MAX)
        .map_err(|e| format!("injections table: {e}"))?;
    let theirs = oracle["injections"].as_array().unwrap_or(&empty);
    let ours_reagents = cats(&info.tables[2], 2, &it.columns[2]);
    let ours_ports = cats(&info.tables[2], 1, &it.columns[1]);
    if ours_reagents.len() != theirs.len() {
        problems.push(format!(
            "{} injections, oracle {}",
            ours_reagents.len(),
            theirs.len()
        ));
    } else {
        for (k, t) in theirs.iter().enumerate() {
            let conc_ok = match t["concentration"].as_f64() {
                Some(c) => (it.columns[3][k] - c).abs() < 1e-9,
                None => it.columns[3][k].is_nan(),
            };
            if t["reagent"].as_str() != Some(ours_reagents[k].as_str())
                || t["port"].as_str() != Some(ours_ports[k].as_str())
                || !conc_ok
            {
                problems.push(format!("injection {k} differs"));
                break;
            }
        }
    }
    // counts
    let readings = oracle["readings"].as_u64().unwrap_or(0);
    if info.traces.first().map(|t| t.sample_count) != Some(readings) {
        problems.push(format!("readings differ from {readings}"));
    }
    if info.tables[1].row_count != oracle["measurements"].as_u64().unwrap_or(0) {
        problems.push("measurement count differs".into());
    }
    if oracle["measurements"] != oracle["measure_commands"] {
        problems.push("the oracle's spans and Measure commands disagree".into());
    }
    // corrected emissions
    let analytes = strs(&oracle["analytes"]);
    let mut points = 0;
    for (k, an) in analytes.iter().enumerate() {
        let t = info
            .traces
            .iter()
            .position(|t| t.extra.get("analyte").and_then(Value::as_str) == Some(an.as_str()));
        let Some(t) = t else {
            problems.push(format!("no trace for analyte {an}"));
            continue;
        };
        let tr = ds
            .read_trace(t as u32, 0, 0, u64::MAX)
            .map_err(|e| format!("trace {k}: {e}"))?;
        for s in oracle["samples"][an].as_array().unwrap_or(&empty) {
            let (Some(r), Some(w), Some(v)) = (s[0].as_u64(), s[1].as_u64(), s[2].as_f64()) else {
                continue;
            };
            let ours = tr
                .channels
                .get(w as usize + 1)
                .and_then(|c| c.get(r as usize))
                .copied();
            if ours.is_none_or(|o| (o - v).abs() > 1e-9 * v.abs().max(1.0)) {
                problems.push(format!("{an} reading {r} well {w}: {ours:?} vs {v}"));
                break;
            }
            points += 1;
        }
    }
    // physical check of the well order (a Wave export checks it well by well instead)
    let wave = oracle.get("wave_export");
    if wave.is_none() {
        match oracle["background_ratio"].as_f64() {
            Some(r) if r < 0.5 => {}
            other => problems.push(format!(
                "background wells do not have the flattest O2 signal (ratio {other:?})"
            )),
        }
    }
    let mut wave_summary = String::new();
    if let Some(w) = wave {
        match compare_rates(ds.as_mut(), &info, &wells, &bg, w) {
            Ok(s) => wave_summary = s,
            Err(e) => problems.push(e),
        }
    }
    let exp = ds.experiment().ok_or("no experiment facts")?;
    let ins = exp.instrument.unwrap_or_default();
    if ins.serial.as_deref() != oracle["serial"].as_str() {
        problems.push(format!("serial {:?}", ins.serial));
    }
    if exp.method.and_then(|m| m.name).as_deref() != oracle["name"].as_str() {
        problems.push("assay name differs".into());
    }
    if problems.is_empty() {
        if wave.is_some() {
            return Ok(format!(
                "{} wells, {} readings, {points} emission points, injections match; Wave's rates: {wave_summary}",
                wells.len(),
                readings
            ));
        }
        Ok(format!(
            "self-consistency: {} wells, {} readings, {points} emission points, injections and background wells match; background/others O2 change {:.2}",
            wells.len(),
            readings,
            oracle["background_ratio"].as_f64().unwrap_or(f64::NAN)
        ))
    } else {
        Err(problems.join("; "))
    }
}

/// Largest allowed |ours − Wave's| for a rate (docs/formats/agilent-seahorse.md, "Validation").
fn tolerance(key: &str, wave: f64) -> f64 {
    match key {
        "OCR" => 0.2_f64.max(0.005 * wave.abs()),
        // PER is ECAR times constants: the same relative agreement
        "PER" => 1e-3_f64.max(1e-5 * wave.abs()),
        _ => 1e-3,
    }
}

/// Wave's own rates (its Prism export of the same assay) against the reader's `rates` table:
/// every exported column must match, within `tolerance`, the wells of one selected group
/// (Wave's column titles may be renamed), each group at most once; the background wells and the
/// measurement times must agree.
#[allow(clippy::too_many_lines, clippy::many_single_char_names)]
fn compare_rates(
    ds: &mut dyn openreadout_core::Dataset,
    info: &openreadout_core::FileInfo,
    wells: &[String],
    bg: &[String],
    wave: &Value,
) -> Result<String, String> {
    let t = info
        .tables
        .iter()
        .position(|t| t.name.as_deref() == Some("rates"))
        .ok_or("no rates table (the rates were not computed)")?;
    let tab = ds
        .read_table(t as u32, 0, u64::MAX)
        .map_err(|e| format!("rates table: {e}"))?;
    let names: Vec<String> = info.tables[t]
        .columns
        .iter()
        .map(|c| c.name.clone())
        .collect();
    let col = |n: &str| {
        names
            .iter()
            .position(|x| x == n)
            .ok_or(format!("no `{n}` column"))
    };
    let (cm, cw, ct) = (col("measurement")?, col("well")?, col("time")?);
    let well_of = cats(&info.tables[t], cw, &tab.columns[cw]);
    let nm = tab.columns[cm].iter().fold(0.0_f64, |a, b| a.max(*b)) as usize;
    let value = |c: usize, w: &str, m: usize| -> Option<f64> {
        (0..tab.columns[c].len())
            .find(|&r| well_of[r] == w && tab.columns[cm][r] as usize == m + 1)
            .map(|r| tab.columns[c][r])
    };
    // measurement times
    let empty = Vec::new();
    let times = wave["times_min"].as_array().unwrap_or(&empty);
    if times.len() != nm {
        return Err(format!(
            "{} measurements, Wave exported {}",
            nm,
            times.len()
        ));
    }
    for (m, tw) in times.iter().enumerate() {
        let ours = value(ct, &wells[0], m).unwrap_or(f64::NAN);
        if (ours - tw.as_f64().unwrap_or(f64::NAN)).abs() > 1e-5 {
            return Err(format!("measurement {} time {ours} min, Wave {tw}", m + 1));
        }
    }
    // background wells
    let wave_bg: Vec<String> = wave["background"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    if wave_bg != bg {
        return Err(format!("background wells {bg:?}, Wave {wave_bg:?}"));
    }
    let groups: Vec<(String, Vec<String>)> = wave["selected_groups"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .map(|g| {
            (
                g["name"].as_str().unwrap_or("").to_string(),
                g["wells"]
                    .as_array()
                    .unwrap_or(&empty)
                    .iter()
                    .filter_map(|w| w.as_str().map(str::to_string))
                    .collect(),
            )
        })
        .collect();
    let mut compared = 0usize;
    let mut worst: Vec<String> = Vec::new();
    for (key, cname) in [("OCR", "ocr"), ("ECAR", "ecar"), ("PER", "per")] {
        let Some(cols) = wave["tables"][key].as_array() else {
            continue;
        };
        let c = col(cname)?;
        let mut used: Vec<&str> = Vec::new();
        let mut max_err = 0.0_f64;
        for wc in cols {
            let reps: Vec<Vec<Option<f64>>> = wc["replicates"]
                .as_array()
                .unwrap_or(&empty)
                .iter()
                .map(|r| {
                    r.as_array()
                        .unwrap_or(&empty)
                        .iter()
                        .map(Value::as_f64)
                        .collect()
                })
                .collect();
            let title = wc["title"].as_str().unwrap_or("");
            // Wave writes PER 0 without a buffer factor; the reader leaves PER out then
            if key == "PER" && reps.iter().flatten().all(|v| v.is_none_or(|x| x == 0.0)) {
                continue;
            }
            let mut best: Option<(f64, &str, usize)> = None;
            for (g, gw) in &groups {
                if gw.len() != reps.len() || used.contains(&g.as_str()) {
                    continue;
                }
                let mut ok = true;
                let mut err = 0.0_f64;
                let mut n = 0;
                for (w, rep) in gw.iter().zip(&reps) {
                    for (m, v) in rep.iter().enumerate() {
                        let Some(v) = v else { continue };
                        let ours = value(c, w, m).unwrap_or(f64::NAN);
                        let d = (ours - v).abs();
                        if d.is_nan() || d > tolerance(key, *v) {
                            ok = false;
                        }
                        err = err.max(if d.is_nan() { f64::INFINITY } else { d });
                        n += 1;
                    }
                }
                if ok && best.is_none_or(|b| err < b.0) {
                    best = Some((err, g.as_str(), n));
                }
            }
            let Some((err, g, n)) = best else {
                return Err(format!(
                    "Wave's {key} column `{title}` matches no selected group within tolerance"
                ));
            };
            used.push(g);
            compared += n;
            max_err = max_err.max(err);
        }
        if !used.is_empty() {
            worst.push(format!(
                "{key} {} columns, max |Δ| {max_err:.4}",
                used.len()
            ));
        }
    }
    if compared == 0 {
        return Err("Wave's export holds no rates to compare".into());
    }
    Ok(format!("{compared} values ({})", worst.join(", ")))
}
