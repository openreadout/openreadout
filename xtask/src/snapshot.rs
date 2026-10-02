//! `cargo xtask snapshot review|accept`: the golden-output snapshots
//! (docs/maintaining.md § Regression snapshots).
//!
//! Two tests write fresh records under `<target>/tmp/snapshots/` when outputs differ from the
//! committed ones: `openreadout`'s `tests/golden.rs` (every committed fixture; committed records
//! in `crates/openreadout-cli/tests/snapshots/fixtures.jsonl`) and the corpus harness's
//! `tests/snapshots.rs` (every development-corpus file on disk; `corpus/snapshots/<format>.jsonl`,
//! with the full `info` of changed inputs in `.../corpus/new/<id>.json` and of unchanged ones in
//! `.../corpus/baseline/<id>.json`). `review` prints every difference field by field, and for
//! corpus inputs with a baseline the changed paths of the full `info`; `accept` writes the fresh
//! records into the committed files.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

#[derive(clap::Subcommand)]
pub enum SnapshotCmd {
    /// Show every difference between the fresh records of the last test run and the committed
    /// snapshots.
    Review {
        /// Only inputs whose id contains this.
        #[arg(long)]
        only: Option<String>,
        /// Lines of full-`info` differences shown per input.
        #[arg(long, default_value_t = 40)]
        lines: usize,
    },
    /// Write the fresh records into the committed snapshots (after `review`).
    Accept {
        /// Only inputs whose id contains this.
        #[arg(long)]
        only: Option<String>,
    },
}

fn target_dir(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from)
}

/// A snapshot set: name, committed file, fresh file, full-output directory, complete.
type Set = (String, PathBuf, PathBuf, Option<PathBuf>, bool);

/// Every snapshot set with fresh records on disk.
fn sets(root: &Path) -> Result<Vec<Set>> {
    let tmp = target_dir(root).join("tmp/snapshots");
    let mut out = Vec::new();
    let f = tmp.join("fixtures/fixtures.jsonl");
    if f.exists() {
        out.push((
            "fixtures".to_string(),
            root.join("crates/openreadout-cli/tests/snapshots/fixtures.jsonl"),
            f,
            None,
            true,
        ));
    }
    let corpus = tmp.join("corpus");
    if corpus.is_dir() {
        let mut names: Vec<PathBuf> = fs::read_dir(&corpus)?
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
            .collect();
        names.sort();
        for p in names {
            let name = p
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            out.push((
                format!("corpus/{name}"),
                root.join(format!("corpus/snapshots/{name}.jsonl")),
                p,
                Some(corpus.clone()),
                false,
            ));
        }
    }
    Ok(out)
}

fn parse(path: &Path) -> BTreeMap<String, Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| Some((v["id"].as_str()?.to_string(), v)))
        .collect()
}

fn render(records: &BTreeMap<String, Value>) -> String {
    let mut s = String::new();
    for v in records.values() {
        s.push_str(&serde_json::to_string(v).unwrap_or_default());
        s.push('\n');
    }
    s
}

/// `path: old -> new` for every leaf that differs.
#[allow(clippy::many_single_char_names)]
pub fn diff(a: &Value, b: &Value) -> Vec<String> {
    fn short(v: &Value) -> String {
        let s = v.to_string();
        if s.chars().count() > 120 {
            format!("{}…", s.chars().take(120).collect::<String>())
        } else {
            s
        }
    }
    fn walk(path: &str, a: &Value, b: &Value, out: &mut Vec<String>) {
        if a == b {
            return;
        }
        match (a, b) {
            (Value::Object(x), Value::Object(y)) => {
                let keys: BTreeSet<&String> = x.keys().chain(y.keys()).collect();
                for k in keys {
                    let p = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    walk(
                        &p,
                        x.get(k).unwrap_or(&Value::Null),
                        y.get(k).unwrap_or(&Value::Null),
                        out,
                    );
                }
            }
            (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
                for (i, (p, q)) in x.iter().zip(y).enumerate() {
                    walk(&format!("{path}[{i}]"), p, q, out);
                }
            }
            (Value::Array(x), Value::Array(y)) => {
                out.push(format!("{path}: {} items -> {} items", x.len(), y.len()));
                for (i, (p, q)) in x.iter().zip(y).enumerate() {
                    walk(&format!("{path}[{i}]"), p, q, out);
                }
            }
            _ => out.push(format!("{path}: {} -> {}", short(a), short(b))),
        }
    }
    let mut out = Vec::new();
    walk("", a, b, &mut out);
    out
}

pub fn run(cmd: SnapshotCmd) -> Result<()> {
    let root = crate::root();
    match cmd {
        SnapshotCmd::Review { only, lines } => review(&root, only.as_deref(), lines),
        SnapshotCmd::Accept { only } => accept(&root, only.as_deref()),
    }
}

