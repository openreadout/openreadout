//! Byte sources on real files: every corpus input up to 64 MiB whose reader reads byte sources
//! is opened from its path and from memory (its data set's files mirrored into a `MemFs` under
//! the same paths), and every file of each data set also through host callbacks with a small block
//! cache; `info`, `vendor_metadata`, `entries`, `check`, the first plane, table rows, trace
//! samples and spectrum must be identical. Readers that do not read byte sources yet must
//! answer "unsupported" (exit 6), which is counted, not failed.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test sources -- --nocapture`
//! Env: `OPENREADOUT_CORPUS_DIR` overrides `corpus/files`; `CORPUS_ONLY=<substring>` filters ids.
#![cfg(feature = "corpus")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::source::Input;
use serde::Deserialize;

#[path = "../../openreadout-cli/src/registry.rs"]
mod registry;
#[allow(dead_code)]
#[path = "../../openreadout-cli/tests/support/sources.rs"]
mod support;

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

const MAX_BYTES: u64 = 64 << 20;
const MAX_UNIT_BYTES: u64 = 160 << 20;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn size_of(p: &Path) -> u64 {
    if p.is_dir() {
        support::unit_files(p, u64::MAX).map_or(u64::MAX, |v| {
            v.iter()
                .map(|f| std::fs::metadata(f).map_or(0, |m| m.len()))
                .sum()
        })
    } else {
        std::fs::metadata(p).map_or(u64::MAX, |m| m.len())
    }
}

/// The files of the data set at `path`: a directory's tree; for a file in a subdirectory of the
/// corpus, that subdirectory's tree; for a top-level file, the top-level files and directories
/// sharing its stem (companions, VSI `_stem_` folders).
fn unit(files_dir: &Path, path: &Path) -> Option<Vec<PathBuf>> {
    if path.is_dir() {
        return support::unit_files(path, MAX_UNIT_BYTES);
    }
    let parent = path.parent()?;
    if parent != files_dir {
        let mut top = parent.to_path_buf();
        while top.parent()? != files_dir {
            top = top.parent()?.to_path_buf();
        }
        return support::unit_files(&top.join("x"), MAX_UNIT_BYTES);
    }
    let name = path.file_name()?.to_string_lossy().to_string();
    let stem = name.split('.').next()?.to_string();
    let mut out = Vec::new();
    let mut total = 0;
    for e in std::fs::read_dir(files_dir).ok()?.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        let other = n.split('.').next().unwrap_or_default();
        // Same stem, or one stem extends the other after a separator: `run (1).czi` parts,
        // `x_1.ser` with `x.emi`, VSI `_x_` folders.
        let extends = |long: &str, short: &str| {
            long.strip_prefix(short)
                .is_some_and(|rest| rest.starts_with([' ', '_', '(', '-']))
        };
        let same = other == stem
            || extends(other, &stem)
            || extends(&stem, other)
            || n == format!("_{stem}_");
        if !same || Path::new(&n).extension().is_some_and(|x| x == "ok") {
            continue;
        }
        let p = e.path();
        if p.is_dir() {
            let sub = support::unit_files(&p.join("x"), MAX_UNIT_BYTES)?;
            total += sub
                .iter()
                .map(|f| std::fs::metadata(f).map_or(0, |m| m.len()))
                .sum::<u64>();
            out.extend(sub);
        } else {
            total += e.metadata().map_or(0, |m| m.len());
            out.push(p);
        }
        if total > MAX_UNIT_BYTES {
            return None;
        }
    }
    Some(out)
}

#[test]
fn byte_sources_match_paths_on_the_corpus() {
    let files_dir = std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root().join("corpus/files"), PathBuf::from);
    let files_dir = files_dir.canonicalize().unwrap_or(files_dir);
    let manifest: Manifest =
        toml::from_str(&std::fs::read_to_string(root().join("corpus/manifest.toml")).unwrap())
            .unwrap();
    let only = std::env::var("CORPUS_ONLY").ok();
    let reg = registry::registry();
    // format → (compared, unsupported, skipped: too large or not openable)
    let mut tally: BTreeMap<String, (u32, u32, u32)> = BTreeMap::new();
    let mut failures = Vec::new();
    for e in &manifest.file {
        if !(e.role.is_empty() || e.role == "input") {
            continue;
        }
        if only.as_deref().is_some_and(|o| !e.id.contains(o)) {
            continue;
        }
        let path = files_dir.join(&e.filename);
        if !path.exists() {
            continue;
        }
        let t = tally.entry(e.format.clone()).or_default();
        if size_of(&path) > MAX_BYTES {
            t.2 += 1;
            continue;
        }
        // Inputs that do not open from their path are the other harnesses' business.
        let Ok((_, ds)) = reg.open(&path) else {
            t.2 += 1;
            continue;
        };
        drop(ds);
        let Some(files) = unit(&files_dir, &path) else {
            t.2 += 1;
            continue;
        };
        let inputs = [
            ("memory", Input::new(&path, support::mirror(&files))),
            (
                "callback",
                Input::new(&path, support::callback_mirror(&files)),
            ),
        ];
        for (how, input) in inputs {
            match support::compare(&reg, &path, &input) {
                Ok(true) => t.0 += 1,
                Ok(false) => t.1 += 1,
                Err(msg) => failures.push(format!("{} ({how}): {msg}", e.id)),
            }
        }
    }
    for (format, (ok, unsupported, skipped)) in &tally {
        println!("{format:>20}: {ok} compared, {unsupported} unsupported, {skipped} skipped");
    }
    assert!(
        failures.is_empty(),
        "{} inputs differ between their path and a byte source:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
