//! Shimadzu LabSolutions `.lcd` files against their ground truth, for every entry (development
//! and held-out) through one code path (`tests/corpus/`). Two oracle shapes:
//!
//! - chromConverter (GPL-3.0, run as a black box by `oracle/shimadzu_oracle.py`): `chromatogram`
//!   {name, n, xxh3} — the stored points of our trace of that name (`extra.stream_label` or its
//!   name; the vendor's leading t = 0 point left out, values divided back by `extra.raw_scale`)
//!   must be chromConverter's, bit for bit (xxh3-128 of the little-endian f64 values);
//! - the vendor's own LabSolutions ASCII export of the same run (`shimadzu_export`, written by
//!   `oracle/shimadzu_export.py`, also for held-out inputs through `oracle/gen_heldout.py`): every
//!   `[LC Chromatogram(<detector>)]` section must be one of our traces (matched by
//!   `extra.export_section`), with the same number of points, sampling interval, start and end
//!   time, and values: the export's `Intensity` column times its `Intensity Multiplier`, within
//!   half a printed unit (the export prints whole numbers); every `[LC Status Trace(...)]`
//!   section one of our status traces (by `extra.export_section`, else in order), in the
//!   export's unit, every value within half a printed unit; every `[Peak Table(<signal>)]` row a
//!   `vendor_peaks` row of that signal (retention time, area, height);
//! - for LC-MS data, msconvert's ground-truth slice (`ms_ground_truth`, written by
//!   `oracle/shimadzu_ms_gt.py`): every MRM/SIM spectrum's MS level and points (m/z within
//!   0.005, intensities equal); a profile spectrum (full or product-ion scan) must be refused
//!   with exit 6.
#![allow(dead_code)] // each test binary that includes this module uses a part of it

use std::path::Path;

use openreadout_core::FormatReader;
use serde_json::Value;

fn close(a: f64, b: f64, rel: f64) -> bool {
    (a - b).abs() <= rel * a.abs().max(b.abs()).max(1e-12)
}

/// Open `path` and compare it with `oracle`: `Ok(summary)` or `Err(differences)`. An oracle
/// with nothing to compare is an error, never a pass.
pub fn compare_file(id: &str, path: &Path, oracle: &Value) -> Result<String, String> {
    let mut ds = openreadout_chrom::ShimadzuReader
        .open(path)
        .map_err(|e| format!("open failed: {e}"))?;
    let info = ds.info().map_err(|e| format!("info failed: {e}"))?;
    let mut problems: Vec<String> = Vec::new();
    let mut done: Vec<String> = Vec::new();
    if let Some(c) = oracle["chromatogram"].as_object() {
        let name = c["name"].as_str().unwrap_or_default();
        match info.traces.iter().find(|t| {
            t.name.as_deref() == Some(name)
                || t.extra.get("stream_label").and_then(Value::as_str) == Some(name)
        }) {
            None => problems.push(format!("no trace named {name}")),
            Some(t) => {
                let n = c["n"].as_u64().unwrap_or(0);
                // chromatograms carry the vendor's leading t = 0 point that chromConverter does not
                let stored = t
                    .extra
                    .get("stored_points")
                    .and_then(Value::as_u64)
                    .unwrap_or(t.sample_count);
                let lead = t.sample_count - stored.min(t.sample_count);
                let raw_scale = t.extra.get("raw_scale").and_then(Value::as_f64);
                if stored == n {
                    match ds.read_trace(t.index, 0, lead, n) {
                        Err(e) => problems.push(format!("{name}: read failed: {e}")),
                        Ok(tr) => {
                            // back to the stored integers
                            let bytes: Vec<u8> = tr.channels[0]
                                .iter()
                                .map(|v| raw_scale.map_or(*v, |k| (v / k).round()))
                                .flat_map(f64::to_le_bytes)
                                .collect();
                            let hash = format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&bytes));
                            if Some(hash.as_str()) == c["xxh3"].as_str() {
                                done.push(format!(
                                    "{name}: {n} points bit-exact against chromConverter"
                                ));
                            } else {
                                problems.push(format!("{name}: values differ from chromConverter"));
                            }
                        }
                    }
                } else {
                    problems.push(format!(
                        "{name}: {stored} stored points, chromConverter {n}"
                    ));
                }
            }
        }
    }
    if let Some(chroms) = oracle["shimadzu_export"]["chromatograms"].as_array() {
        for c in chroms {
            compare_export_chromatogram(ds.as_mut(), &info, c, &mut problems, &mut done);
        }
    }
    if let Some(status) = oracle["shimadzu_export"]["status_traces"].as_array() {
        compare_export_status(ds.as_mut(), &info, status, &mut problems, &mut done);
    }
    if let Some(tables) = oracle["shimadzu_export"]["peak_tables"].as_object() {
        compare_export_peaks(ds.as_mut(), &info, tables, &mut problems, &mut done);
    }
    if let Some(spectra) = oracle["ms_ground_truth"]["spectra"].as_array() {
        compare_ms_ground_truth(ds.as_mut(), spectra, &mut problems, &mut done);
    }
    if done.is_empty() && problems.is_empty() {
        return Err(format!(
            "{id}: the oracle holds no chromatogram to compare (neither chromConverter's nor a vendor export's)"
        ));
    }
    if problems.is_empty() {
        Ok(done.join("; "))
    } else {
        Err(problems.join("; "))
    }
}

