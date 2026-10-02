//! `cargo xtask coverage`: line and function coverage per crate (docs/maintaining.md § Code
//! health), with the largest uncovered regions of every reader crate listed so a maintainer knows
//! which decode paths no test or corpus file reaches.
//!
//! It drives cargo-llvm-cov (an optional tool: `cargo install cargo-llvm-cov` and
//! `rustup component add llvm-tools-preview`), or summarizes an LCOV file another run produced
//! (`--from lcov.info`, what CI's coverage job does). Output: `target/coverage/summary.md`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

#[derive(clap::Args)]
pub struct CoverageArgs {
    /// Summarize this LCOV file instead of running the tests.
    #[arg(long)]
    pub from: Option<PathBuf>,
    /// Only these crates (package names or short names: `czi`, `openreadout-czi`). Repeatable.
    /// Default: the workspace's default members.
    #[arg(long = "crate")]
    pub crates: Vec<String>,
    /// Also run the corpus tests under coverage (needs the corpus on disk; slow): shows the code
    /// that neither a unit test nor a corpus file reaches.
    #[arg(long)]
    pub corpus: bool,
    /// Output directory.
    #[arg(long, default_value = "target/coverage")]
    pub out: PathBuf,
    /// Uncovered regions listed per reader crate.
    #[arg(long, default_value_t = 15)]
    pub regions: usize,
}

/// Coverage of one source file.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FileCov {
    /// Instrumented lines and those executed at least once.
    pub lines: (u64, u64),
    /// Functions and those entered at least once.
    pub functions: (u64, u64),
    /// Instrumented lines never executed, in order.
    pub uncovered: Vec<u64>,
}

/// Parse LCOV (`SF:`, `DA:line,count`, `FNF:`/`FNH:`, `end_of_record`).
pub fn parse_lcov(text: &str) -> BTreeMap<String, FileCov> {
    let mut out: BTreeMap<String, FileCov> = BTreeMap::new();
    let mut current: Option<(String, FileCov)> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(p) = line.strip_prefix("SF:") {
            current = Some((p.replace('\\', "/"), FileCov::default()));
        } else if let Some(d) = line.strip_prefix("DA:") {
            if let Some((_, f)) = current.as_mut() {
                let mut parts = d.split(',');
                let n: u64 = parts.next().and_then(|x| x.parse().ok()).unwrap_or(0);
                let hits: u64 = parts
                    .next()
                    .and_then(|x| x.parse::<f64>().ok())
                    .map_or(0, |h| u64::from(h > 0.0));
                f.lines.0 += 1;
                f.lines.1 += hits;
                if hits == 0 {
                    f.uncovered.push(n);
                }
            }
        } else if let Some(n) = line.strip_prefix("FNF:") {
            if let Some((_, f)) = current.as_mut() {
                f.functions.0 = n.parse().unwrap_or(0);
            }
        } else if let Some(n) = line.strip_prefix("FNH:") {
            if let Some((_, f)) = current.as_mut() {
                f.functions.1 = n.parse().unwrap_or(0);
            }
        } else if line == "end_of_record"
            && let Some((path, f)) = current.take()
        {
            let e = out.entry(path).or_default();
            e.lines.0 += f.lines.0;
            e.lines.1 += f.lines.1;
            e.functions.0 += f.functions.0;
            e.functions.1 += f.functions.1;
            e.uncovered.extend(f.uncovered);
            e.uncovered.sort_unstable();
            e.uncovered.dedup();
        }
    }
    out
}

/// Consecutive uncovered lines as `(first, last)` ranges.
pub fn ranges(lines: &[u64]) -> Vec<(u64, u64)> {
    let mut out: Vec<(u64, u64)> = Vec::new();
    for &l in lines {
        match out.last_mut() {
            Some((_, last)) if l == *last + 1 => *last = l,
            _ => out.push((l, l)),
        }
    }
    out
}

/// The crate a source path belongs to (`crates/openreadout-czi/src/…` → `openreadout-czi`), or
/// `None` for dependencies and generated code.
pub fn crate_of(path: &str) -> Option<String> {
    let i = path.find("crates/")?;
    let rest = &path[i + 7..];
    let name = rest.split('/').next()?;
    rest.contains("/src/").then(|| name.to_string())
}

fn pct(a: u64, b: u64) -> String {
    if b == 0 {
        "-".into()
    } else {
        #[allow(clippy::cast_precision_loss)]
        let p = 100.0 * a as f64 / b as f64;
        format!("{p:.1}%")
    }
}

