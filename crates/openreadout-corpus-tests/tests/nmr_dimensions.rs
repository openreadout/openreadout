//! The indirect-dimension metadata of the NMR readers against nmrglue (`oracle/gen.py`):
//!
//! - Bruker: each `acquNs` file's parameters as nmrglue read them (`dimensions` of the
//!   time-domain trace) against `extra.indirect_dimensions[]`, and each `procs`/`procNs` file
//!   (`dimensions` of a processed trace) against `extra.processing` and
//!   `extra.indirect_dimensions[]`;
//! - Varian/Agilent: nmrglue's `find_shape` and `find_torder` against `extra.sweep_order` (the
//!   loops multiply to the shape's traces, and the phase loops are in nmrglue's order).
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test
//! nmr_dimensions -- --nocapture` (`OPENREADOUT_CORPUS_DIR` overrides `corpus/files`).
#![cfg(feature = "corpus")]

use std::path::{Path, PathBuf};

use openreadout_corpus_tests::oracle_json;
use serde::Deserialize;
use serde_json::Value;

#[path = "support/registry.rs"]
mod registry;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn corpus_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR").map_or_else(|| root().join("corpus/files"), PathBuf::from)
}

#[derive(Deserialize)]
struct Manifest {
    file: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    id: String,
    format: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    tier: String,
    filename: String,
}

/// Two parameter values agree: equal numbers (to 1e-9 relative), else equal text with `<>`
/// brackets removed.
fn same(ours: Option<&Value>, oracle: &Value) -> bool {
    let Some(o) = ours else { return false };
    match (o.as_f64(), oracle.as_f64()) {
        (Some(a), Some(b)) => (a - b).abs() <= 1e-9 * b.abs().max(1.0),
        _ => {
            let t = |v: &Value| {
                v.as_str()
                    .map_or_else(|| v.to_string(), str::to_string)
                    .trim()
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .to_string()
            };
            t(o) == t(oracle)
        }
    }
}

/// Compare one parameter file's values with our object, `(parameter, our key)` pairs.
fn compare(what: &str, ours: &Value, oracle: &Value, map: &[(&str, &str)], problems: &mut Vec<String>) -> usize {
    let mut n = 0;
    for (param, key) in map {
        if let Some(v) = oracle.get(*param) {
            n += 1;
            if !same(ours.get(*key), v) {
                problems.push(format!("{what}: {key} {:?} != nmrglue {param} {v}", ours.get(*key)));
            }
        }
    }
    n
}

const ACQU: &[(&str, &str)] = &[
    ("TD", "time_domain_size"),
    ("NUC1", "nucleus"),
    ("SW_h", "spectral_width_hz"),
    ("SW", "spectral_width_ppm"),
    ("SFO1", "spectrometer_frequency_mhz"),
    ("BF1", "base_frequency_mhz"),
    ("O1", "carrier_offset_hz"),
    ("FnMODE", "encoding_code"),
];

const PROC: &[(&str, &str)] = &[
    ("SI", "size"),
    ("XDIM", "submatrix"),
    ("AXNUC", "nucleus"),
    ("SW_p", "spectral_width_hz"),
    ("SF", "spectrometer_frequency_mhz"),
    ("OFFSET", "offset_ppm"),
    ("WDW", "window_function_code"),
    ("LB", "line_broadening_hz"),
    ("SSB", "sine_bell_shift"),
    ("PHC0", "phase0_deg"),
    ("PHC1", "phase1_deg"),
    ("FT_mod", "transform_mode_code"),
    ("TDeff", "effective_time_domain_size"),
    ("ME_mod", "linear_prediction_mode_code"),
    ("NCOEF", "linear_prediction_coefficients"),
];