/// The export's `[LC Status Trace(...)]` sections against our status traces (`extra.detector`
/// `status`): matched by `extra.export_section` (newer layout), else in order (older layout:
/// the file names no status log). Same number of points, the export's unit, and every value
/// (the printed number times the multiplier) within half a printed unit.
fn compare_export_status(
    ds: &mut dyn openreadout_core::Dataset,
    info: &openreadout_core::FileInfo,
    status: &[Value],
    problems: &mut Vec<String>,
    done: &mut Vec<String>,
) {
    let ours: Vec<&openreadout_core::TraceInfo> = info
        .traces
        .iter()
        .filter(|t| t.extra.get("detector").and_then(Value::as_str) == Some("status"))
        .collect();
    let (mut traces, mut points) = (0usize, 0usize);
    for (k, s) in status.iter().enumerate() {
        let name = s["name"].as_str().unwrap_or_default();
        let section = format!("LC Status Trace({name})");
        let Some(t) = ours
            .iter()
            .find(|t| {
                t.extra.get("export_section").and_then(Value::as_str) == Some(section.as_str())
            })
            .or_else(|| {
                ours.iter()
                    .all(|t| !t.extra.contains_key("export_section"))
                    .then(|| ours.get(k))
                    .flatten()
            })
        else {
            problems.push(format!("no status trace for the export's [{section}]"));
            continue;
        };
        let tname = t.name.clone().unwrap_or_default();
        let want: Vec<f64> = s["values"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default();
        if t.sample_count != want.len() as u64 {
            problems.push(format!(
                "{tname}: {} points, the export's [{section}] {}",
                t.sample_count,
                want.len()
            ));
            continue;
        }
        let unit = match s["intensity_units"].as_str().unwrap_or_default() {
            "uV" => "µV".to_string(),
            "C" => "°C".to_string(),
            u => u.to_string(),
        };
        if t.channels[0].unit.as_deref().unwrap_or_default() != unit {
            problems.push(format!(
                "{tname}: unit {:?}, the export's [{section}] {unit}",
                t.channels[0].unit
            ));
        }
        let mult = s["intensity_multiplier"].as_f64().unwrap_or(1.0);
        let decimals = i32::try_from(s["decimals"].as_u64().unwrap_or(0)).unwrap_or(0);
        let tol = 0.5 * 10f64.powi(-decimals) * mult.abs() + 1e-9;
        match ds.read_trace(t.index, 0, 0, want.len() as u64) {
            Err(e) => problems.push(format!("{tname}: read failed: {e}")),
            Ok(tr) => {
                let bad = (0..want.len())
                    .filter(|&i| (tr.channels[0][i] - want[i] * mult).abs() > tol)
                    .count();
                if bad > 0 {
                    problems.push(format!(
                        "{tname}: {bad} of {} values differ from the export's [{section}]",
                        want.len()
                    ));
                } else {
                    traces += 1;
                    points += want.len();
                }
            }
        }
    }
    if traces > 0 {
        done.push(format!(
            "{traces} status traces = the export's (unit, {points} values within half a printed unit)"
        ));
    }
}

/// The export's `[Peak Table(<signal>)]` sections against our `vendor_peaks` rows of the same
/// signal: retention time within the printed 0.001 min, area and height within half a unit.
fn compare_export_peaks(
    ds: &mut dyn openreadout_core::Dataset,
    info: &openreadout_core::FileInfo,
    tables: &serde_json::Map<String, Value>,
    problems: &mut Vec<String>,
    done: &mut Vec<String>,
) {
    let wanted: usize = tables
        .values()
        .map(|v| v.as_array().map_or(0, Vec::len))
        .sum();
    if wanted == 0 {
        return;
    }
    let Some(t) = info
        .tables
        .iter()
        .find(|t| t.name.as_deref() == Some("vendor_peaks"))
    else {
        problems.push(format!(
            "no vendor_peaks table for the export's {wanted} peaks"
        ));
        return;
    };
    let cats: Vec<String> = t.columns[0]
        .extra
        .get("categories")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let Ok(tab) = ds.read_table(t.index, 0, t.row_count) else {
        problems.push("vendor_peaks: read failed".into());
        return;
    };
    let mut matched = 0usize;
    for (signal, peaks) in tables {
        let peaks = peaks.as_array().cloned().unwrap_or_default();
        if peaks.is_empty() {
            continue;
        }
        let key = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        let Some(code) = cats.iter().position(|c| key(c) == key(signal)) else {
            problems.push(format!(
                "the export's [Peak Table({signal})]: no vendor_peaks signal of that name"
            ));
            continue;
        };
        let rows: Vec<usize> = (0..tab.columns[0].len())
            .filter(|&i| tab.columns[0][i] as usize == code)
            .collect();
        if rows.len() != peaks.len() {
            problems.push(format!(
                "{signal}: {} vendor peaks, the export {}",
                rows.len(),
                peaks.len()
            ));
            continue;
        }
        for (i, p) in rows.iter().zip(&peaks) {
            let (rt, area, height) = (tab.columns[2][*i], tab.columns[5][*i], tab.columns[6][*i]);
            let (wrt, warea, wheight) = (
                p["rt_min"].as_f64().unwrap_or(f64::NAN),
                p["area"].as_f64().unwrap_or(f64::NAN),
                p["height"].as_f64().unwrap_or(f64::NAN),
            );
            if (rt - wrt).abs() > 0.0005 + 1e-9
                || (area - warea).abs() > 0.5 + 1e-9
                || (height - wheight).abs() > 0.5 + 1e-9
            {
                problems.push(format!(
                    "{signal} peak at {wrt} min: ours {rt}/{area}/{height}, the export {wrt}/{warea}/{wheight}"
                ));
            } else {
                matched += 1;
            }
        }
    }
    if matched > 0 {
        done.push(format!(
            "{matched} vendor peaks = the export's peak tables (RT, area, height)"
        ));
    }
}

/// msconvert's ground-truth spectra against ours (run 0): MRM/SIM spectra point for point;
/// profile spectra (full and product-ion scans) must be refused (exit 6).
fn compare_ms_ground_truth(
    ds: &mut dyn openreadout_core::Dataset,
    spectra: &[Value],
    problems: &mut Vec<String>,
    done: &mut Vec<String>,
) {
    let (mut matched, mut points, mut refused) = (0usize, 0usize, 0usize);
    for g in spectra {
        let scan = g["scan"].as_u64().unwrap_or(0);
        let level = g["ms_level"].as_u64().unwrap_or(0);
        match ds.read_spectrum(0, scan) {
            Err(e) if e.exit_code() == 6 => refused += 1,
            Err(e) => problems.push(format!("spectrum {scan}: {e}")),
            Ok(s) => {
                if u64::from(s.ms_level) != level {
                    problems.push(format!(
                        "spectrum {scan}: MS level {} vs {level}",
                        s.ms_level
                    ));
                    continue;
                }
                let n = g["n_points"].as_u64().unwrap_or(0) as usize;
                let want: Vec<(f64, f64)> = g["points"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|p| Some((p[0].as_f64()?, p[1].as_f64()?)))
                            .collect()
                    })
                    .unwrap_or_default();
                let ours: Vec<(f64, f64)> =
                    s.mz.iter()
                        .zip(&s.intensity)
                        .map(|(m, v)| (*m, f64::from(*v)))
                        .collect();
                let nonzero: Vec<&(f64, f64)> = ours.iter().filter(|p| p.1 != 0.0).collect();
                let same = ours.len() == n
                    && nonzero.len() == want.len()
                    && nonzero
                        .iter()
                        .zip(&want)
                        .all(|(o, w)| (o.0 - w.0).abs() <= 0.005 && (o.1 - w.1).abs() <= 1e-6);
                if same {
                    matched += 1;
                    points += n;
                } else {
                    problems.push(format!(
                        "spectrum {scan}: {} points ({} non-zero) vs msconvert's {n} ({})",
                        ours.len(),
                        nonzero.len(),
                        want.len()
                    ));
                }
            }
        }
    }
    if matched + refused > 0 {
        done.push(format!(
            "{matched} spectra = msconvert's ({points} points){}",
            if refused > 0 {
                format!(", {refused} profile spectra refused (exit 6) as documented")
            } else {
                String::new()
            }
        ));
    }
}

