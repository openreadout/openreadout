//! Golden-output snapshots of every development-corpus input on disk: an unintended change in
//! any reader's `info` (anywhere in it: the record keeps an xxh3-128 of the whole output),
//! `check --headers-only` verdict or assurance on any corpus file fails here and is reviewed
//! before it is accepted (docs/maintaining.md § Regression snapshots).
//!
//! Committed records: `corpus/snapshots/<format>.jsonl` (one line per input, by manifest format,
//! so a fix to one reader touches one file). Inputs not on disk are skipped; held-out files are
//! never read. On a difference the test writes the fresh records to
//! `target/tmp/snapshots/corpus/<format>.jsonl` and the full `info` of each changed input to
//! `target/tmp/snapshots/corpus/new/<id>.json` (unchanged inputs leave their full output in
//! `.../baseline/<id>.json` once, which `cargo xtask snapshot review` diffs against), then fails.
//! `cargo xtask snapshot accept` (or `SNAPSHOT_ACCEPT=1`) takes the new records.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test snapshots`
//! (`CORPUS_FORMAT=czi,nd2` to limit it).
#![cfg(feature = "corpus")]

#[path = "support/registry.rs"]
mod registry;
#[path = "../../openreadout-cli/tests/support/snapshot.rs"]
mod snapshot;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct Manifest {
    file: Vec<Entry>,
}

#[derive(Deserialize, Clone)]
struct Entry {
    id: String,
    format: String,
    #[serde(default)]
    tier: String,
    #[serde(default)]
    role: String,
    filename: String,
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// The development inputs: what the corpus test compares (never a held-out entry).
fn development(e: &Entry) -> bool {
    e.tier != "heldout"
        && e.role != "heldout"
        && (e.role.is_empty()
            || e.role == "input"
            || (e.role == "oracle-export" && (e.format == "mzml" || e.format == "mzxml")))
}

/// The record of one input, and its normalized full `info` (for review).
fn observe(path: &Path, id: &str, prefixes: &[String]) -> (Value, Option<Value>) {
    let reg = registry::registry();
    let opened = reg.detect(path).and_then(|(r, det)| {
        let ds = r.open(path)?;
        Ok((det.format_id, ds))
    });
    let (format, mut ds) = match opened {
        Ok(x) => x,
        Err(e) => {
            let code = e.code();
            return (
                snapshot::record(id, None, Err(code), Err(code), prefixes),
                None,
            );
        }
    };
    let info = ds
        .info()
        .map(|i| serde_json::to_value(openreadout_core::InfoOutput::new(ds.as_ref(), i)).unwrap());
    let check = ds.check_headers().map(|c| serde_json::to_value(c).unwrap());
    let rec = snapshot::record(
        id,
        Some(format),
        info.as_ref().map_err(openreadout_core::Error::code),
        check.as_ref().map_err(openreadout_core::Error::code),
        prefixes,
    );
    let full = info.ok().map(|i| snapshot::normalized(&i, prefixes));
    (rec, full)
}

#[test]
fn corpus_outputs_match_their_snapshots() {
    let root = root();
    let manifest: Manifest =
        toml::from_str(&std::fs::read_to_string(root.join("corpus/manifest.toml")).unwrap())
            .unwrap();
    let files = std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root.join("corpus/files"), PathBuf::from);
    let formats = std::env::var("CORPUS_FORMAT").ok();
    let inputs: Vec<Entry> = manifest
        .file
        .iter()
        .filter(|e| development(e))
        .filter(|e| {
            formats
                .as_ref()
                .is_none_or(|f| f.split(',').any(|x| x.trim() == e.format))
        })
        .filter(|e| files.join(&e.filename).exists())
        .cloned()
        .collect();
    if inputs.is_empty() {
        println!("no development-corpus input on disk; nothing to compare");
        return;
    }
    let prefixes = snapshot::prefixes(&[&files, &root]);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap();
    // Each input is read twice, from two separate opens: a reader whose output depends on a
    // hash map's iteration order (a fresh random order per map) differs between the two.
    let observed: Vec<(Entry, Value, Option<Value>, bool)> = pool.install(|| {
        inputs
            .par_iter()
            .map(|e| {
                let path = files.join(&e.filename);
                let (rec, full) = observe(&path, &e.id, &prefixes);
                let (again, _) = observe(&path, &e.id, &prefixes);
                let stable = again == rec;
                (e.clone(), rec, full, stable)
            })
            .collect()
    });
    let unstable: Vec<String> = observed
        .iter()
        .filter(|o| !o.3)
        .map(|o| format!("[{}] {}: output differs between two opens of the same file (non-deterministic reader)", o.0.format, o.0.id))
        .collect();
    let out = snapshot::out_dir("corpus");
    let mut by_format: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();
    let mut full: BTreeMap<String, Value> = BTreeMap::new();
    for (e, rec, f, _) in observed {
        by_format
            .entry(e.format.clone())
            .or_default()
            .insert(e.id.clone(), rec);
        if let Some(f) = f {
            full.insert(e.id.clone(), f);
        }
    }
    let accept = std::env::var("SNAPSHOT_ACCEPT").is_ok_and(|v| v == "1");
    let mut problems = unstable;
    for (format, fresh) in &by_format {
        let committed = root.join(format!("corpus/snapshots/{format}.jsonl"));
        let committed_records = std::fs::read_to_string(&committed)
            .map(|t| snapshot::parse(&t))
            .unwrap_or_default();
        let p = snapshot::compare(&committed, fresh, &out, format, false);
        // full outputs: the changed ones for review, a baseline of the unchanged ones
        for (id, rec) in fresh {
            let Some(f) = full.get(id) else { continue };
            let text = serde_json::to_string_pretty(f).unwrap();
            if accept || committed_records.get(id) == Some(rec) {
                let b = out.join("baseline").join(format!("{id}.json"));
                if accept || !b.exists() {
                    let _ = std::fs::create_dir_all(b.parent().unwrap());
                    let _ = std::fs::write(b, text);
                }
                // a full output left from an earlier failing run is stale now
                let _ = std::fs::remove_file(out.join("new").join(format!("{id}.json")));
            } else {
                let n = out.join("new").join(format!("{id}.json"));
                let _ = std::fs::create_dir_all(n.parent().unwrap());
                let _ = std::fs::write(n, text);
            }
        }
        problems.extend(p.into_iter().map(|m| format!("[{format}] {m}")));
    }
    let n: usize = by_format.values().map(BTreeMap::len).sum();
    println!(
        "snapshots: {n} development inputs compared, {} differ",
        problems.len()
    );
    assert!(
        problems.is_empty(),
        "{} corpus outputs differ from corpus/snapshots/. If intended, review and accept: \
         `cargo xtask snapshot review` then `cargo xtask snapshot accept` (or SNAPSHOT_ACCEPT=1).\n  {}",
        problems.len(),
        problems.join("\n  ")
    );
}
