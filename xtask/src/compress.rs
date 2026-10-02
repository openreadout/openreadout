//! `cargo xtask corpus compress`: committed ground truth over 1 MiB is stored as
//! `<name>.json.gz`, smaller files as plain `<name>.json` (reviewable diffs).
//!
//! Only the directories whose readers accept both spellings are converted ([`DIRS`]: the corpus
//! harness through `openreadout_corpus_tests::oracle_json`, the Python side through
//! `oracle/oracle_json.py`, which lists the same directories). Each conversion writes the new file
//! next to the old one, reads it back and compares, renames it into place, removes the old
//! spelling and stages both paths in git (a delete and an add). Idempotent: rerun it after merging
//! branches that wrote large oracles as plain JSON. The gzip stream is deterministic (no file
//! name, mtime 0, best compression), so compressing the same JSON twice gives the same bytes.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// JSON larger than this is stored gzip-compressed (`oracle/oracle_json.py` `GZ_THRESHOLD`).
pub const THRESHOLD: u64 = 1 << 20;

/// Directories (relative to the repository root) whose readers accept `<name>.json.gz`.
pub const DIRS: [&str; 3] = [
    "corpus/oracle",
    "corpus/oracle/heldout",
    "corpus/oracle/flow",
];

/// What to do with one file.
#[derive(Debug, PartialEq, Eq)]
pub enum Change {
    /// `<name>.json` over the threshold → `<name>.json.gz`.
    Compress(PathBuf),
    /// `<name>.json.gz` whose JSON is at most the threshold → `<name>.json`.
    Decompress(PathBuf),
}

/// Deterministic gzip of `data`.
///
/// # Errors
/// Only on an encoder failure (none in practice: the sink is a `Vec`).
pub fn gzip(data: &[u8]) -> Result<Vec<u8>> {
    let mut enc = flate2::GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(Vec::new(), flate2::Compression::best());
    enc.write_all(data)?;
    Ok(enc.finish()?)
}

fn gunzip(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(data).read_to_end(&mut out)?;
    Ok(out)
}

/// The changes `dir` needs. Errors when a name is stored both ways.
///
/// # Errors
/// Unreadable directory or `.gz` file, or `<name>.json` and `<name>.json.gz` both present.
#[allow(clippy::case_sensitive_file_extension_comparisons)] // exact spellings, as the readers match them
pub fn plan(dir: &Path) -> Result<Vec<Change>> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else {
        return Ok(out);
    };
    let mut paths: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for p in paths {
        if !p.is_file() {
            continue;
        }
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if let Some(plain) = name.strip_suffix(".gz").filter(|s| s.ends_with(".json")) {
            let sibling = p.with_file_name(plain);
            if sibling.exists() {
                bail!(
                    "both {} and {} exist: delete the stale one, then rerun",
                    sibling.display(),
                    p.display()
                );
            }
            let len = gunzip(&fs::read(&p).with_context(|| format!("read {}", p.display()))?)
                .with_context(|| format!("gunzip {}", p.display()))?
                .len() as u64;
            if len <= THRESHOLD {
                out.push(Change::Decompress(p));
            }
        } else if name.ends_with(".json") && p.metadata()?.len() > THRESHOLD {
            out.push(Change::Compress(p));
        }
    }
    Ok(out)
}

/// Write `bytes` to `dst` through a temporary sibling, read it back, compare, rename into place.
fn write_verified(dst: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = dst.with_extension("tmp-compress");
    fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
    if fs::read(&tmp)? != bytes {
        let _ = fs::remove_file(&tmp);
        bail!("{}: read-back differs", tmp.display());
    }
    fs::rename(&tmp, dst).with_context(|| format!("rename to {}", dst.display()))?;
    Ok(())
}

/// Apply one change; returns (removed, written).
///
/// # Errors
/// I/O failure, or a round trip that does not reproduce the original JSON bytes.
pub fn apply(c: &Change) -> Result<(PathBuf, PathBuf)> {
    match c {
        Change::Compress(p) => {
            let raw = fs::read(p).with_context(|| format!("read {}", p.display()))?;
            let gz = gzip(&raw)?;
            if gunzip(&gz)? != raw {
                bail!("{}: gzip round trip differs", p.display());
            }
            let mut dst = p.as_os_str().to_owned();
            dst.push(".gz");
            let dst = PathBuf::from(dst);
            write_verified(&dst, &gz)?;
            fs::remove_file(p)?;
            Ok((p.clone(), dst))
        }
        Change::Decompress(p) => {
            let raw = gunzip(&fs::read(p)?)?;
            let dst = p.with_extension(""); // drop `.gz`
            write_verified(&dst, &raw)?;
            fs::remove_file(p)?;
            Ok((p.clone(), dst))
        }
    }
}

