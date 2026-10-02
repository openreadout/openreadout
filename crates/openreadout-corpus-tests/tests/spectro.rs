//! Spectroscopy corpus checks beyond the generic oracle harness (`tests/corpus/`, which compares
//! every value with the third-party readers' bit for bit):
//!
//! - the x axis of every trace against the oracle's `x_first`/`x_last` (within `x_tolerance`);
//! - per-spectrum table columns the oracle can only give approximately (`approx_columns`);
//! - every vendor text export in the manifest (`role = "oracle-export"`: OMNIC CSV, OPUS CSV and
//!   data-point table, WiRE text) against the spectrum it was exported from, point by point, to the
//!   precision the export prints;
//! - WiRE's own map analyses (`wire_map: Intensity At Point N` columns: the spectrum linearly
//!   interpolated at N, computed by WiRE) against our spectra.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test spectro -- --nocapture`
#![cfg(feature = "corpus")]
#![allow(clippy::many_single_char_names)] // x, y, m, p, e: same short names as the other corpus tests

use std::path::{Path, PathBuf};

use openreadout_core::{Dataset, Registry};
use openreadout_corpus_tests::oracle_json;
use serde::Deserialize;

const FORMATS: &[&str] = &[
    "bruker-opus",
    "thermo-omnic",
    "renishaw-wdf",
    "perkinelmer-sp",
    "galactic-spc",
];

#[derive(Deserialize)]
struct Manifest {
    file: Vec<Entry>,
}
#[derive(Deserialize)]
struct Entry {
    id: String,
    format: String,
    filename: String,
    #[serde(default)]
    role: String,
    /// `heldout` entries are for measuring only (docs/benchmark/heldout.md): never checked here.
    #[serde(default)]
    tier: String,
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn files_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root().join("corpus/files"), PathBuf::from)
}

fn registry() -> Registry {
    Registry::new()
        .with(Box::new(openreadout_spectro::OpusReader))
        .with(Box::new(openreadout_spectro::OmnicReader))
        .with(Box::new(openreadout_spectro::WdfReader))
        .with(Box::new(openreadout_spectro::PeSpReader))
        .with(Box::new(openreadout_spectro::JwsReader))
        .with(Box::new(openreadout_spectro::SpcReader))
        .with(Box::new(openreadout_spectro::WitecReader))
        .with(Box::new(openreadout_spectro::AgilentFpaReader))
        .with(Box::new(openreadout_spectro::FsmReader))
        .with(Box::new(openreadout_spectro::CaryReader))
}

fn manifest() -> Manifest {
    toml::from_str(&std::fs::read_to_string(root().join("corpus/manifest.toml")).unwrap()).unwrap()
}

/// x values and y values of sweep `sweep` of trace `t`.
fn spectrum(ds: &mut dyn Dataset, t: u32, sweep: u32) -> (Vec<f64>, Vec<f64>) {
    let info = ds.info().unwrap();
    let ti = &info.traces[t as usize];
    let tr = ds.read_trace(t, sweep, 0, u64::MAX).unwrap();
    let axis = &ti.extra["axis"];
    if axis.get("irregular").and_then(serde_json::Value::as_bool) == Some(true) {
        return (tr.channels[0].clone(), tr.channels[1].clone());
    }
    let first = axis["first"].as_f64().unwrap();
    let step = axis["step"].as_f64().unwrap();
    let y = tr.channels[0].clone();
    ((0..y.len()).map(|i| first + step * i as f64).collect(), y)
}

/// A number as printed and half a unit of its last printed digit (`5.197873e-002`, `0.90603`,
/// `445952.031250`).
fn parse_token(tok: &str) -> Option<(f64, f64)> {
    let v: f64 = tok.parse().ok()?;
    let (mant, exp) = match tok.find(['e', 'E']) {
        Some(i) => (&tok[..i], tok[i + 1..].parse::<i32>().ok()?),
        None => (tok, 0),
    };
    let decimals = mant.split_once('.').map_or(0, |(_, f)| f.len()) as i32;
    Some((v, 0.5 * 10f64.powi(exp - decimals)))
}

