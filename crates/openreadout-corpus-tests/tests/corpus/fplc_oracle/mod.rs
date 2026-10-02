//! Cytiva ÄKTA / UNICORN results against their ground truth (`oracle/fplc_oracle.py`), for every
//! entry (development and held-out) through one code path (`tests/corpus/`):
//!
//! - every curve PyCORN (GPL-2.0, run as a black box) returns must be one of our traces (by
//!   name) with the same values at the oracle's sample positions: exactly for UNICORN 6/7
//!   exports (both sides widen the same 32-bit floats; PyCORN starts at our sample 5 and stops
//!   11 samples before our last, see `oracle/fplc_oracle.py`), within 1e-9 for `.res` files
//!   (integers × the descriptor's factor); `.res` volumes are compared as differences, and
//!   PyCORN's zero (it counts volumes from the last injection) must be our injection mark.
//!   PyCORN reports `.res` pH 10× larger than the factor the file declares: that relation is
//!   asserted instead of equality;
//! - allotropy (MIT) data cubes: sample count and first values (same five-sample shift);
//! - fraction and injection marks: time, volume and label, exactly;
//! - curves whose members are not whole float arrays (`not_arrays`) must not be traces;
//! - UNICORN's own peak table (UNICORN 6/7): each peak's apex on our curve lies within three
//!   samples' volume (at least 0.002 ml) of the vendor retention (+ the injection it is zeroed
//!   at); the vendor height equals our curve minus the table's baseline curve (zero when the table
//!   names none) at the retention, within 0.5 % (or 0.002 in the curve's unit); the vendor area
//!   equals the trapezoid integral of curve minus baseline between the peak limits within 0.5 %
//!   (or 0.002); the width is end minus start (`docs/formats/cytiva-unicorn.md`).
#![allow(dead_code)] // each test binary that includes this module uses a part of it
#![allow(
    clippy::many_single_char_names,
    clippy::float_cmp,
    clippy::if_not_else,
    clippy::semicolon_if_nothing_returned
)] // comparison code: short names for x, y, t; exact equality where values are copied

use std::path::Path;

use openreadout_core::FormatReader;
use serde_json::Value;

/// Format ids this module compares.
pub const FORMATS: &[&str] = &["cytiva-unicorn-res", "cytiva-unicorn-zip"];

fn close(a: f64, b: f64, abs: f64) -> bool {
    (a - b).abs() <= abs.max(1e-12 * a.abs().max(b.abs()))
}

