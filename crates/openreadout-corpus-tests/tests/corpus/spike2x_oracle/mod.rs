//! 64-bit Spike2 `.smrx` files against the Spike2 software's own exports of the same recordings
//! (`oracle/spike2x_oracle.py` → `corpus/oracle/spike2x/<id>.json`; the `.smrx` itself is never
//! read by the oracle):
//!
//! - MATLAB exports: per waveform channel the sample count, start, interval, unit and the stored
//!   int16 codes recovered from the exported values (xxh3-128 of all of them as int64, exact);
//!   channels processed at export are compared for count, start and interval only; per event
//!   channel the count and every time in ticks (xxh3-128).
//! - Text exports: per waveform channel the sample count, start, interval and 500 evenly spaced
//!   values within 1.5e-5 (the export prints 5 decimals); per event channel the sample bin of
//!   every event (xxh3-128).
#![allow(dead_code)] // each test binary that includes this module uses a part of it
#![allow(clippy::many_single_char_names, clippy::float_cmp)] // comparison code

use std::path::Path;

use openreadout_core::FormatReader;
use openreadout_core::model::TraceInfo;
use openreadout_core::reader::Dataset;
use serde_json::Value;

/// Oracle directory under `corpus/oracle`.
pub const DIR: &str = "spike2x";

fn h64(v: &[i64]) -> String {
    let mut b = Vec::with_capacity(v.len() * 8);
    for x in v {
        b.extend_from_slice(&x.to_le_bytes());
    }
    format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&b))
}

fn read_all(ds: &mut dyn Dataset, t: &TraceInfo) -> Result<Vec<f64>, String> {
    let mut out = Vec::with_capacity(t.sample_count as usize);
    while (out.len() as u64) < t.sample_count {
        let page = ds
            .read_trace(t.index, 0, out.len() as u64, u64::MAX)
            .map_err(|e| format!("trace {}: read failed: {e}", t.index))?;
        let got = page.channels.first().map_or(0, Vec::len);
        if got == 0 {
            break;
        }
        out.extend_from_slice(&page.channels[0]);
    }
    Ok(out)
}

fn close(a: f64, b: f64, rel: f64) -> bool {
    (a - b).abs() <= rel * a.abs().max(b.abs()).max(1e-12)
}

/// Our trace for an oracle channel: by channel number, else by title and start time.
fn find<'a>(info: &'a openreadout_core::model::FileInfo, o: &Value) -> Option<&'a TraceInfo> {
    if let Some(n) = o["channel_number"].as_u64() {
        return info
            .traces
            .iter()
            .find(|t| t.extra.get("channel_number").and_then(Value::as_u64) == Some(n));
    }
    let title = o["title"].as_str()?;
    let start = o["start_s"].as_f64()?;
    info.traces.iter().find(|t| {
        t.name.as_deref() == Some(title) && t.start_s.is_some_and(|s| (s - start).abs() < 1e-9)
    })
}

