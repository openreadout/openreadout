//! The generic HDF5 reader (`hdf5`) against h5py (`oracle/hdf5_structure_oracle.py` →
//! `corpus/oracle/hdf5/<id>.json`): the superblock version, the group and dataset counts, and
//! every group and dataset reached from `/` with its kind, shape, type and attributes, matched by
//! path. An attribute whose oracle value is `null` (a value h5py could not summarise) is
//! compared by name only. An empty text attribute is `""` in h5py and has no value (`null`) in
//! our listing; both say the attribute holds no text. h5py reads an HDF5 FALSE/TRUE enum as a
//! bool, where the listing keeps the stored 0 or 1.
//!
//! With `HDF5_RESULTS=<file>`, each compared file is written as a results line for `cargo xtask
//! assurance-audit refresh` (docs/assurance.md): the listing is the reader's metadata.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test hdf5_structure -- --nocapture`
#![cfg(feature = "corpus")]

use std::path::{Path, PathBuf};

use openreadout_core::FormatReader;
use serde_json::Value;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn corpus_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root().join("corpus/files"), PathBuf::from)
}

/// `filename` of the development input `id` in the manifest.
fn filename(manifest: &toml::Value, id: &str) -> Option<String> {
    manifest["file"].as_array()?.iter().find_map(|f| {
        (f["id"].as_str() == Some(id) && f["role"].as_str() == Some("input"))
            .then(|| f["filename"].as_str().map(str::to_string))
            .flatten()
    })
}

fn same_value(ours: &Value, oracle: &Value) -> bool {
    match (ours, oracle) {
        (_, Value::Null) => true,
        (Value::Null, Value::String(s)) => s.is_empty(),
        // h5py turns HDF5's FALSE/TRUE enum into a bool; the listing keeps the stored 0 or 1
        (Value::Number(n), Value::Bool(b)) => n.as_u64() == Some(u64::from(*b)),
        (Value::Number(a), Value::Number(b)) => match (a.as_f64(), b.as_f64()) {
            (Some(x), Some(y)) => x == y || (x - y).abs() <= 1e-12 * x.abs().max(y.abs()),
            _ => a == b,
        },
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same_value(x, y))
        }
        _ => ours == oracle,
    }
}

#[test]
fn listings_match_h5py() {
    let dir = root().join("corpus/oracle/hdf5");
    let manifest: toml::Value =
        toml::from_str(&std::fs::read_to_string(root().join("corpus/manifest.toml")).unwrap())
            .unwrap();
    let mut results = String::new();
    let mut failures = Vec::new();
    let mut compared = 0;
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    paths.sort();
    for p in paths {
        let o: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let id = o["id"].as_str().unwrap().to_string();
        let Some(name) = filename(&manifest, &id) else {
            failures.push(format!("{id}: not a development input of the manifest"));
            continue;
        };
        let path = corpus_dir().join(&name);
        if !path.exists() {
            eprintln!("skip {id}: missing");
            continue;
        }
        let ds = openreadout_hdf5::Hdf5Reader.open(&path).unwrap();
        let info = ds.info().unwrap();
        let entries = ds.entries().unwrap();
        let mut problems = Vec::new();
        let want_version = format!("superblock version {}", o["superblock_version"]);
        if info.format_version.as_deref() != Some(want_version.as_str()) {
            problems.push(format!(
                "format version {:?} != {want_version:?}",
                info.format_version
            ));
        }
        let objects = o["objects"].as_array().unwrap();
        if entries.len() != objects.len() {
            problems.push(format!(
                "{} entries, h5py {} objects",
                entries.len(),
                objects.len()
            ));
        }
        // matched by path: the listing's order is not h5py's breadth-first order
        for x in objects {
            let at = x["path"].as_str().unwrap_or_default();
            let Some(e) = entries.iter().find(|e| e.name == at) else {
                problems.push(format!("{at}: not listed"));
                continue;
            };
            if Some(e.kind.as_str()) != x["kind"].as_str() {
                problems.push(format!("{at}: {} != {}", e.kind, x["kind"]));
                continue;
            }
            if x["kind"] == "dataset" {
                for k in ["shape", "dtype"] {
                    if e.details[k] != x[k] {
                        problems.push(format!("{at}: {k} {} != {}", e.details[k], x[k]));
                    }
                }
            }
            let ours = e.details["attributes"].as_object().cloned().unwrap_or_default();
            let theirs = x["attributes"].as_object().cloned().unwrap_or_default();
            let mut a: Vec<&String> = ours.keys().collect();
            let mut b: Vec<&String> = theirs.keys().collect();
            a.sort();
            b.sort();
            if a != b {
                problems.push(format!("{at}: attributes {a:?} != {b:?}"));
                continue;
            }
            for (k, v) in &theirs {
                if !same_value(&ours[k], v) {
                    problems.push(format!("{at} @{k}: {} != {v}", ours[k]));
                }
            }
        }
        let status = if problems.is_empty() { "pass" } else { "FAIL" };
        println!(
            "{status:<6} {id:<55} {} objects; {}",
            objects.len(),
            problems.first().map_or("", String::as_str)
        );
        results.push_str(
            &serde_json::json!({
                "id": id, "format": "hdf5", "status": status, "independent": true,
                "compared": ["metadata"],
            })
            .to_string(),
        );
        results.push('\n');
        if !problems.is_empty() {
            failures.push(format!("{id}: {}", problems.join("; ")));
        }
        compared += 1;
    }
    if let Ok(p) = std::env::var("HDF5_RESULTS") {
        std::fs::write(p, results).unwrap();
    }
    assert!(failures.is_empty(), "{failures:#?}");
    eprintln!("{compared} HDF5 listings equal h5py's");
}
