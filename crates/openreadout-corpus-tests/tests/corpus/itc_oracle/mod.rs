//! MicroCal `.itc` files against their ground truth (`oracle/itc_oracle.py`), for every entry
//! (development and held-out) through one code path (`tests/corpus/`):
//!
//! - Origin's raw export of the same run: the same number of samples, and time and differential
//!   power at the oracle's rows within the export's rounding (five decimals);
//! - Origin's integrated-heats table: the injection volumes (in order) and the cell concentration;
//! - values the depositor states outside the file (file names, README): cell and syringe
//!   concentrations within 2 % (the names are rounded);
//! - always: one injection row per declared injection, block starts in increasing order.
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
pub const FORMATS: &[&str] = &["microcal-itc"];

pub fn compare_file(_id: &str, path: &Path, oracle: &Value) -> Result<String, String> {
    let mut ds = openreadout_biophys::ItcReader
        .open(path)
        .map_err(|e| format!("open failed: {e}"))?;
    let info = ds.info().map_err(|e| format!("info failed: {e}"))?;
    let t = info.traces.first().ok_or("no thermogram trace")?;
    let tr = ds
        .read_trace(0, 0, 0, u64::MAX)
        .map_err(|e| format!("read failed: {e}"))?;
    let inj = ds
        .read_table(0, 0, u64::MAX)
        .map_err(|e| format!("table failed: {e}"))?;
    let exp = ds.experiment().ok_or("no experiment facts")?;
    let param = |k: &str| {
        exp.method
            .as_ref()
            .and_then(|m| m.parameters.get(k))
            .and_then(|q| q.value.as_f64())
    };
    let mut problems = Vec::new();
    let mut done = Vec::new();
    let empty = Vec::new();

    // structure
    let declared = param("injections");
    let rows = inj.columns[0].len();
    if declared.is_some_and(|n| n as usize != rows) {
        problems.push(format!("{rows} injection rows, {declared:?} declared"));
    }
    let starts: Vec<f64> = inj.columns[6]
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .collect();
    if starts.windows(2).any(|w| w[1] <= w[0]) {
        problems.push("injection blocks do not start in increasing order".into());
    }
    done.push(format!("{rows} injections"));

    if let Some(raw) = oracle["raw_export"].as_object() {
        let n = raw["n"].as_u64().unwrap_or(0);
        if n != t.sample_count {
            problems.push(format!(
                "{} samples, the raw export has {n}",
                t.sample_count
            ));
        } else {
            let rate = t.sample_rate_hz;
            let t0 = t.start_s.unwrap_or(0.0);
            let dp = &tr.channels[0];
            let mut bad = 0;
            let samples = raw["samples"].as_array().unwrap_or(&empty);
            for s in samples {
                let (Some(i), Some(time), Some(p)) = (s[0].as_u64(), s[1].as_f64(), s[2].as_f64())
                else {
                    continue;
                };
                let i = i as usize;
                let ours_t = t0 + i as f64 / rate;
                let ours_p = dp.get(i).copied().unwrap_or(f64::NAN);
                if (ours_t - time).abs() > 1e-6 || (ours_p - p).abs() > 5.000_001e-6 {
                    bad += 1;
                }
            }
            if bad > 0 {
                problems.push(format!(
                    "{bad} of {} export rows differ in time or power",
                    samples.len()
                ));
            } else {
                done.push(format!("{} export rows match (time, power)", samples.len()));
            }
        }
    }
    if let Some(h) = oracle["heats_export"].as_object() {
        let vols: Vec<f64> = h["injection_volumes_ul"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter_map(Value::as_f64)
            .collect();
        let ours = &inj.columns[1];
        if vols.len() > ours.len() || vols.iter().zip(ours).any(|(a, b)| (a - b).abs() > 1e-9) {
            problems.push(format!(
                "injection volumes {:?} vs Origin's {vols:?}",
                &ours[..ours.len().min(vols.len())]
            ));
        } else {
            done.push(format!("{} injection volumes match Origin", vols.len()));
        }
        if let Some(mt) = h["cell_concentration_mM"].as_f64() {
            match param("cell_concentration") {
                Some(c) if (c - mt).abs() <= 1e-9 => {
                    done.push("cell concentration matches Origin".into())
                }
                other => problems.push(format!("cell concentration {other:?}, Origin {mt}")),
            }
        }
    }
    if let Some(f) = oracle["depositor_facts"].as_object() {
        for (key, ours_key) in [
            ("cell_concentration_mM", "cell_concentration"),
            ("syringe_concentration_mM", "syringe_concentration"),
        ] {
            if let Some(v) = f.get(key).and_then(Value::as_f64) {
                match param(ours_key) {
                    Some(c) if (c - v).abs() <= 0.02 * v => {
                        done.push(format!("{ours_key} as the depositor states"))
                    }
                    other => problems.push(format!("{ours_key} {other:?}, depositor states {v}")),
                }
            }
        }
    }
    if problems.is_empty() {
        Ok(done.join(", "))
    } else {
        Err(problems.join("; "))
    }
}
