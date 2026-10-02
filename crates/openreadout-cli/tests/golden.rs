//! Golden-output snapshots of every committed test fixture (`crates/*/tests/fixtures`), so an
//! unintended change in any reader's `info`, `check` or assurance output is caught by
//! `cargo test`, without the corpus. The records are in `tests/snapshots/fixtures.jsonl`
//! (`tests/support/snapshot.rs` says what a record holds). On a difference the test fails with a
//! field-level summary; review it with `cargo xtask snapshot review` and take it with
//! `cargo xtask snapshot accept` (or rerun with `SNAPSHOT_ACCEPT=1`). The same records for every
//! development-corpus file are checked by `openreadout-corpus-tests/tests/snapshots.rs`.
//!
//! Unix only: Windows checkouts may convert the line endings of text fixtures.
#![cfg(unix)]

#[path = "support/snapshot.rs"]
mod snapshot;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// Every `crates/*/tests/fixtures` directory, relative to the repository root, sorted.
fn fixture_dirs(root: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(root.join("crates"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path().join("tests/fixtures"))
        .filter(|p| p.is_dir())
        .map(|p| {
            p.strip_prefix(root)
                .unwrap()
                .display()
                .to_string()
                .replace('\\', "/")
        })
        .collect();
    out.sort();
    out
}

/// One JSON-Lines batch over the fixture directories; envelopes by input path.
fn batch(root: &Path, args: &[&str], dirs: &[String]) -> BTreeMap<String, Value> {
    let out = Command::new(env!("CARGO_BIN_EXE_openreadout"))
        .current_dir(root)
        .env("OPENREADOUT_LIVE_WINDOW", "0")
        .env_remove("OPENREADOUT_STRICT")
        .args(args)
        .args(["--jsonl", "--recursive", "--skip-unknown", "--threads", "2"])
        .args(dirs)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| Some((v["path"].as_str()?.replace('\\', "/"), v)))
        .collect()
}

#[test]
fn fixture_outputs_match_their_snapshots() {
    let root = root();
    let dirs = fixture_dirs(&root);
    assert!(dirs.len() > 10, "fixture directories: {dirs:?}");
    let infos = batch(&root, &["info"], &dirs);
    let checks = batch(&root, &["check", "--headers-only"], &dirs);
    assert!(infos.len() > 50, "only {} fixtures read", infos.len());
    let prefixes = snapshot::prefixes(&[&root]);
    let mut fresh = BTreeMap::new();
    for (path, env) in &infos {
        let ok = env["ok"].as_bool() == Some(true);
        let info = if ok {
            Ok(&env["data"])
        } else {
            Err(env["error"]["code"].as_str().unwrap_or("error"))
        };
        let format = env["data"]["format"]["id"]
            .as_str()
            .or_else(|| checks.get(path).and_then(|c| c["data"]["format"].as_str()));
        let check = match checks.get(path) {
            Some(c) if c["ok"].as_bool() == Some(true) => Ok(&c["data"]),
            Some(c) => Err(c["error"]["code"].as_str().unwrap_or("error")),
            None => Err("missing"),
        };
        fresh.insert(
            path.clone(),
            snapshot::record(path, format, info, check, &prefixes),
        );
    }
    let committed =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/fixtures.jsonl");
    let problems = snapshot::compare(
        &committed,
        &fresh,
        &snapshot::out_dir("fixtures"),
        "fixtures",
        true,
    );
    assert!(
        problems.is_empty(),
        "{} fixture outputs differ from tests/snapshots/fixtures.jsonl. If the change is intended, \
         review and accept it: `cargo xtask snapshot review` then `cargo xtask snapshot accept` \
         (or rerun with SNAPSHOT_ACCEPT=1).\n  {}",
        problems.len(),
        problems.join("\n  ")
    );
}