/// Compare one `.smrx` with its export-derived oracle.
pub fn compare_file(_id: &str, path: &Path, oracle: &Value) -> Result<String, String> {
    let mut ds = openreadout_ephys::Spike2Reader
        .open(path)
        .map_err(|e| format!("open failed: {e}"))?;
    let info = ds.info().map_err(|e| format!("info failed: {e}"))?;
    let mut problems = Vec::new();
    let (mut exact, mut sampled, mut meta, mut events) = (0usize, 0usize, 0usize, 0usize);
    let empty = Vec::new();
    for o in oracle["traces"].as_array().unwrap_or(&empty) {
        let label = o["title"].as_str().unwrap_or("?").to_string();
        let Some(t) = find(&info, o) else {
            problems.push(format!("no trace for exported channel {label:?}"));
            continue;
        };
        let t = t.clone();
        let n = o["sample_count"].as_u64().unwrap_or(0);
        if t.sample_count != n || t.sweep_count != 1 {
            problems.push(format!(
                "{label}: {} samples in {} sweeps, the export has {n}",
                t.sample_count, t.sweep_count
            ));
            continue;
        }
        let start = o["start_s"].as_f64().unwrap_or(f64::NAN);
        if !t.start_s.is_some_and(|s| (s - start).abs() < 1e-9) {
            problems.push(format!(
                "{label}: start {:?} s, the export {start}",
                t.start_s
            ));
        }
        let interval = o["interval_s"].as_f64().unwrap_or(f64::NAN);
        if !close(1.0 / t.sample_rate_hz, interval, 1e-9) {
            problems.push(format!(
                "{label}: interval {} s, the export {interval}",
                1.0 / t.sample_rate_hz
            ));
        }
        if let Some(u) = o["unit"].as_str().filter(|u| !u.is_empty())
            && t.channels[0].unit.as_deref() != Some(u)
        {
            problems.push(format!(
                "{label}: unit {:?}, the export {u:?}",
                t.channels[0].unit
            ));
        }
        if let Some(g) = o["gain"].as_f64()
            && !close(t.channels[0].scale, g, 1e-12)
        {
            problems.push(format!(
                "{label}: scale {}, the export {g}",
                t.channels[0].scale
            ));
        }
        meta += 1;
        if let Some(want) = o["codes_xxh3"].as_str() {
            let v = read_all(ds.as_mut(), &t)?;
            let (g, off) = (t.channels[0].scale, t.channels[0].offset);
            let codes: Vec<i64> = v.iter().map(|x| ((x - off) / g).round() as i64).collect();
            let first: Vec<i64> = o["codes_first"]
                .as_array()
                .unwrap_or(&empty)
                .iter()
                .filter_map(Value::as_i64)
                .collect();
            if codes.len() as u64 != n || codes[..first.len().min(codes.len())] != first[..] {
                problems.push(format!(
                    "{label}: first codes {:?}, the export {first:?}",
                    &codes[..first.len().min(codes.len())]
                ));
            } else if h64(&codes) == want {
                exact += 1;
            } else {
                problems.push(format!("{label}: int16 codes differ from the export's"));
            }
        } else if let Some(s) = o["samples"].as_object() {
            let v = read_all(ds.as_mut(), &t)?;
            let tol = s["tolerance"].as_f64().unwrap_or(1.5e-5);
            let idx = s["index"].as_array().unwrap_or(&empty);
            let val = s["value"].as_array().unwrap_or(&empty);
            let bad = idx
                .iter()
                .zip(val)
                .filter(|(i, w)| {
                    let i = i.as_u64().unwrap_or(u64::MAX) as usize;
                    let w = w.as_f64().unwrap_or(f64::NAN);
                    v.get(i)
                        .is_none_or(|x| (x - w).abs().is_nan() || (x - w).abs() > tol)
                })
                .count();
            if bad == 0 {
                sampled += 1;
            } else {
                problems.push(format!(
                    "{label}: {bad} of {} sampled values differ from the export by more than {tol}",
                    idx.len()
                ));
            }
        }
    }
    let ev = oracle["events"].as_array().unwrap_or(&empty);
    if !ev.is_empty() {
        let table = info
            .tables
            .iter()
            .find(|t| t.name.as_deref() == Some("events"));
        let rows = match table {
            Some(t) => {
                ds.read_table(t.index, 0, t.row_count)
                    .map_err(|e| format!("events table: {e}"))?
                    .columns
            }
            None => Vec::new(),
        };
        let chans: Vec<Value> = table
            .and_then(|t| t.extra.get("channels"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for o in ev {
            let label = o["title"].as_str().unwrap_or("?");
            let number = o["channel_number"].as_u64().or_else(|| {
                chans
                    .iter()
                    .find(|c| c["title"].as_str() == Some(label))
                    .and_then(|c| c["channel_number"].as_u64())
            });
            let want_n = o["count"].as_u64().unwrap_or(0);
            let Some(number) = number else {
                if want_n > 0 {
                    problems.push(format!("no event channel {label:?}"));
                }
                continue;
            };
            let pick: Vec<usize> = rows
                .get(2)
                .map(|c| (0..c.len()).filter(|&i| c[i] == number as f64).collect())
                .unwrap_or_default();
            if pick.len() as u64 != want_n {
                problems.push(format!(
                    "{label}: {} events, the export {want_n}",
                    pick.len()
                ));
                continue;
            }
            if let Some(want) = o["ticks_xxh3"].as_str() {
                let tick_s = o["tick_s"].as_f64().unwrap_or(f64::NAN);
                let ticks: Vec<i64> = pick
                    .iter()
                    .map(|&i| (rows[0][i] / tick_s).round() as i64)
                    .collect();
                if h64(&ticks) == want {
                    events += 1;
                } else {
                    problems.push(format!("{label}: event times differ from the export's"));
                }
            } else if let Some(want) = o["bins_xxh3"].as_str() {
                let bin = o["bin_s"].as_f64().unwrap_or(f64::NAN);
                let bins: Vec<i64> = pick
                    .iter()
                    .map(|&i| (rows[0][i] / bin + 1e-9).floor() as i64)
                    .collect();
                if h64(&bins) == want {
                    events += 1;
                } else {
                    problems.push(format!(
                        "{label}: event sample bins differ from the export's"
                    ));
                }
            }
        }
    }
    if problems.is_empty() {
        Ok(format!(
            "{} ({meta} waveform channels: {exact} all int16 codes equal, {sampled} sampled values within the export's rounding; {events} event channels equal)",
            oracle["export"].as_str().unwrap_or("export")
        ))
    } else {
        Err(problems.join("; "))
    }
}