/// Rows `(x, y, y_half_ulp)` of a two-column text export (comma, tab or space separated;
/// lines starting with `#` skipped).
fn read_export(p: &Path) -> Vec<(f64, f64, f64)> {
    let text = String::from_utf8_lossy(&std::fs::read(p).unwrap()).to_string();
    text.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| {
            let mut it = l.split([',', '\t', ' ', ';']).filter(|s| !s.is_empty());
            let (x, _) = parse_token(it.next()?)?;
            let (y, h) = parse_token(it.next()?)?;
            Some((x, y, h))
        })
        .collect()
}

#[test]
fn vendor_exports_match() {
    let m = manifest();
    let dir = files_dir();
    let reg = registry();
    let mut checked = 0;
    let mut failures = Vec::new();
    for e in m.file.iter().filter(|e| {
        e.role == "oracle-export" && e.tier != "heldout" && FORMATS.contains(&e.format.as_str())
    }) {
        let input_id = ["-csv", "-txt", "-dpt"]
            .iter()
            .find_map(|s| e.id.strip_suffix(s))
            .unwrap_or(&e.id);
        let Some(input) = m
            .file
            .iter()
            .find(|i| i.id == input_id && i.role == "input")
        else {
            failures.push(format!("{}: no input {input_id}", e.id));
            continue;
        };
        let (ep, ip) = (dir.join(&e.filename), dir.join(&input.filename));
        if !ep.exists() || !ip.exists() {
            continue;
        }
        let rows = read_export(&ep);
        let (_, mut ds) = reg.open(&ip).unwrap();
        let (x, y) = spectrum(ds.as_mut(), 0, 0);
        let step = (x[x.len() - 1] - x[0]).abs() / (x.len() - 1) as f64;
        let (mut matched, mut worst) = (0usize, 0f64);
        let mut bad = 0usize;
        for (ex, ey, half) in &rows {
            // nearest point of ours
            let i = match x.binary_search_by(|v| {
                if x[0] <= x[x.len() - 1] {
                    v.total_cmp(ex)
                } else {
                    ex.total_cmp(v)
                }
            }) {
                Ok(i) => i,
                Err(i) => {
                    let cand = [i.saturating_sub(1), i.min(x.len() - 1)];
                    *cand
                        .iter()
                        .min_by(|a, b| (x[**a] - ex).abs().total_cmp(&(x[**b] - ex).abs()))
                        .unwrap()
                }
            };
            if (x[i] - ex).abs() > 0.02 * step + 1e-6 * ex.abs() {
                continue; // outside our range (OMNIC CSVs add one zero row below it)
            }
            matched += 1;
            // the export rounds our value to its printed precision
            let d = (y[i] - ey).abs();
            worst = worst.max(d / half.max(f64::MIN_POSITIVE));
            // a tie rounds either way; allow the decimal parse error of large values
            if d > half * 1.000_001 + 8.0 * f64::EPSILON * ey.abs().max(y[i].abs()) {
                bad += 1;
            }
        }
        checked += 1;
        let min_match = if rows.len() > 2 {
            rows.len() - 2
        } else {
            rows.len()
        };
        let status = if bad == 0 && matched >= min_match.min(x.len()) {
            "pass"
        } else {
            "FAIL"
        };
        println!(
            "{status}  {:<55} {matched} of {} export rows matched to {} points, worst |diff| = {worst:.3} × half the printed digit",
            e.id,
            rows.len(),
            x.len()
        );
        if status == "FAIL" {
            failures.push(format!("{}: {bad} values off, {matched} matched", e.id));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
    if checked == 0 {
        eprintln!(
            "no spectroscopy exports present; run `cargo xtask corpus fetch --format thermo-omnic`"
        );
    }
}

#[derive(Deserialize)]
struct Oracle {
    #[serde(default)]
    traces: Vec<OracleTrace>,
    #[serde(default)]
    tables: Vec<OracleTable>,
}
#[derive(Deserialize)]
struct OracleTrace {
    index: u32,
    #[serde(default)]
    x_first: Option<f64>,
    #[serde(default)]
    x_last: Option<f64>,
    #[serde(default)]
    x_tolerance: Option<f64>,
}
#[derive(Deserialize)]
struct OracleTable {
    index: u32,
    #[serde(default)]
    approx_columns: std::collections::BTreeMap<String, Vec<f64>>,
}

#[test]
fn axes_and_approximate_columns_match() {
    let m = manifest();
    let dir = files_dir();
    let reg = registry();
    let mut failures = Vec::new();
    let mut checked = 0;
    for e in m
        .file
        .iter()
        .filter(|e| e.role == "input" && FORMATS.contains(&e.format.as_str()))
    {
        let p = dir.join(&e.filename);
        let op = root().join(format!("corpus/oracle/{}.json", e.id));
        if !p.exists() || !oracle_json::exists(&op) {
            continue;
        }
        let o: Oracle = serde_json::from_str(&oracle_json::read_to_string(&op).unwrap()).unwrap();
        let (_, mut ds) = reg.open(&p).unwrap();
        let info = ds.info().unwrap();
        for t in &o.traces {
            let Some(ti) = info.traces.iter().find(|x| x.index == t.index) else {
                failures.push(format!("{}: trace {} missing", e.id, t.index));
                continue;
            };
            let a = &ti.extra["axis"];
            for (k, want) in [("first", t.x_first), ("last", t.x_last)] {
                let Some(w) = want else { continue };
                let got = a[k].as_f64().unwrap_or(f64::NAN);
                // an oracle that rounds (x_tolerance = half its last digit) can be off by exactly
                // the tolerance: allow the float error of that half-unit on top
                let tol = t
                    .x_tolerance
                    .map_or(1e-9 * w.abs().max(1.0), |x| x + 1e-12 * w.abs().max(1.0));
                if (got - w).abs() > tol {
                    failures.push(format!(
                        "{} trace {}: x {k} {got} != oracle {w}",
                        e.id, t.index
                    ));
                }
            }
        }
        for t in &o.tables {
            if t.approx_columns.is_empty() {
                continue;
            }
            let ti = &info.tables[t.index as usize];
            let tab = ds.read_table(t.index, 0, ti.row_count).unwrap();
            for (name, want) in &t.approx_columns {
                let Some(c) = ti.columns.iter().position(|c| &c.name == name) else {
                    failures.push(format!("{}: no column {name}", e.id));
                    continue;
                };
                for (i, w) in want.iter().enumerate() {
                    if (tab.columns[c][i] - w).abs() > 1e-5 {
                        failures.push(format!("{} {name}[{i}] {} != {w}", e.id, tab.columns[c][i]));
                        break;
                    }
                }
            }
        }
        checked += 1;
    }
    println!("{checked} spectroscopy files: axes and approximate columns compared");
    assert!(failures.is_empty(), "{failures:#?}");
}

/// WiRE's "Intensity At Point N" map analyses are the spectra linearly interpolated at N.
#[test]
fn wire_map_analyses_match() {
    let m = manifest();
    let dir = files_dir();
    let reg = registry();
    let mut checked = 0;
    for e in m
        .file
        .iter()
        .filter(|e| e.role == "input" && e.format == "renishaw-wdf")
    {
        let p = dir.join(&e.filename);
        if !p.exists() {
            continue;
        }
        let (_, mut ds) = reg.open(&p).unwrap();
        let info = ds.info().unwrap();
        let Some(ti) = info.tables.first() else {
            continue;
        };
        let cols: Vec<(usize, f64)> = ti
            .columns
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let n = c.name.strip_prefix("wire_map: Intensity At Point ")?;
                (!n.contains('/')).then(|| n.trim().parse().ok().map(|v| (i, v)))?
            })
            .collect();
        if cols.is_empty() {
            continue;
        }
        let tab = ds.read_table(0, 0, ti.row_count).unwrap();
        let mut worst = 0f64;
        for k in 0..info.traces[0].sweep_count {
            let (x, y) = spectrum(ds.as_mut(), 0, k);
            for (c, at) in &cols {
                // linear interpolation at `at` on the (descending or ascending) x list
                let i = (0..x.len() - 1)
                    .find(|&i| (x[i] - at) * (x[i + 1] - at) <= 0.0)
                    .unwrap();
                let f = (at - x[i]) / (x[i + 1] - x[i]);
                let v = y[i] + f * (y[i + 1] - y[i]);
                let want = tab.columns[*c][k as usize];
                worst = worst.max((v - want).abs() / want.abs().max(1e-12));
            }
        }
        println!(
            "pass  {:<40} {} spectra × {} WiRE 'Intensity At Point' analyses, worst relative difference {worst:.2e}",
            e.id,
            info.traces[0].sweep_count,
            cols.len()
        );
        assert!(worst < 1e-6, "{}: worst {worst}", e.id);
        checked += 1;
    }
    println!("{checked} WiRE files with map analyses");
}
