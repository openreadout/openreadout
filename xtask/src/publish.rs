//! `cargo xtask publish-order`: the publishable crates of the workspace in the order crates.io
//! needs them (every internal dependency, dev- and build-dependencies included, before its
//! dependents). The CI `publish-dry-run` job and the release workflow's `publish-crates` job read
//! the list from here, so a new crate cannot be left out of either.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use anyhow::{Context, Result, bail};

use crate::root;

/// Publishable workspace crates (`crates/*` without `publish = false`) and, for each, the
/// internal crates it depends on.
fn graph() -> Result<BTreeMap<String, BTreeSet<String>>> {
    let mut crates = BTreeMap::new();
    let mut unpublished = BTreeSet::new();
    let dir = root().join("crates");
    for entry in fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
        let manifest = entry?.path().join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let text = fs::read_to_string(&manifest)
            .with_context(|| format!("read {}", manifest.display()))?;
        let doc: toml::Value =
            toml::from_str(&text).with_context(|| format!("parse {}", manifest.display()))?;
        let package = doc
            .get("package")
            .with_context(|| format!("{}: no [package]", manifest.display()))?;
        let name = package
            .get("name")
            .and_then(toml::Value::as_str)
            .with_context(|| format!("{}: no package name", manifest.display()))?
            .to_string();
        if package.get("publish").and_then(toml::Value::as_bool) == Some(false) {
            unpublished.insert(name);
            continue;
        }
        let mut deps = BTreeSet::new();
        let mut tables: Vec<&toml::Value> =
            ["dependencies", "dev-dependencies", "build-dependencies"]
                .iter()
                .filter_map(|k| doc.get(*k))
                .collect();
        if let Some(targets) = doc.get("target").and_then(toml::Value::as_table) {
            for t in targets.values() {
                for k in ["dependencies", "dev-dependencies", "build-dependencies"] {
                    if let Some(v) = t.get(k) {
                        tables.push(v);
                    }
                }
            }
        }
        for t in tables.into_iter().filter_map(toml::Value::as_table) {
            deps.extend(
                t.keys()
                    .filter(|d| d.starts_with("openreadout-") && **d != name)
                    .cloned(),
            );
        }
        crates.insert(name, deps);
    }
    for (name, deps) in &crates {
        if let Some(d) = deps.iter().find(|d| unpublished.contains(*d)) {
            bail!("{name} depends on {d}, which is `publish = false`");
        }
    }
    Ok(crates)
}

/// Dependencies first; among crates whose dependencies are all placed, alphabetical order.
pub fn order() -> Result<Vec<String>> {
    let g = graph()?;
    let mut placed: Vec<String> = Vec::with_capacity(g.len());
    let mut done = BTreeSet::new();
    while placed.len() < g.len() {
        let next: Vec<String> = g
            .iter()
            .filter(|(n, deps)| {
                !done.contains(*n) && deps.iter().all(|d| done.contains(d) || !g.contains_key(d))
            })
            .map(|(n, _)| n.clone())
            .collect();
        if next.is_empty() {
            let rest: Vec<&String> = g.keys().filter(|n| !done.contains(*n)).collect();
            bail!("dependency cycle among {rest:?}");
        }
        for n in next {
            done.insert(n.clone());
            placed.push(n);
        }
    }
    Ok(placed)
}

/// Print the order, one crate per line, or as `-p NAME` arguments for `cargo publish`.
pub fn run(flags: bool) -> Result<()> {
    let o = order()?;
    if flags {
        let args: Vec<String> = o.iter().map(|c| format!("-p {c}")).collect();
        println!("{}", args.join(" "));
    } else {
        for c in o {
            println!("{c}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn workspace_order_puts_dependencies_first() {
        let o = super::order().unwrap();
        let pos = |n: &str| o.iter().position(|c| c == n).unwrap();
        assert_eq!(o[0], "openreadout-core");
        assert!(pos("openreadout-codecs") < pos("openreadout-czi"));
        assert!(pos("openreadout-mcp") < pos("openreadout"));
        assert_eq!(o.last().map(String::as_str), Some("openreadout"));
        assert!(
            !o.iter()
                .any(|c| c == "openreadout-py" || c == "openreadout-wasm")
        );
    }
}
