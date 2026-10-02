//! `index` over the whole corpus: every input file the manifest lists is recognised (as a data
//! set or as a member of one), a second crawl reuses every record, search and the health
//! report work on real files, and the personal-data flags never carry values.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test index -- --nocapture`
//! Env: `OPENREADOUT_CORPUS_DIR` overrides `corpus/files`.
#![cfg(feature = "corpus")]

#[path = "support/registry.rs"]
mod shared_registry;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use openreadout_core::Registry;
use openreadout_index::tables::{Cell, scan_table};
use openreadout_index::{HealthOptions, IndexOptions, SearchRequest};
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
    #[serde(default)]
    tier: String,
    #[serde(default)]
    bundle: String,
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

/// Every reader, in the CLI's detection order (`support/registry.rs`, kept equal to
/// `crates/openreadout-cli/src/registry.rs` by `openreadout`'s `tests/registry_parity.rs`).
fn registry() -> Registry {
    shared_registry::registry()
}

fn str_cell(c: Cell) -> Option<String> {
    match c {
        Cell::Str(s) => Some(s),
        _ => None,
    }
}

/// Files below `dir` (symbolic links not followed) modified, created or renamed at or after
/// `since`.
fn touched_since(dir: &Path, since: std::time::SystemTime) -> u64 {
    let mut n = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let Ok(md) = e.path().symlink_metadata() else {
                continue;
            };
            if md.is_dir() {
                stack.push(e.path());
                continue;
            }
            let mut when = md.modified().ok();
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let ctime = std::time::UNIX_EPOCH
                    + std::time::Duration::from_secs(u64::try_from(md.ctime()).unwrap_or(0));
                when = when.max(Some(ctime));
            }
            if when.is_some_and(|t| t >= since) {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn corpus_index() {
    let dir = corpus_dir();
    if !dir.exists() {
        eprintln!("no corpus at {}; skipped", dir.display());
        return;
    }
    let dir = dir.canonicalize().unwrap();
    let manifest: Manifest =
        toml::from_str(&std::fs::read_to_string(root().join("corpus/manifest.toml")).unwrap())
            .unwrap();
    let tmp = std::env::temp_dir().join(format!("openreadout-corpus-index-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let reg = registry();
    let mut o = IndexOptions::default();
    o.roots = vec![dir.clone()];
    o.index_dir.clone_from(&tmp);
    // Held-out files are never opened outside the held-out measurement
    // (docs/benchmark/heldout.md): leave out `heldout/` and the held-out bundles' folders.
    let mut held: std::collections::BTreeSet<String> = ["heldout".to_string()].into();
    for e in manifest.file.iter().filter(|e| e.tier == "heldout") {
        if e.role == "bundle" {
            held.insert(e.id.clone());
        }
        if !e.bundle.is_empty() {
            held.insert(e.bundle.clone());
        }
        if let Some((top, _)) = e.filename.split_once('/') {
            held.insert(top.to_string());
        }
    }
    o.exclude = held.into_iter().collect();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .unwrap();
    // Other processes may add files to a shared corpus while this test runs: the second crawl
    // then reads those again (below). Files are counted as touched from here on.
    let started = std::time::SystemTime::now() - std::time::Duration::from_secs(2);
    let m = pool
        .install(|| openreadout_index::index(&reg, &o, None))
        .unwrap();
    println!(
        "corpus index: {} data sets, {} files, {} items in {:.1} s ({:.0} items/s), {} bytes read by the crawler",
        m.datasets, m.files, m.crawl.items, m.crawl.elapsed_s, m.files_per_s, m.crawl.bytes_read
    );
    println!(
        "formats: {:?}",
        m.formats
            .iter()
            .map(|(k, v)| (k, v.count))
            .collect::<Vec<_>>()
    );
    println!("check: {:?}  pii: {:?}", m.check_status, m.pii.by_kind);
    assert!(m.complete);

    // Role of every file the index saw.
    let mut role: HashMap<String, (String, Option<String>)> = HashMap::new();
    scan_table(
        &tmp.join("files.parquet"),
        Some(&["path", "role", "format"]),
        &mut |rows| {
            for r in 0..rows.len() {
                role.insert(
                    str_cell(rows.get(r, "path")).unwrap(),
                    (
                        str_cell(rows.get(r, "role")).unwrap(),
                        str_cell(rows.get(r, "format")),
                    ),
                );
            }
            Ok(())
        },
    )
    .unwrap();
    let mut missing = Vec::new();
    let mut checked = 0usize;
    for e in &manifest.file {
        if e.role != "input" && !e.role.is_empty() {
            continue;
        }
        let p = dir.join(&e.filename);
        if !p.exists() || p.to_string_lossy().split('/').any(|c| c.starts_with('.')) {
            continue;
        }
        checked += 1;
        match role.get(p.to_string_lossy().as_ref()) {
            Some((r, _)) if r != "unknown" => {}
            other => missing.push(format!("{} ({}): {other:?}", e.id, e.format)),
        }
    }
    println!("{checked} manifest input files checked");
    assert!(
        missing.is_empty(),
        "manifest input files the index does not recognise:\n{}",
        missing.join("\n")
    );

    // Values of flagged fields never reach the tables.
    let mut problems_text = String::new();
    scan_table(&tmp.join("problems.parquet"), None, &mut |rows| {
        for r in 0..rows.len() {
            for c in 0..rows.names().len() {
                problems_text.push_str(&rows.cell(r, c).to_text());
                problems_text.push('\n');
            }
        }
        Ok(())
    })
    .unwrap();
    let mut req = SearchRequest::default();
    req.query = "pii=person_name".into();
    req.limit = Some(0);
    req.fields = vec!["operator".into()];
    let flagged = openreadout_index::search(&tmp, &req).unwrap();
    for h in &flagged.results {
        if let Some(op) = h["operator"].as_str() {
            // Whole words only ("Sam" is in "SamplePrecision"); paths are not values.
            let hit = problems_text.lines().find(|l| {
                !l.starts_with('/')
                    && l.match_indices(op).any(|(i, _)| {
                        let before = l[..i].chars().next_back();
                        let after = l[i + op.len()..].chars().next();
                        !before.is_some_and(char::is_alphanumeric)
                            && !after.is_some_and(char::is_alphanumeric)
                    })
            });
            assert!(
                hit.is_none(),
                "a flagged value reached problems.parquet: {hit:?}"
            );
        }
    }

    // Search agrees with the tables.
    let mut req = SearchRequest::default();
    req.query = "format=czi".into();
    req.limit = Some(0);
    let czi = openreadout_index::search(&tmp, &req).unwrap();
    assert_eq!(czi.total, m.formats["czi"].count);
    let mut per_family: BTreeMap<String, u64> = BTreeMap::new();
    scan_table(
        &tmp.join("experiments.parquet"),
        Some(&["family"]),
        &mut |rows| {
            for r in 0..rows.len() {
                *per_family
                    .entry(str_cell(rows.get(r, "family")).unwrap_or_default())
                    .or_default() += 1;
            }
            Ok(())
        },
    )
    .unwrap();
    for (f, t) in &m.families {
        assert_eq!(per_family.get(f), Some(&t.count), "{f}");
    }

    // A second crawl reuses every record, except items whose files another process added or
    // rewrote since the first crawl began (each such file makes at most one item new or
    // changed).
    let m2 = pool
        .install(|| openreadout_index::index(&reg, &o, None))
        .unwrap();
    let touched = touched_since(&dir, started);
    let reread = m2.crawl.new + m2.crawl.changed;
    if touched == 0 {
        assert_eq!(m2.crawl.unchanged, m2.crawl.items, "{:?}", m2.crawl);
        assert_eq!(m2.datasets, m.datasets);
    } else {
        println!("{touched} corpus files were added or rewritten during the test");
        assert_eq!(
            m2.crawl.unchanged + reread,
            m2.crawl.items,
            "{:?}",
            m2.crawl
        );
        assert!(
            reread <= touched,
            "second crawl read {reread} items again, but only {touched} files changed: {:?}",
            m2.crawl
        );
    }

    // Health.
    let h = openreadout_index::health(&tmp, &HealthOptions::default()).unwrap();
    println!(
        "health: {} integrity problems, {} duplicate groups ({} bytes hashed), {} near-duplicate groups, {} at risk ({} legacy)",
        h.integrity.problem_count,
        h.duplicates.group_count,
        h.duplicates.bytes_hashed,
        h.near_duplicates.group_count,
        h.at_risk.count,
        h.at_risk.legacy
    );
    assert!(
        h.integrity.problem_count > 0,
        "the corpus holds truncated files on purpose"
    );
    let md = openreadout_index::health::render_markdown(&h);
    for hit in &flagged.results {
        if let Some(op) = hit["operator"].as_str() {
            // Paths are listed as they are; everything else must not carry the value.
            let without_paths: String = md
                .split_whitespace()
                .filter(|t| !t.contains('/'))
                .collect::<Vec<_>>()
                .join(" ");
            let leaked = without_paths.match_indices(op).any(|(i, _)| {
                let before = without_paths[..i].chars().next_back();
                let after = without_paths[i + op.len()..].chars().next();
                !before.is_some_and(char::is_alphanumeric)
                    && !after.is_some_and(char::is_alphanumeric)
            });
            assert!(!leaked, "a flagged value reached the health report");
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
}