/// One `[LC Chromatogram(...)]` section of the vendor's ASCII export against our trace.
fn compare_export_chromatogram(
    ds: &mut dyn openreadout_core::Dataset,
    info: &openreadout_core::FileInfo,
    c: &Value,
    problems: &mut Vec<String>,
    done: &mut Vec<String>,
) {
    let name = c["name"].as_str().unwrap_or_default();
    let section = format!("LC Chromatogram({name})");
    let Some(t) = info
        .traces
        .iter()
        .find(|t| t.extra.get("export_section").and_then(Value::as_str) == Some(section.as_str()))
    else {
        problems.push(format!(
            "no trace for the export's [{section}] (traces: {})",
            info.traces
                .iter()
                .map(|t| t.name.clone().unwrap_or_default())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        return;
    };
    let tname = t.name.clone().unwrap_or_default();
    let before = problems.len();
    let points = c["points"].as_u64().unwrap_or(0);
    if t.sample_count != points {
        problems.push(format!(
            "{tname}: {} points, the vendor export {points}",
            t.sample_count
        ));
    }
    let interval = c["interval_ms"].as_f64().unwrap_or(f64::NAN);
    let ours_iv = t.extra.get("interval_ms").and_then(Value::as_f64);
    if !ours_iv.is_some_and(|v| close(v, interval, 1e-9)) {
        problems.push(format!(
            "{tname}: interval {ours_iv:?} ms, the vendor export {interval}"
        ));
    }
    let axis = &t.extra.get("axis").cloned().unwrap_or(Value::Null);
    let first = axis["first"].as_f64().unwrap_or(f64::NAN);
    let step = axis["step"].as_f64().unwrap_or(f64::NAN);
    let (start, end) = (
        c["start_min"].as_f64().unwrap_or(f64::NAN),
        c["end_min"].as_f64().unwrap_or(f64::NAN),
    );
    let last = first + step * (t.sample_count.saturating_sub(1) as f64);
    // the export prints times to 3 decimals of a minute (0.06 s)
    if (first - start).abs() > 1e-3 || (last - end).abs() > 1e-3 {
        problems.push(format!(
            "{tname}: time axis {first:.4}-{last:.4} min, the vendor export {start}-{end}"
        ));
    }
    // every value when the oracle has them, else the first ones
    let want: Vec<f64> = c["values"]
        .as_array()
        .or_else(|| c["first"].as_array())
        .map(|a| a.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    let mut compared = 0usize;
    if !want.is_empty() {
        let mult = c["intensity_multiplier"].as_f64().unwrap_or(1.0);
        match ds.read_trace(t.index, 0, 0, want.len() as u64) {
            Err(e) => problems.push(format!("{tname}: read failed: {e}")),
            Ok(tr) => {
                let ours = &tr.channels[0];
                // the export prints whole numbers of `Intensity`: half a unit, times the multiplier
                let tol = 0.5 * mult.abs() * (1.0 + 1e-9) + 1e-12;
                let bad: Vec<usize> = (0..want.len())
                    .filter(|&i| ours.get(i).is_none_or(|o| (o - want[i] * mult).abs() > tol))
                    .collect();
                compared = want.len() - bad.len();
                if let Some(&i) = bad.first() {
                    problems.push(format!(
                        "{tname}: {} of {} values differ from the vendor export (first at point {i}: ours {:?}, export {})",
                        bad.len(),
                        want.len(),
                        ours.get(i),
                        want[i] * mult
                    ));
                }
            }
        }
    }
    if problems.len() == before {
        done.push(format!(
            "{tname} = export [{section}]: {points} points, interval and time axis agree, {compared} values within half a printed unit"
        ));
    }
}