fn review(root: &Path, only: Option<&str>, lines: usize) -> Result<()> {
    let sets = sets(root)?;
    if sets.is_empty() {
        println!(
            "no fresh records under {}: run `cargo test -p openreadout --test golden` and/or \
             `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test snapshots` first",
            target_dir(root).join("tmp/snapshots").display()
        );
        return Ok(());
    }
    let mut changed = 0;
    let mut added = 0;
    let mut gone = 0;
    for (name, committed, fresh, full, complete) in &sets {
        let old = parse(committed);
        let new = parse(fresh);
        for (id, n) in &new {
            if only.is_some_and(|o| !id.contains(o)) {
                continue;
            }
            match old.get(id) {
                Some(o) if o == n => {}
                Some(o) => {
                    changed += 1;
                    println!("~ [{name}] {id}");
                    for d in diff(o, n) {
                        println!("    {d}");
                    }
                    if let Some(dir) = full {
                        let b = dir.join("baseline").join(format!("{id}.json"));
                        let c = dir.join("new").join(format!("{id}.json"));
                        if let (Ok(bt), Ok(ct)) = (fs::read_to_string(&b), fs::read_to_string(&c)) {
                            let bv: Value = serde_json::from_str(&bt).unwrap_or_default();
                            let cv: Value = serde_json::from_str(&ct).unwrap_or_default();
                            let d = diff(&bv, &cv);
                            println!("    full info ({} paths changed):", d.len());
                            for l in d.iter().take(lines) {
                                println!("      {l}");
                            }
                            if d.len() > lines {
                                println!("      … {} more (--lines N)", d.len() - lines);
                            }
                        } else if c.exists() {
                            println!(
                                "    full info: {} (no baseline: run the test on the base branch first to diff field by field)",
                                c.display()
                            );
                        }
                    }
                }
                None => {
                    added += 1;
                    println!("+ [{name}] {id}: {}", serde_json::to_string(n)?);
                }
            }
        }
        if *complete {
            for id in old.keys().filter(|k| !new.contains_key(*k)) {
                if only.is_some_and(|o| !id.contains(o)) {
                    continue;
                }
                gone += 1;
                println!("- [{name}] {id}: committed, no longer read");
            }
        }
    }
    println!(
        "\n{changed} changed, {added} new, {gone} gone. Accept with `cargo xtask snapshot accept`{}.",
        only.map(|o| format!(" --only {o}")).unwrap_or_default()
    );
    Ok(())
}

fn accept(root: &Path, only: Option<&str>) -> Result<()> {
    let mut n = 0;
    for (name, committed, fresh, full, complete) in sets(root)? {
        let old = parse(&committed);
        let new = parse(&fresh);
        let mut merged = if complete && only.is_none() {
            BTreeMap::new()
        } else {
            old.clone()
        };
        if complete && only.is_none() {
            merged.clone_from(&new);
        } else {
            for (id, v) in &new {
                if only.is_none_or(|o| id.contains(o)) {
                    merged.insert(id.clone(), v.clone());
                }
            }
        }
        let changed = merged
            .iter()
            .filter(|(k, v)| old.get(*k) != Some(*v))
            .count()
            + old.keys().filter(|k| !merged.contains_key(*k)).count();
        if changed == 0 {
            continue;
        }
        if let Some(p) = committed.parent() {
            fs::create_dir_all(p)?;
        }
        fs::write(&committed, render(&merged))
            .with_context(|| format!("write {}", committed.display()))?;
        n += changed;
        println!(
            "[{name}] {changed} records -> {}",
            committed.strip_prefix(root).unwrap_or(&committed).display()
        );
        // the accepted full outputs become the baseline
        if let Some(dir) = full {
            for id in new.keys().filter(|id| only.is_none_or(|o| id.contains(o))) {
                let c = dir.join("new").join(format!("{id}.json"));
                if c.exists() {
                    let b = dir.join("baseline").join(format!("{id}.json"));
                    fs::create_dir_all(b.parent().expect("has a parent"))?;
                    fs::rename(&c, &b)?;
                }
            }
        }
    }
    println!("accepted {n} records");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diffs_name_the_changed_paths() {
        let a = serde_json::json!({"x": 1, "l": [1, 2], "o": {"k": "a"}});
        let b = serde_json::json!({"x": 2, "l": [1, 2, 3], "o": {"k": "a", "n": true}});
        let d = diff(&a, &b);
        assert!(d.contains(&"x: 1 -> 2".to_string()), "{d:?}");
        assert!(d.contains(&"l: 2 items -> 3 items".to_string()), "{d:?}");
        assert!(d.contains(&"o.n: null -> true".to_string()), "{d:?}");
    }
}