/// Larger-than-threshold JSON outside [`DIRS`]: their readers expect plain JSON, so they stay.
fn large_elsewhere(root: &Path) -> Vec<PathBuf> {
    let keep: Vec<PathBuf> = DIRS.iter().map(|d| root.join(d)).collect();
    let mut out = Vec::new();
    let mut stack = vec![root.join("corpus/oracle")];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.filter_map(std::result::Result::ok) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if !keep.iter().any(|k| p.parent() == Some(k.as_path()))
                && p.extension().is_some_and(|x| x == "json")
                && p.metadata().is_ok_and(|m| m.len() > THRESHOLD)
            {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// `cargo xtask corpus compress [--check]`.
///
/// # Errors
/// A name stored both ways, an I/O failure, or (with `check`) any pending change.
pub fn run(root: &Path, check: bool) -> Result<()> {
    let mut changes = Vec::new();
    for d in DIRS {
        changes.extend(plan(&root.join(d))?);
    }
    for p in large_elsewhere(root) {
        eprintln!(
            "note: {} is over 1 MiB but its directory's readers expect plain JSON (left as is)",
            p.strip_prefix(root).unwrap_or(&p).display()
        );
    }
    let rel = |p: &Path| p.strip_prefix(root).unwrap_or(p).display().to_string();
    if check {
        for c in &changes {
            match c {
                Change::Compress(p) => println!("over 1 MiB, not compressed: {}", rel(p)),
                Change::Decompress(p) => println!("1 MiB or less, compressed: {}", rel(p)),
            }
        }
        if !changes.is_empty() {
            bail!(
                "{} ground-truth file(s) need `cargo xtask corpus compress`",
                changes.len()
            );
        }
        println!("ground truth storage ok");
        return Ok(());
    }
    let (mut before, mut after) = (0u64, 0u64);
    for c in &changes {
        let size_of = |p: &Path| fs::metadata(p).map_or(0, |m| m.len());
        let src = match c {
            Change::Compress(p) | Change::Decompress(p) => p.clone(),
        };
        before += size_of(&src);
        let (old, new) = apply(c)?;
        after += size_of(&new);
        println!("{} -> {}", rel(&old), rel(&new));
        // Stage the delete and the add (like `git mv`); outside a git checkout this is a no-op.
        let _ = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["add", "-A", "--"])
            .arg(&old)
            .arg(&new)
            .status();
    }
    println!(
        "{} file(s) converted ({:.1} MB -> {:.1} MB)",
        changes.len(),
        before as f64 / 1e6,
        after as f64 / 1e6
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compresses_large_and_restores_small_idempotently() {
        let d = tempfile::tempdir().unwrap();
        let big = format!("[{}0]", "1234567890,".repeat(120_000));
        fs::write(d.path().join("big.json"), &big).unwrap();
        fs::write(d.path().join("small.json"), "{}").unwrap();
        fs::write(d.path().join("tiny.json.gz"), gzip(b"[1]").unwrap()).unwrap();
        let plan1 = plan(d.path()).unwrap();
        assert_eq!(plan1.len(), 2);
        for c in &plan1 {
            apply(c).unwrap();
        }
        assert!(!d.path().join("big.json").exists());
        let gz = fs::read(d.path().join("big.json.gz")).unwrap();
        assert_eq!(gunzip(&gz).unwrap(), big.as_bytes());
        assert_eq!(gz, gzip(big.as_bytes()).unwrap(), "deterministic");
        assert_eq!(fs::read(d.path().join("tiny.json")).unwrap(), b"[1]");
        assert!(plan(d.path()).unwrap().is_empty());
        fs::write(d.path().join("big.json"), "{}").unwrap();
        assert!(plan(d.path()).is_err(), "both spellings present");
    }
}
