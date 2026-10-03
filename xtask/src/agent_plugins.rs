//! `cargo xtask agent-plugins --out DIR`: assemble the openreadout/agent-plugins repository.
//!
//! That repository holds the Claude Code and Codex plugins and the Gemini CLI extension, so
//! installing one doesn't clone this whole repository (and so the Claude plugin directory, which
//! stops at 50 MiB, can read it). The release workflow builds the tree with this command and
//! pushes it there (docs/release-process.md). The tree is:
//!
//! - everything in `packaging/agent-plugins/`: the manifests, the plugins' `.mcp.json`, a README;
//! - `skills/openreadout/` with its references;
//! - `assets/icon.svg`, `PRIVACY.md`, `LICENSE-MIT`, `LICENSE-APACHE` and `NOTICE`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Where the manifests live in this repository, relative to the root.
pub const SOURCE: &str = "packaging/agent-plugins";

/// Single files copied from the repository root to the same path in the tree.
const ROOT_FILES: [&str; 5] = [
    "assets/icon.svg",
    "PRIVACY.md",
    "LICENSE-MIT",
    "LICENSE-APACHE",
    "NOTICE",
];

/// Directories copied whole from the repository root to the same path in the tree.
const ROOT_DIRS: [&str; 1] = ["skills/openreadout"];

/// The JSON manifests in the tree, relative to its root.
const MANIFESTS: [&str; 5] = [
    ".claude-plugin/plugin.json",
    ".claude-plugin/marketplace.json",
    ".codex-plugin/plugin.json",
    ".agents/plugins/marketplace.json",
    "gemini-extension.json",
];

/// Every version string in the manifests under `dir`, labelled by file and field.
pub fn manifest_versions(dir: &Path) -> Result<Vec<(String, Option<String>)>> {
    let read = |rel: &str| -> Result<serde_json::Value> {
        let p = dir.join(rel);
        let text = fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?;
        serde_json::from_str(&text).with_context(|| format!("parse {}", p.display()))
    };
    let s = |v: &serde_json::Value| v.as_str().map(str::to_owned);
    let mut out = Vec::new();
    out.push((
        ".claude-plugin/plugin.json version".to_owned(),
        s(&read(".claude-plugin/plugin.json")?["version"]),
    ));
    let mk = read(".claude-plugin/marketplace.json")?;
    out.push((
        ".claude-plugin/marketplace.json metadata.version".to_owned(),
        s(&mk["metadata"]["version"]),
    ));
    for p in mk["plugins"].as_array().into_iter().flatten() {
        out.push((
            format!(
                ".claude-plugin/marketplace.json plugins[{}] version",
                p["name"].as_str().unwrap_or("?")
            ),
            s(&p["version"]),
        ));
    }
    for file in [".codex-plugin/plugin.json", "gemini-extension.json"] {
        out.push((format!("{file} version"), s(&read(file)?["version"])));
    }
    Ok(out)
}

/// Build the tree from the repository at `root` into `out`, which must be missing or empty.
/// Fails if a manifest's version is not `version`, or a manifest names a `./` path the tree
/// lacks. Returns the files written, relative to `out`, sorted.
pub fn build(root: &Path, version: &str, out: &Path) -> Result<Vec<PathBuf>> {
    let source = root.join(SOURCE);
    let wrong: Vec<String> = manifest_versions(&source)?
        .into_iter()
        .filter(|(_, v)| v.as_deref() != Some(version))
        .map(|(label, v)| format!("{SOURCE}/{label} is {v:?}"))
        .collect();
    if !wrong.is_empty() {
        bail!(
            "manifest versions disagree with Cargo.toml ({version}): {}; run scripts/bump-version.sh",
            wrong.join(", ")
        );
    }

    if out.exists() {
        let mut entries = fs::read_dir(out).with_context(|| format!("read {}", out.display()))?;
        if entries.next().is_some() {
            bail!(
                "{} is not empty; remove it or pass an empty directory",
                out.display()
            );
        }
    }
    fs::create_dir_all(out).with_context(|| format!("create {}", out.display()))?;

    let mut written = Vec::new();
    copy_dir(&source, out, Path::new(""), &mut written)?;
    for dir in ROOT_DIRS {
        copy_dir(
            &root.join(dir),
            &out.join(dir),
            Path::new(dir),
            &mut written,
        )?;
    }
    for file in ROOT_FILES {
        let dest = out.join(file);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(root.join(file), &dest).with_context(|| format!("copy {file}"))?;
        written.push(PathBuf::from(file));
    }
    written.sort();

    // Every relative path a manifest names (skills, icons, MCP configs, plugin sources) exists.
    let mut missing = Vec::new();
    for m in MANIFESTS {
        let p = out.join(m);
        let json: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?,
        )
        .with_context(|| format!("parse {}", p.display()))?;
        let mut paths = Vec::new();
        relative_paths(&json, &mut paths);
        for rel in paths {
            if !out.join(rel).exists() {
                missing.push(format!("{m} names {rel}"));
            }
        }
    }
    if !missing.is_empty() {
        bail!("missing from the tree: {}", missing.join(", "));
    }
    Ok(written)
}