#[test]
fn indirect_dimensions_match_nmrglue() {
    let root = root();
    let manifest: Manifest = toml::from_str(&std::fs::read_to_string(root.join("corpus/manifest.toml")).unwrap()).unwrap();
    let reg = registry::registry();
    let (mut files, mut checked) = (0usize, 0usize);
    let mut problems = Vec::new();
    for e in manifest.file.iter().filter(|e| {
        e.role == "input" && e.tier != "heldout" && matches!(e.format.as_str(), "bruker-nmr" | "varian-nmr")
    }) {
        let Ok(text) = oracle_json::read_to_string(&root.join("corpus/oracle").join(format!("{}.json", e.id))) else {
            continue;
        };
        let oracle: Value = serde_json::from_str(&text).unwrap();
        let traces = oracle["traces"].as_array().cloned().unwrap_or_default();
        let wanted = traces
            .iter()
            .any(|t| t.get("dimensions").is_some() || t.get("nmrglue_find_shape").is_some());
        let path = corpus_dir().join(&e.filename);
        if !wanted || !path.exists() {
            continue;
        }
        let (_, ds) = match reg.open(&path) {
            Ok(x) => x,
            Err(err) => {
                problems.push(format!("{}: open failed: {err}", e.id));
                continue;
            }
        };
        let info = ds.info().unwrap();
        files += 1;
        for o in &traces {
            let Some(index) = o["index"].as_u64() else { continue };
            let Some(t) = info.traces.iter().find(|t| u64::from(t.index) == index) else {
                problems.push(format!("{}: trace {index} missing", e.id));
                continue;
            };
            let ours = |k: &str| t.extra.get(k).cloned().unwrap_or(Value::Null);
            let what = |k: usize| format!("{} trace {index} dimension {}", e.id, k + 2);
            if let Some(dims) = o.get("dimensions").and_then(Value::as_array) {
                if t.extra.get("kind").and_then(Value::as_str) == Some("time_domain") {
                    let mine = ours("indirect_dimensions");
                    for (k, d) in dims.iter().enumerate() {
                        checked += compare(&what(k), &mine[k], d, ACQU, &mut problems);
                    }
                } else {
                    for (k, d) in dims.iter().enumerate() {
                        let mine = if k == 0 {
                            ours("processing")
                        } else {
                            ours("indirect_dimensions")[k - 1].clone()
                        };
                        let mut map = PROC.to_vec();
                        if k > 0 {
                            map.push(("MC2", "quadrature_mode_code"));
                        }
                        checked += compare(&format!("{} trace {index} dimension {}", e.id, k + 1), &mine, d, &map, &mut problems);
                    }
                }
            }
            if let Some(shape) = o.get("nmrglue_find_shape").and_then(Value::as_array) {
                let order = ours("sweep_order");
                let loops = order["loops"].as_array().cloned().unwrap_or_default();
                let product: u64 = loops.iter().filter_map(|l| l["size"].as_u64()).product();
                let traces_in_shape: u64 = shape[..shape.len().saturating_sub(1)]
                    .iter()
                    .filter_map(Value::as_u64)
                    .product();
                checked += 1;
                if product != traces_in_shape || order["matches_sweeps"] != Value::Bool(true) {
                    problems.push(format!(
                        "{}: sweep_order loops multiply to {product}, nmrglue find_shape {shape:?} to {traces_in_shape}",
                        e.id
                    ));
                }
                // nmrglue's torder: 'r' when the F1 phase changes faster than the F2 phase
                let pos = |p: &str| loops.iter().position(|l| l["parameter"] == p);
                if let (Some(torder), Some(a), Some(b)) =
                    (o["nmrglue_torder"].as_str(), pos("phase"), pos("phase2"))
                {
                    checked += 1;
                    let ours_r = a > b;
                    if (torder == "r") != ours_r {
                        problems.push(format!(
                            "{}: phase loops in order phase@{a}, phase2@{b}; nmrglue torder {torder}",
                            e.id
                        ));
                    }
                }
            }
        }
    }
    println!("nmr dimensions: {files} data sets, {checked} values compared with nmrglue");
    assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
}
