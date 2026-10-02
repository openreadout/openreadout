//! Assurance evidence stays true (docs/assurance.md): every development file recorded in
//! `corpus/assurance/evidence.json` that is on disk still yields exactly the recorded variant
//! features, and every reader in the evidence has a profile whose generated table knows each
//! of them. A reworded note or a changed extra that silently drops a feature fails here.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test assurance`.
#![cfg(feature = "corpus")]

#[path = "support/registry.rs"]
mod shared_registry;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::Registry;
use openreadout_core::assurance::FeatureStatus;
use serde::Deserialize;

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
}

#[derive(Deserialize)]
struct Evidence {
    files: Vec<FileEvidence>,
}
#[derive(Deserialize)]
struct FileEvidence {
    id: String,
    format: String,
    oracle: String,
    features: Vec<(String, String, Vec<String>)>,
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// Every reader, in the CLI's detection order (`support/registry.rs`, kept equal to
/// `crates/openreadout-cli/src/registry.rs` by `openreadout`'s `tests/registry_parity.rs`).
fn registry() -> Registry {
    shared_registry::registry()
}

#[test]
fn assurance_evidence_is_current() {
    let root = root();
    let manifest: Manifest =
        toml::from_str(&std::fs::read_to_string(root.join("corpus/manifest.toml")).unwrap())
            .unwrap();
    let evidence: Evidence = serde_json::from_str(
        &std::fs::read_to_string(root.join("corpus/assurance/evidence.json")).unwrap(),
    )
    .unwrap();
    let files_dir = std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root.join("corpus/files"), PathBuf::from);
    // development inputs only: an oracle export may share its input's id and format
    let paths: BTreeMap<(&str, &str), &str> = manifest
        .file
        .iter()
        // Evidence is per input; a vendor export can share its input's id and format id.
        .filter(|e| e.role.is_empty() || e.role == "input")
        .map(|e| ((e.id.as_str(), e.format.as_str()), e.filename.as_str()))
        .collect();
    let by_id: BTreeMap<&str, Vec<&str>> =
        manifest.file.iter().fold(BTreeMap::new(), |mut m, e| {
            m.entry(e.id.as_str())
                .or_default()
                .push(e.filename.as_str());
            m
        });
    let reg = registry();
    let (mut checked, mut problems) = (0usize, Vec::new());
    for f in evidence.files.iter().filter(|f| f.oracle != "unreadable") {
        // The evidence records the reader's format id; the manifest entry may name another
        // (a depositor mzML export shares its raw file's id).
        let filename = paths
            .get(&(f.id.as_str(), f.format.as_str()))
            .copied()
            .or_else(|| {
                by_id.get(f.id.as_str()).and_then(|names| {
                    names.iter().copied().find(|n| {
                        reg.detect(&files_dir.join(n))
                            .is_ok_and(|(_, d)| d.format_id == f.format)
                    })
                })
            });
        let Some(filename) = filename else {
            continue;
        };
        let path = files_dir.join(filename);
        if !path.exists() {
            continue;
        }
        let (_, ds) = match reg.open(&path) {
            Ok(x) => x,
            Err(e) => {
                problems.push(format!("{}: open failed: {e}", f.id));
                continue;
            }
        };
        let info = match ds.info() {
            Ok(i) => i,
            Err(e) => {
                problems.push(format!("{}: info failed: {e}", f.id));
                continue;
            }
        };
        let a = openreadout_core::assurance::assess_dataset(ds.as_ref(), &info);
        let mut got: Vec<(String, String, Vec<String>)> = a
            .variant
            .iter()
            .map(|v| {
                (
                    v.kind.as_str().to_string(),
                    v.value.clone(),
                    v.scope.iter().map(|s| s.as_str().to_string()).collect(),
                )
            })
            .collect();
        let mut want = f.features.clone();
        got.sort();
        want.sort();
        if got != want {
            problems.push(format!(
                "{} ({}): features drifted from the evidence (rerun `cargo xtask assurance-audit refresh`)\n    evidence: {want:?}\n    now:      {got:?}",
                f.id, f.format
            ));
        }
        // Every feature of a development file is in its reader's generated table.
        for v in &a.variant {
            if v.status == FeatureStatus::Unseen {
                problems.push(format!(
                    "{} ({}): feature {}={} is not in the generated table (run `cargo xtask assurance-audit --write`)",
                    f.id,
                    f.format,
                    v.kind.as_str(),
                    v.value
                ));
            }
        }
        checked += 1;
    }
    println!("assurance evidence: {checked} development files checked");
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}