/// The Markdown summary: one row per crate, then the largest uncovered regions of each reader
/// crate.
pub fn summary(files: &BTreeMap<String, FileCov>, root: &Path, regions: usize) -> String {
    let mut by_crate: BTreeMap<String, FileCov> = BTreeMap::new();
    for (path, f) in files {
        if let Some(c) = crate_of(path) {
            let e = by_crate.entry(c).or_default();
            e.lines.0 += f.lines.0;
            e.lines.1 += f.lines.1;
            e.functions.0 += f.functions.0;
            e.functions.1 += f.functions.1;
        }
    }
    let readers: Vec<String> = crate::repo::reader_crates(root)
        .map(|v| {
            v.into_iter()
                .map(|c| format!("openreadout-{}", c.name))
                .collect()
        })
        .unwrap_or_default();
    let mut s = String::from(
        "# Coverage per crate\n\n| crate | lines | covered | functions | covered |\n| --- | --- | --- | --- | --- |\n",
    );
    let (mut tl, mut tc) = (0, 0);
    for (c, f) in &by_crate {
        tl += f.lines.0;
        tc += f.lines.1;
        let _ = writeln!(
            s,
            "| `{c}`{} | {} | {} | {} | {} |",
            if readers.contains(c) { " (reader)" } else { "" },
            f.lines.0,
            pct(f.lines.1, f.lines.0),
            f.functions.0,
            pct(f.functions.1, f.functions.0)
        );
    }
    let _ = writeln!(s, "| **total** | {tl} | {} | | |", pct(tc, tl));
    s.push_str("\n## Largest uncovered regions of the reader crates\n\nCode no test (and, with `--corpus`, no corpus file) executes. Each is either a variant nobody has a file for (make the assurance profile flag it, or add a synthetic test built from the documented structure) or dead code.\n");
    for r in &readers {
        let mut spans: Vec<(u64, String, u64, u64)> = Vec::new();
        for (path, f) in files
            .iter()
            .filter(|(p, _)| crate_of(p).as_deref() == Some(r))
        {
            let rel = path
                .find("crates/")
                .map_or(path.as_str(), |i| &path[i..])
                .to_string();
            for (a, b) in ranges(&f.uncovered) {
                spans.push((b - a + 1, rel.clone(), a, b));
            }
        }
        if spans.is_empty() {
            continue;
        }
        spans.sort_by(|x, y| y.0.cmp(&x.0).then(x.1.cmp(&y.1)).then(x.2.cmp(&y.2)));
        let _ = writeln!(s, "\n### `{r}`\n");
        for (n, file, a, b) in spans.into_iter().take(regions) {
            let _ = writeln!(s, "- `{file}:{a}-{b}` ({n} lines)");
        }
    }
    s
}

pub fn run(a: &CoverageArgs) -> Result<()> {
    let root = crate::root();
    let out = if a.out.is_absolute() {
        a.out.clone()
    } else {
        root.join(&a.out)
    };
    fs::create_dir_all(&out)?;
    let lcov = if let Some(f) = &a.from {
        f.clone()
    } else {
        let have = Command::new("cargo")
            .args(["llvm-cov", "--version"])
            .output()
            .is_ok_and(|o| o.status.success());
        if !have {
            bail!(
                "cargo-llvm-cov is not installed: `cargo install cargo-llvm-cov --locked` and `rustup component add llvm-tools-preview`, or summarize an existing LCOV file with --from"
            );
        }
        let run = |args: &[String]| -> Result<()> {
            let st = Command::new("cargo")
                .current_dir(&root)
                .args(args)
                .status()
                .context("run cargo llvm-cov")?;
            if !st.success() {
                bail!("cargo {} failed", args.join(" "));
            }
            Ok(())
        };
        run(&["llvm-cov".into(), "clean".into(), "--workspace".into()])?;
        let mut args: Vec<String> = vec!["llvm-cov".into(), "--no-report".into()];
        for c in &a.crates {
            args.push("-p".into());
            args.push(if c.starts_with("openreadout") || c == "xtask" {
                c.clone()
            } else {
                format!("openreadout-{c}")
            });
        }
        run(&args)?;
        if a.corpus {
            run(&[
                "llvm-cov".into(),
                "--no-report".into(),
                "-p".into(),
                "openreadout-corpus-tests".into(),
                "--features".into(),
                "corpus".into(),
            ])?;
        }
        let lcov = out.join("lcov.info");
        run(&[
            "llvm-cov".into(),
            "report".into(),
            "--lcov".into(),
            "--output-path".into(),
            lcov.display().to_string(),
        ])?;
        lcov
    };
    let text = fs::read_to_string(&lcov).with_context(|| format!("read {}", lcov.display()))?;
    let files = parse_lcov(&text);
    if files.is_empty() {
        bail!("{}: no coverage records", lcov.display());
    }
    let md = summary(&files, &root, a.regions);
    let p = out.join("summary.md");
    fs::write(&p, &md)?;
    println!("{md}");
    println!("wrote {}", p.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LCOV: &str = "SF:/w/crates/openreadout-czi/src/container.rs\nFNF:4\nFNH:3\nDA:10,5\nDA:11,0\nDA:12,0\nDA:14,0\nDA:15,1\nend_of_record\nSF:/w/crates/openreadout-core/src/bytes.rs\nFNF:1\nFNH:1\nDA:1,2\nend_of_record\nSF:/home/.cargo/registry/src/x/lib.rs\nDA:1,0\nend_of_record\n";

    #[test]
    fn lcov_is_summed_per_file_and_uncovered_lines_kept() {
        let f = parse_lcov(LCOV);
        let c = &f["/w/crates/openreadout-czi/src/container.rs"];
        assert_eq!(c.lines, (5, 2));
        assert_eq!(c.functions, (4, 3));
        assert_eq!(c.uncovered, [11, 12, 14]);
        assert_eq!(ranges(&c.uncovered), [(11, 12), (14, 14)]);
    }

    #[test]
    fn files_map_to_their_crate() {
        assert_eq!(
            crate_of("/w/crates/openreadout-czi/src/container.rs").as_deref(),
            Some("openreadout-czi")
        );
        assert_eq!(crate_of("/home/.cargo/registry/src/x/lib.rs"), None);
        assert_eq!(crate_of("/w/crates/openreadout-czi/tests/t.rs"), None);
    }

    #[test]
    fn summary_has_rows_and_regions() {
        let s = summary(&parse_lcov(LCOV), &crate::root(), 5);
        assert!(
            s.contains("| `openreadout-czi` (reader) | 5 | 40.0% | 4 | 75.0% |"),
            "{s}"
        );
        assert!(
            s.contains("`crates/openreadout-czi/src/container.rs:11-12` (2 lines)"),
            "{s}"
        );
        assert!(s.contains("| `openreadout-core` | 1 | 100.0% |"), "{s}");
    }
}
