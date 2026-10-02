//! Detection over the whole development corpus, the way `openreadout info --view format -r` walks
//! it: every item the recursive batch walk yields is detected with the binary's registry (in its
//! detection order) and compared with the manifest's `format`.
//!
//! - A manifest file or folder that is an item is detected as its manifest format.
//! - A manifest input that is not an item lies inside a directory data set of its own format
//!   (a ChemStation `.D` holds `data.ms`, a Harmony measurement folder holds `Index.idx.xml`).
//! - A directory data set never swallows a manifest file of another format (an Olympus
//!   `.oif.files` folder is not an ImageXpress plate; a Bruker project folder does not hide the
//!   VnmrJ `.fid` folders next to its experiments).
//!
//! Held-out files (`role = "heldout"`, the `heldout` tier: docs/benchmark/heldout.md) are never
//! walked: the walk starts only at top-level corpus entries the development manifest names.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test detect -- --nocapture`
//! Env: `OPENREADOUT_CORPUS_DIR` overrides `corpus/files`.
#![cfg(feature = "corpus")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use openreadout_core::batch::{ExpandOptions, expand};
use serde::Deserialize;

#[path = "../../openreadout-cli/src/registry.rs"]
mod registry;

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
    #[serde(default)]
    tier: String,
    #[serde(default)]
    bundle: String,
}

impl Entry {
    fn held_out(&self) -> bool {
        self.tier == "heldout" || self.role == "heldout"
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn corpus_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root().join("corpus/files"), PathBuf::from)
}

/// Top-level corpus names the held-out set uses (its files' first path component, its
/// bundles' extraction folders).
fn held_out_names(m: &Manifest) -> BTreeSet<String> {
    let mut held: BTreeSet<String> = ["heldout".to_string()].into();
    for e in m.file.iter().filter(|e| e.held_out()) {
        held.insert(e.filename.split('/').next().unwrap_or_default().to_string());
        if e.role == "bundle" {
            held.insert(e.id.clone());
        }
        if !e.bundle.is_empty() {
            held.insert(e.bundle.clone());
        }
    }
    held
}

#[test]
fn detect_matches_manifest() {
    let dir = corpus_dir();
    if !dir.exists() {
        eprintln!("no corpus at {}; skipped", dir.display());
        return;
    }
    let manifest: Manifest =
        toml::from_str(&std::fs::read_to_string(root().join("corpus/manifest.toml")).unwrap())
            .unwrap();
    let held = held_out_names(&manifest);
    let dev: Vec<&Entry> = manifest
        .file
        .iter()
        .filter(|e| !e.held_out() && e.role != "bundle")
        .collect();
    // Walk roots: the top-level names the development manifest uses, present on disk.
    let tops: BTreeSet<String> = dev
        .iter()
        .map(|e| e.filename.split('/').next().unwrap_or_default().to_string())
        .filter(|t| !held.contains(t))
        .collect();
    let args: Vec<PathBuf> = tops
        .iter()
        .map(|t| dir.join(t))
        .filter(|p| p.exists())
        .collect();
    let reg = registry::registry();
    let mut opts = ExpandOptions::default();
    opts.recursive = true;
    let items = expand(&reg, &args, opts).items;

    // Detected format of every item (None: unknown).
    let mut detected: BTreeMap<PathBuf, Option<String>> = BTreeMap::new();
    // Items a reader claims without a format signature (a workbook any spreadsheet reader may take).
    let mut unsure: BTreeSet<PathBuf> = BTreeSet::new();
    let mut dirs: Vec<(PathBuf, String)> = Vec::new();
    for it in &items {
        if it.error.is_some() {
            continue;
        }
        let d = reg.detect(&it.path).ok();
        if d.as_ref().is_some_and(|(_, d)| {
            d.confidence != openreadout_core::model::DetectConfidence::Definite
        }) {
            unsure.insert(it.path.clone());
        }
        let f = d.map(|(_, d)| d.format_id.to_string());
        if it.path.is_dir()
            && let Some(f) = &f
        {
            dirs.push((it.path.clone(), f.clone()));
        }
        detected.insert(it.path.clone(), f);
    }
    println!(
        "{} walk items under {} top-level corpus entries, {} directory data sets",
        items.len(),
        args.len(),
        dirs.len()
    );

    let mut problems = Vec::new();
    let mut compared = 0usize;
    for e in &dev {
        let p = dir.join(&e.filename);
        if !p.exists() {
            continue;
        }
        if let Some(f) = detected.get(&p) {
            // Parts and members of multi-file sets may be detected as their container format
            // (the TIFF planes of an OIF set are TIFFs); every other file is its own data set.
            if e.role == "part" {
                continue;
            }
            compared += 1;
            // Companions (an imzML's `.ibd`), gating files and vendor text exports are not
            // data sets by themselves: unknown is right for them, another format is not.
            let may_be_unknown = matches!(
                e.role.as_str(),
                "companion" | "analysis" | "oracle-export" | "sidecar"
            );
            // A vendor export in a spreadsheet (a qPCR `.xls` Results export) may be claimed,
            // without a signature, by the plate reader that opens workbooks.
            let weak_claim = e.role == "oracle-export" && unsure.contains(&p);
            if f.as_deref() != Some(e.format.as_str())
                && !(may_be_unknown && (f.is_none() || weak_claim))
            {
                problems.push(format!(
                    "{} ({}, role {}): detected as {f:?}",
                    e.id, e.format, e.role
                ));
            }
            continue;
        }
        // Not an item: inside a directory data set, which must be of its format.
        if let Some((d, f)) = dirs.iter().find(|(d, _)| p.starts_with(d)) {
            if f != &e.format && !(e.role == "oracle-export" || e.role == "analysis") {
                problems.push(format!(
                    "{} ({}, role {}) is swallowed by {} detected as {f}",
                    e.id,
                    e.format,
                    e.role,
                    d.display()
                ));
            }
        } else if e.role == "input" {
            problems.push(format!(
                "{} ({}): not reached by the recursive walk",
                e.id, e.format
            ));
        }
    }
    println!("{compared} manifest files and folders compared");
    assert!(
        problems.is_empty(),
        "detection disagrees with the manifest:\n{}",
        problems.join("\n")
    );
}