/// Every string value in `v` that starts with `./`.
fn relative_paths<'a>(v: &'a serde_json::Value, out: &mut Vec<&'a str>) {
    match v {
        serde_json::Value::String(s) if s.starts_with("./") => out.push(s),
        serde_json::Value::Array(a) => a.iter().for_each(|x| relative_paths(x, out)),
        serde_json::Value::Object(o) => o.values().for_each(|x| relative_paths(x, out)),
        _ => {}
    }
}

/// Copy the files under `from` into `to`, recording each as `rel/<name>`.
fn copy_dir(from: &Path, to: &Path, rel: &Path, written: &mut Vec<PathBuf>) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from).with_context(|| format!("read {}", from.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".DS_Store" {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_dir(&entry.path(), &to.join(&name), &rel.join(&name), written)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), to.join(&name))
                .with_context(|| format!("copy {}", entry.path().display()))?;
            written.push(rel.join(&name));
        } else {
            bail!("{} is not a regular file", entry.path().display());
        }
    }
    Ok(())
}

/// The command: build the tree from this repository at the Cargo.toml version.
pub fn run(out: &Path) -> Result<()> {
    let version = crate::packaging::cargo_version()?;
    let files = build(&crate::root(), &version, out)?;
    for f in &files {
        println!("{}", f.display());
    }
    println!(
        "agent-plugins {version}: {} files in {}",
        files.len(),
        out.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(files: &[PathBuf]) -> Vec<String> {
        files.iter().map(|f| f.display().to_string()).collect()
    }

    #[test]
    fn builds_the_tree_from_this_repository() {
        let root = crate::root();
        let version = crate::packaging::cargo_version().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("tree");
        let files = rel(&build(&root, &version, &out).unwrap());
        for want in [
            ".claude-plugin/plugin.json",
            ".claude-plugin/marketplace.json",
            ".codex-plugin/plugin.json",
            ".agents/plugins/marketplace.json",
            "gemini-extension.json",
            ".mcp.json",
            "README.md",
            "skills/openreadout/SKILL.md",
            "skills/openreadout/references/formats.md",
            "assets/icon.svg",
            "PRIVACY.md",
            "LICENSE-MIT",
            "LICENSE-APACHE",
            "NOTICE",
        ] {
            assert!(files.iter().any(|f| f == want), "{want} missing: {files:?}");
        }
        assert_eq!(
            fs::read(out.join("skills/openreadout/SKILL.md")).unwrap(),
            fs::read(root.join("skills/openreadout/SKILL.md")).unwrap()
        );
        // The plugin directory holds a plugin with over 512 files or a file over 256 KiB.
        assert!(files.len() < 512);
        for f in &files {
            let len = fs::metadata(out.join(f)).unwrap().len();
            assert!(len < 256 * 1024, "{f} is {len} bytes");
        }
        // A second build into the same directory refuses to mix trees.
        let err = build(&root, &version, &out).unwrap_err().to_string();
        assert!(err.contains("not empty"), "{err}");
    }

    #[test]
    fn refuses_a_version_that_disagrees_with_cargo() {
        let tmp = tempfile::tempdir().unwrap();
        let err = build(&crate::root(), "999.0.0", &tmp.path().join("tree"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("999.0.0"), "{err}");
        assert!(err.contains("gemini-extension.json version"), "{err}");
        assert!(!tmp.path().join("tree").exists());
    }

    #[test]
    fn finds_relative_paths_anywhere_in_a_manifest() {
        let v = serde_json::json!({
            "skills": ["./skills/openreadout"],
            "interface": {"logo": "./assets/icon.svg", "url": "https://x"},
            "plugins": [{"source": {"path": "./"}}],
            "name": "openreadout"
        });
        let mut paths = Vec::new();
        relative_paths(&v, &mut paths);
        paths.sort_unstable();
        assert_eq!(paths, ["./", "./assets/icon.svg", "./skills/openreadout"]);
    }

    #[test]
    fn refuses_a_manifest_path_the_tree_lacks() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        let real = crate::root();
        let version = crate::packaging::cargo_version().unwrap();
        // Copy this repository's inputs, then point the Codex manifest at an icon nothing copies.
        let src = root.join(SOURCE);
        let mut written = Vec::new();
        copy_dir(&real.join(SOURCE), &src, Path::new(""), &mut written).unwrap();
        let codex = src.join(".codex-plugin/plugin.json");
        let text = fs::read_to_string(&codex)
            .unwrap()
            .replace("./assets/icon.svg", "./assets/missing.svg");
        fs::write(&codex, text).unwrap();
        for dir in ROOT_DIRS {
            copy_dir(
                &real.join(dir),
                &root.join(dir),
                Path::new(dir),
                &mut written,
            )
            .unwrap();
        }
        for file in ROOT_FILES {
            fs::create_dir_all(root.join(file).parent().unwrap()).unwrap();
            fs::copy(real.join(file), root.join(file)).unwrap();
        }
        let err = build(&root, &version, &tmp.path().join("tree"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("./assets/missing.svg"), "{err}");
    }
}