/// Open `path` and compare it with `oracle`: `Ok(summary)` or `Err(differences)`. An oracle
/// with nothing to compare is an error, never a pass.
pub fn compare_file(
    format: &str,
    _id: &str,
    path: &Path,
    oracle: &Value,
) -> Result<String, String> {
    let res = format == "cytiva-unicorn-res";
    let mut ds = if res {
        openreadout_fplc::UnicornResReader.open(path)
    } else {
        openreadout_fplc::UnicornZipReader.open(path)
    }
    .map_err(|e| format!("open failed: {e}"))?;
    let info = ds.info().map_err(|e| format!("info failed: {e}"))?;
    let mut problems: Vec<String> = Vec::new();
    let mut compared_curves = 0usize;
    let mut compared_values = 0usize;
    let mut scaled: Vec<String> = Vec::new();
    let empty = Vec::new();

    // our injection marks (the zero PyCORN and UNICORN count `.res` volumes from)
    let injections: Vec<f64> = info
        .tables
        .iter()
        .find(|t| t.name.as_deref().is_some_and(|n| n.ends_with("injections")))
        .map(|t| {
            let tb = ds
                .read_table(t.index, 0, u64::MAX)
                .expect("injections table");
            tb.columns[1].clone()
        })
        .unwrap_or_default();

    for c in oracle["curves"].as_array().unwrap_or(&empty) {
        let name = c["name"].as_str().unwrap_or_default();
        let Some(t) = info.traces.iter().find(|t| t.name.as_deref() == Some(name)) else {
            problems.push(format!("no trace named `{name}`"));
            continue;
        };
        let n = c["n"].as_u64().unwrap_or(0);
        let shift = c["shift"].as_u64().unwrap_or(0);
        let expect_n = if res { n } else { n + 16 };
        if t.sample_count != expect_n {
            problems.push(format!(
                "`{name}`: {} samples, the oracle has {n} (expected {expect_n})",
                t.sample_count
            ));
            continue;
        }
        let tr = match ds.read_trace(t.index, 0, 0, u64::MAX) {
            Ok(tr) => tr,
            Err(e) => {
                problems.push(format!("`{name}`: read failed: {e}"));
                continue;
            }
        };
        let (val, vol) = (&tr.channels[0], &tr.channels[1]);
        let idx = c["index"].as_array().unwrap_or(&empty);
        let ovals = c["values"].as_array().unwrap_or(&empty);
        let ovols = c["volumes"].as_array().unwrap_or(&empty);
        // PyCORN scales `.res` pH and pressure by fixed factors (÷10, ÷100); where a file
        // declares another factor (sample1: pressure 0.001 MPa, pH 0.01), PyCORN's values are
        // ours × 10 — the file's own method dump (a 300 kPa pressure limit the ÷100 values
        // would exceed for the whole run) shows the declared factor is the right one
        let kind = t
            .extra
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let ratio10 = res
            && matches!(kind, "ph" | "pressure")
            && idx
                .iter()
                .zip(ovals)
                .filter_map(|(i, o)| Some((val.get(i.as_u64()? as usize)?, o.as_f64()?)))
                .any(|(v, o)| *v != 0.0 && close(v * 10.0, o, 1e-9));
        let v0_ours = vol.get(shift as usize).copied().unwrap_or(0.0);
        let v0_oracle = ovols.first().and_then(Value::as_f64).unwrap_or(0.0);
        let mut bad = 0usize;
        for ((i, ov), ow) in idx.iter().zip(ovals).zip(ovols) {
            let Some(k) = i.as_u64().map(|i| (i + shift) as usize) else {
                continue;
            };
            let (Some(ours), Some(ours_v)) = (val.get(k), vol.get(k)) else {
                bad += 1;
                continue;
            };
            compared_values += 1;
            match ov.as_f64() {
                Some(o) if ratio10 => {
                    if !close(*ours * 10.0, o, 1e-9) {
                        bad += 1;
                    }
                }
                Some(o) if res => {
                    if !close(*ours, o, 1e-9) {
                        bad += 1;
                    }
                }
                Some(o) => {
                    if ours.to_bits() != o.to_bits() && !(ours.is_nan() && o.is_nan()) {
                        bad += 1;
                    }
                }
                None => {
                    if ours.is_finite() {
                        bad += 1;
                    }
                }
            }
            if let Some(w) = ow.as_f64() {
                let ok = if res {
                    close(ours_v - v0_ours, w - v0_oracle, 1e-9)
                } else {
                    ours_v.to_bits() == w.to_bits()
                };
                if !ok {
                    bad += 1;
                }
            }
        }
        if bad > 0 {
            problems.push(format!(
                "`{name}`: {bad} of {} sampled values or volumes differ",
                idx.len()
            ));
        }
        if res {
            // PyCORN's zero: the last injection (0 when there is none)
            let zero = injections.last().copied().unwrap_or(0.0);
            if !close(v0_ours - v0_oracle, zero, 1e-9) {
                problems.push(format!(
                    "`{name}`: PyCORN's volume zero is at {} ml of ours, our last injection at {zero}",
                    v0_ours - v0_oracle
                ));
            }
        }
        if ratio10 {
            scaled.push(name.to_string());
        }
        compared_curves += 1;
    }

    // curves whose members are not whole float arrays must be refused (no trace)
    let mut refused = 0usize;
    for name in oracle["not_arrays"].as_array().unwrap_or(&empty) {
        let name = name.as_str().unwrap_or_default();
        if info.traces.iter().any(|t| t.name.as_deref() == Some(name)) {
            problems.push(format!(
                "`{name}` is not a whole float array, yet we read it"
            ));
        } else {
            refused += 1;
        }
    }

    // allotropy (second opinion, UNICORN 6/7)
    let mut allo_checked = 0usize;
    if let Some(a) = oracle["allotropy"].as_object() {
        for (label, cube) in a {
            let Some(t) = info
                .traces
                .iter()
                .find(|t| t.name.as_deref() == Some(label.as_str()))
            else {
                problems.push(format!("allotropy cube `{label}`: no such trace"));
                continue;
            };
            let n = cube["n"].as_u64().unwrap_or(0);
            if t.sample_count != n + 16 {
                problems.push(format!(
                    "allotropy cube `{label}`: {n} values, we have {} (expected n + 16)",
                    t.sample_count
                ));
                continue;
            }
            let tr = ds.read_trace(t.index, 0, 5, 5).map_err(|e| e.to_string())?;
            // allotropy converts conductivity from mS/cm to S/m (÷ 10)
            let k10 = if t.extra.get("kind").and_then(Value::as_str) == Some("conductivity") {
                10.0
            } else {
                1.0
            };
            for (k, f) in cube["first"]
                .as_array()
                .unwrap_or(&empty)
                .iter()
                .enumerate()
            {
                let ok = match (f.as_f64(), tr.channels[0].get(k)) {
                    (Some(a), Some(b)) => close(a * k10, *b, 1e-6 * b.abs()),
                    _ => false,
                };
                if !ok {
                    problems.push(format!("allotropy cube `{label}`: value {k} differs"));
                    break;
                }
            }
            allo_checked += 1;
        }
    }

    // fraction and injection marks
    let mut marks = 0usize;
    let table = |ds: &mut Box<dyn openreadout_core::Dataset>,
                 suffix: &str|
     -> Option<(Vec<Vec<f64>>, Vec<String>)> {
        let t = info
            .tables
            .iter()
            .find(|t| t.name.as_deref().is_some_and(|n| n.ends_with(suffix)))?;
        let tb = ds.read_table(t.index, 0, u64::MAX).ok()?;
        let cats: Vec<String> = t.columns[2].extra["categories"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        Some((tb.columns, cats))
    };
    if res {
        if let Some(f) = oracle["fractions"].as_array().filter(|a| !a.is_empty()) {
            match table(&mut ds, "fractions") {
                None => problems.push("no fractions table".into()),
                Some((cols, cats)) => {
                    let zero = injections.last().copied().unwrap_or(0.0);
                    if cols[1].len() != f.len() {
                        problems.push(format!("{} fractions, PyCORN {}", cols[1].len(), f.len()));
                    }
                    for (k, fr) in f.iter().enumerate() {
                        let v = fr[0].as_f64().unwrap_or(f64::NAN);
                        let l = fr[1].as_str().unwrap_or_default();
                        let ours_l = cols[2]
                            .get(k)
                            .and_then(|c| cats.get(*c as usize))
                            .map(String::as_str);
                        if !cols[1].get(k).is_some_and(|x| close(*x - zero, v, 1e-6))
                            || ours_l != Some(l)
                        {
                            problems.push(format!("fraction {k} differs ({v} ml `{l}`)"));
                            break;
                        }
                        marks += 1;
                    }
                }
            }
        }
    } else if let Some(ev) = oracle["events"].as_object() {
        for (kind, suffix) in [("Fraction", "fractions"), ("Injection", "injections")] {
            let Some(list) = ev.get(kind).and_then(Value::as_array) else {
                continue;
            };
            match table(&mut ds, suffix) {
                None if list.is_empty() => {}
                None => problems.push(format!("no {suffix} table")),
                Some((cols, cats)) => {
                    if cols[0].len() != list.len() {
                        problems.push(format!(
                            "{} {suffix}, the XML {}",
                            cols[0].len(),
                            list.len()
                        ));
                        continue;
                    }
                    for (k, e) in list.iter().enumerate() {
                        let t = e[0].as_f64();
                        let v = e[1].as_f64();
                        let l = e[2].as_str().unwrap_or_default();
                        let ours_l = cols[2]
                            .get(k)
                            .and_then(|c| cats.get(*c as usize))
                            .map(String::as_str);
                        let same = |a: Option<f64>, b: f64| a.map_or(b.is_nan(), |a| a == b);
                        if !same(t, cols[0][k]) || !same(v, cols[1][k]) || ours_l != Some(l) {
                            problems.push(format!("{suffix} {k} differs"));
                            break;
                        }
                        marks += 1;
                    }
                }
            }
        }
    }

    // UNICORN's own peak table against our curve
    let mut vendor_peaks = 0usize;
    for pt in oracle["peak_tables"].as_array().unwrap_or(&empty) {
        let num = pt["curve_number"]
            .as_str()
            .and_then(|s| s.parse::<f64>().ok());
        let Some(t) = info
            .traces
            .iter()
            .find(|t| t.extra.get("curve_number").and_then(Value::as_f64) == num && num.is_some())
        else {
            if refused > 0 {
                continue; // its curve is one of the refused ones
            }
            problems.push(format!(
                "peak table `{}`: no trace with curve number {num:?}",
                pt["name"].as_str().unwrap_or_default()
            ));
            continue;
        };
        let zero_n = pt["zero_at_injection"]
            .as_str()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(1);
        let zero = injections
            .get(zero_n.saturating_sub(1))
            .copied()
            .unwrap_or(0.0);
        let tr = ds
            .read_trace(t.index, 0, 0, u64::MAX)
            .map_err(|e| e.to_string())?;
        let (val, vol) = (&tr.channels[0], &tr.channels[1]);
        // the baseline curve the table measures above, when it names one
        let bnum = pt["baseline_curve_number"]
            .as_str()
            .and_then(|s| s.parse::<f64>().ok());
        let base = match bnum {
            None => None,
            Some(b) => {
                let Some(bt) = info
                    .traces
                    .iter()
                    .find(|x| x.extra.get("curve_number").and_then(Value::as_f64) == Some(b))
                else {
                    problems.push(format!(
                        "peak table `{}`: no trace for its baseline curve {b}",
                        pt["name"].as_str().unwrap_or_default()
                    ));
                    continue;
                };
                // the reader names the same trace as the table's baseline
                if info.tables.iter().any(|tb| {
                    tb.extra
                        .get("baseline_curve_number")
                        .and_then(Value::as_f64)
                        == Some(b)
                        && tb.extra.get("baseline_trace").and_then(Value::as_u64)
                            != Some(u64::from(bt.index))
                }) {
                    problems.push(format!("baseline curve {b}: the table names another trace"));
                }
                let btr = ds
                    .read_trace(bt.index, 0, 0, u64::MAX)
                    .map_err(|e| e.to_string())?;
                Some((btr.channels[1].clone(), btr.channels[0].clone()))
            }
        };
        let interp = |xs: &[f64], ys: &[f64], x: f64| -> f64 {
            let k = xs.partition_point(|v| *v < x);
            if k == 0 || k >= xs.len() {
                return ys
                    .get(k.min(xs.len().saturating_sub(1)))
                    .copied()
                    .unwrap_or(f64::NAN);
            }
            let (x0, x1, y0, y1) = (xs[k - 1], xs[k], ys[k - 1], ys[k]);
            if x1 == x0 {
                y0
            } else {
                y0 + (y1 - y0) * (x - x0) / (x1 - x0)
            }
        };
        let net = |x: f64| -> f64 {
            interp(vol, val, x) - base.as_ref().map_or(0.0, |(bv, ba)| interp(bv, ba, x))
        };
        for pk in pt["peaks"].as_array().unwrap_or(&empty) {
            let (Some(r), Some(s), Some(e), Some(h)) = (
                pk["retention"].as_f64(),
                pk["start"].as_f64(),
                pk["end"].as_f64(),
                pk["height"].as_f64(),
            ) else {
                continue;
            };
            let (r, s, e) = (r + zero, s + zero, e + zero);
            let lo = vol.partition_point(|x| *x < s);
            let hi = vol.partition_point(|x| *x <= e);
            if lo >= hi {
                problems.push(format!(
                    "vendor peak at {r} ml: no samples between {s} and {e}"
                ));
                continue;
            }
            let (k, _) = (lo..hi).fold((lo, f64::NEG_INFINITY), |a, i| {
                if val[i] > a.1 { (i, val[i]) } else { a }
            });
            let step = (vol[k.min(vol.len() - 2) + 1] - vol[k.saturating_sub(1)])
                .abs()
                .max(1e-9);
            let ours_h = net(r);
            // the trapezoid integral of curve − baseline over [s, e], ends interpolated
            let mut xs = vec![s];
            xs.extend(vol[lo..hi].iter().copied().filter(|x| *x > s && *x < e));
            xs.push(e);
            let ours_a: f64 = xs
                .windows(2)
                .map(|w| (w[1] - w[0]) * (net(w[0]) + net(w[1])) / 2.0)
                .sum();
            if base.is_none() && (vol[k] - r).abs() > (3.0 * step).max(0.002) {
                problems.push(format!(
                    "vendor peak at {r:.4} ml: our apex at {:.4} ml",
                    vol[k]
                ));
            } else if !close(ours_h, h, 0.002_f64.max(0.005 * h.abs())) {
                problems.push(format!(
                    "vendor peak at {r:.4} ml: height {h}, our curve − baseline {ours_h}"
                ));
            } else if let Some(a) = pk["area"].as_f64()
                && !close(ours_a, a, 0.002_f64.max(0.005 * a.abs()))
            {
                problems.push(format!(
                    "vendor peak at {r:.4} ml: area {a}, our integral {ours_a}"
                ));
            } else if let Some(w) = pk["width"].as_f64()
                && !close(e - s, w, 1e-4_f64.max(1e-5 * w.abs()))
            {
                problems.push(format!(
                    "vendor peak at {r:.4} ml: width {w}, end − start {}",
                    e - s
                ));
            } else {
                vendor_peaks += 1;
            }
        }
    }

    if compared_curves == 0 && allo_checked == 0 && marks == 0 && vendor_peaks == 0 && refused == 0
    {
        return Err(format!(
            "nothing compared{}",
            oracle["pycorn_error"]
                .as_str()
                .map(|e| format!(" (PyCORN: {e})"))
                .unwrap_or_default()
        ));
    }
    if problems.is_empty() {
        Ok(format!(
            "{compared_curves} curves ({compared_values} sampled values) vs PyCORN{}, {allo_checked} allotropy cubes, {marks} marks, {vendor_peaks} vendor peaks, {refused} damaged curves refused",
            if scaled.is_empty() {
                String::new()
            } else {
                format!(" (PyCORN = ours × 10 for {})", scaled.join(", "))
            }
        ))
    } else {
        Err(problems.join("; "))
    }
}
