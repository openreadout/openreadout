//! the plate-reader assays of `openreadout analyze` against independent ground truth: every case in
//! `corpus/oracle/assay/*.json` (written by `oracle/assay.py` with SciPy, R `drc` and
//! `growthcurver` run as black boxes, and the curve fits and concentrations Gen5 and SkanIt stored
//! in their exports). Synthetic cases read the committed fixtures in `tests/fixtures`; corpus
//! cases run when the corpus file is present (`OPENREADOUT_CORPUS_DIR`, default
//! `corpus/files`) and are skipped otherwise.
//!
//! Each check names a JSON path in the `AssayOutput` (`compounds[compound=CPD-A].ec50`), the
//! expected value, its source and a tolerance: pass when |ours − expected| ≤ max(abs, rel ×
//! |expected|). Run with `-- --nocapture` for the per-case agreement report.
#![allow(clippy::many_single_char_names, clippy::float_cmp)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_assay::{AssayRequest, analyze_file};
use openreadout_core::Registry;
use serde_json::Value;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn corpus_file(id: &str) -> Option<PathBuf> {
    let manifest = std::fs::read_to_string(root().join("corpus/manifest.toml")).ok()?;
    let at = manifest.find(&format!("\nid = \"{id}\"\n"))?;
    let rest = &manifest[at..];
    let end = rest[1..].find("[[file]]").map_or(rest.len(), |e| e + 1);
    let line = rest[..end].lines().find(|l| l.starts_with("filename = "))?;
    let name = line.trim_start_matches("filename = ").trim_matches('"');
    let dir = std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root().join("corpus/files"), PathBuf::from);
    Some(dir.join(name))
}

fn fixture(name: &str) -> PathBuf {
    // from the repository root, so the corpus harness can include this file as a module
    root()
        .join("crates/openreadout-assay/tests/fixtures")
        .join(name)
}

/// Follow `a.b[key=value].c` through a JSON value.
fn lookup<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = v;
    for token in path.split('.') {
        let (name, sel) = match token.split_once('[') {
            Some((n, s)) => (n, Some(s.trim_end_matches(']'))),
            None => (token, None),
        };
        cur = cur.get(name)?;
        if let Some(sel) = sel {
            let (k, want) = sel.split_once('=')?;
            cur = cur.as_array()?.iter().find(|item| {
                item.get(k).is_some_and(|x| match x {
                    Value::String(s) => s == want,
                    other => other
                        .as_f64()
                        .is_some_and(|n| want.parse::<f64>().is_ok_and(|w| w == n)),
                })
            })?;
        }
    }
    Some(cur)
}

struct Outcome {
    passed: usize,
    failed: Vec<String>,
    /// source → (checks, largest relative difference)
    by_source: BTreeMap<String, (usize, f64)>,
}

fn run_case(reg: &Registry, doc: &Value) -> Option<Outcome> {
    let name = doc["case"].as_str().unwrap_or("?");
    let input = &doc["input"];
    let path = if let Some(f) = input.get("fixture").and_then(Value::as_str) {
        fixture(f)
    } else {
        corpus_file(input["corpus"].as_str()?)?
    };
    if !path.exists() {
        println!("skip   {name}: {} not present", path.display());
        return None;
    }
    let mut req_json = doc["request"].clone();
    if let Some(l) = req_json.get("layout").and_then(Value::as_str)
        && let Some(f) = l.strip_prefix("fixture:")
    {
        req_json["layout"] = Value::String(fixture(f).display().to_string());
    }
    let req: AssayRequest = serde_json::from_value(req_json).expect("request JSON");
    let out = analyze_file(reg, &path, &req).unwrap_or_else(|e| panic!("{name}: {e}"));
    let got = serde_json::to_value(&out).expect("serialize");
    let mut o = Outcome {
        passed: 0,
        failed: Vec::new(),
        by_source: BTreeMap::new(),
    };
    for c in doc["checks"].as_array().expect("checks") {
        let p = c["path"].as_str().expect("path");
        let src = c["source"].as_str().unwrap_or("").to_string();
        let ours = lookup(&got, p);
        let exp = &c["expect"];
        let (ok, rel_err) = match exp {
            Value::Number(n) => {
                let e = n.as_f64().unwrap_or(f64::NAN);
                match ours.and_then(Value::as_f64) {
                    Some(v) => {
                        let tol = c["abs"]
                            .as_f64()
                            .unwrap_or(0.0)
                            .max(c["rel"].as_f64().unwrap_or(0.0) * e.abs());
                        let d = (v - e).abs();
                        (d <= tol, if e == 0.0 { d } else { d / e.abs() })
                    }
                    None => (false, f64::INFINITY),
                }
            }
            Value::String(s) => (ours.and_then(Value::as_str) == Some(s.as_str()), 0.0),
            Value::Array(any) => (
                ours.and_then(Value::as_str)
                    .is_some_and(|v| any.iter().any(|a| a.as_str() == Some(v))),
                0.0,
            ),
            Value::Null => (ours.is_none_or(Value::is_null), 0.0),
            other => panic!("{name}: unsupported expectation {other}"),
        };
        let e = o.by_source.entry(src.clone()).or_insert((0, 0.0));
        e.0 += 1;
        if rel_err.is_finite() {
            e.1 = e.1.max(rel_err);
        }
        if ok {
            o.passed += 1;
        } else {
            o.failed.push(format!(
                "{p}: ours {} expected {exp} ({src}; rel {:?}, abs {:?})",
                ours.map_or_else(|| "missing".into(), ToString::to_string),
                c["rel"].as_f64(),
                c["abs"].as_f64()
            ));
        }
    }
    Some(o)
}

#[test]
fn assay_matches_independent_ground_truth() {
    let dir = root().join("corpus/oracle/assay");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("corpus/oracle/assay")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no cases in {}", dir.display());
    let reg = Registry::new().with(Box::new(openreadout_plate::PlateReader));
    let mut failures = Vec::new();
    let mut ran = 0;
    for p in &paths {
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        let name = doc["case"].as_str().unwrap_or("?").to_string();
        let Some(o) = run_case(&reg, &doc) else {
            continue;
        };
        ran += 1;
        println!(
            "{:<6} {name}: {}/{} checks",
            if o.failed.is_empty() { "ok" } else { "FAIL" },
            o.passed,
            o.passed + o.failed.len()
        );
        for (src, (n, worst)) in &o.by_source {
            println!("         {n:>4} × {src}: largest relative difference {worst:.2e}");
        }
        for f in &o.failed {
            println!("         {f}");
        }
        failures.extend(o.failed.into_iter().map(|f| format!("{name}: {f}")));
    }
    assert!(
        ran >= 5,
        "only {ran} cases ran (synthetic fixtures missing?)"
    );
    assert!(
        failures.is_empty(),
        "{} checks failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
