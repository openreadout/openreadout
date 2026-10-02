//! New-variant intakes (`corpus/intake/<id>.toml`, written by `cargo xtask variant intake`;
//! docs/maintaining.md): every record's file must meet the record's expectations. At intake the
//! expectations fail (the file is refused, unvalidated or has no ground truth); they pass once the
//! reader handles the variant, its oracle exists and the assurance evidence is refreshed, and
//! from then on they pin the variant.
//!
//! Records with status `awaiting-file` (a report bundle only) or `closed` are skipped, and so are
//! files not on disk (intake files start on the `hold` tier, which is never fetched
//! automatically).
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test intake`.
#![cfg(feature = "corpus")]

#[path = "support/registry.rs"]
mod registry;

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Deserialize)]
struct Manifest {
    file: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    id: String,
    #[serde(default)]
    role: String,
    filename: String,
}

#[derive(Deserialize)]
struct Record {
    id: String,
    status: String,
    expect: Expect,
}

#[derive(Deserialize)]
struct Expect {
    opens: bool,
    min_level: String,
    oracle: bool,
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn rank(level: &str) -> u8 {
    match level {
        "validated" => 3,
        "partially_validated" => 2,
        "unvalidated" => 1,
        _ => 0,
    }
}

#[test]
fn intake_expectations_hold() {
    let root = root();
    let dir = root.join("corpus/intake");
    if !dir.exists() {
        return;
    }
    let manifest: Manifest =
        toml::from_str(&std::fs::read_to_string(root.join("corpus/manifest.toml")).unwrap())
            .unwrap();
    let files = std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root.join("corpus/files"), PathBuf::from);
    let reg = registry::registry();
    let mut failures = Vec::new();
    let mut checked = 0;
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    paths.sort();
    for p in paths {
        let r: Record = toml::from_str(&std::fs::read_to_string(&p).unwrap())
            .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        if !matches!(r.status.as_str(), "open" | "validated") {
            continue;
        }
        let Some(e) = manifest
            .file
            .iter()
            .find(|e| e.id == r.id && (e.role.is_empty() || e.role == "input"))
        else {
            failures.push(format!("{}: no manifest input entry", r.id));
            continue;
        };
        let path = files.join(&e.filename);
        if !path.exists() {
            println!("skip   {:<40} not on disk ({})", r.id, e.filename);
            continue;
        }
        checked += 1;
        let mut why = Vec::new();
        match reg.open(&path).and_then(|(_, ds)| {
            let info = ds.info()?;
            Ok(openreadout_core::assurance::assess_dataset(
                ds.as_ref(),
                &info,
            ))
        }) {
            Ok(a) => {
                let level = a.level.as_str();
                if rank(level) < rank(&r.expect.min_level) {
                    why.push(format!(
                        "assurance {level} < {} ({})",
                        r.expect.min_level,
                        a.reasons.join("; ")
                    ));
                }
            }
            Err(err) if r.expect.opens => why.push(format!("does not open: {err}")),
            Err(_) => {}
        }
        if r.expect.oracle
            && !openreadout_corpus_tests::oracle_json::exists(
                &root.join(format!("corpus/oracle/{}.json", r.id)),
            )
        {
            why.push(format!(
                "no ground truth corpus/oracle/{}.json (cd oracle && uv run python gen.py --id {} ...)",
                r.id, r.id
            ));
        }
        if why.is_empty() {
            println!("pass   {:<40} {}", r.id, r.status);
        } else {
            println!("FAIL   {:<40} {}", r.id, why.join(" | "));
            failures.push(format!("{} ({}): {}", r.id, r.status, why.join(" | ")));
        }
    }
    println!("intake: {checked} records checked");
    assert!(
        failures.is_empty(),
        "intake expectations not met (docs/maintaining.md; the record says what the file must do):\n  {}",
        failures.join("\n  ")
    );
}
