//! `cargo xtask variant intake|status|check`: the maintainer side of the new-variant loop
//! (docs/maintaining.md).
//!
//! A user whose file is refused or not validated runs `openreadout check FILE --report` and files
//! the bundle in a new-variant issue, ideally with a vendor export of the same file and a public
//! deposit. `variant intake` turns what arrives into the pieces a fix needs:
//!
//! - a file (or a data-set directory): copied into the corpus directory under
//!   `intake/<id>/`, a manifest entry (tier `hold` until its licence is confirmed), the vendor
//!   exports as `oracle-export` entries with the same id, an intake record
//!   `corpus/intake/<id>.toml` whose expectations make the corpus test `intake` fail until the
//!   reader handles the variant, and a dated stub in the format's provenance log;
//! - a report bundle alone (no file yet): the bundle under `corpus/intake/<id>.report.json` and a
//!   record with status `awaiting-file`. A bundle is triage information, not a parser input
//!   (clean-room rule 1): the fix waits for a file we may hold.
//!
//! It prints the exact next steps. Held-out files are refused (clean-room rule 11).

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::repo;

/// `cargo xtask variant …`
#[derive(clap::Subcommand)]
#[allow(clippy::large_enum_variant)] // parsed once
pub enum VariantCmd {
    /// Turn a new file (or a report bundle from an issue) into a corpus entry, an intake record
    /// with a failing expectation, a provenance stub and the next steps.
    Intake(IntakeArgs),
    /// List the intake records and what each still needs.
    Status,
    /// CI: every intake record is consistent with the manifest (no held-out file, a known
    /// status, a manifest entry for records with a file).
    Check,
}

#[derive(clap::Args)]
pub struct IntakeArgs {
    /// The file or data-set directory, or an `openreadout-report-*.json` bundle.
    pub input: PathBuf,
    /// A vendor export of the same acquisition (the ground truth): OME-TIFF, mzML, CSV, ...
    /// Repeatable.
    #[arg(long)]
    pub export: Vec<PathBuf>,
    /// Files that belong with the input (a `.wiff.scan`, a SpikeGLX `.meta`, CZI parts, `.ets`
    /// directories): copied next to it under their own names. Repeatable.
    #[arg(long)]
    pub companion: Vec<PathBuf>,
    /// Corpus id. Default `intake-<format>-<first 8 hex digits of its SHA-256>`.
    #[arg(long)]
    pub id: Option<String>,
    /// Format id of the reader that should read it (needed when no reader claims it).
    #[arg(long)]
    pub format: Option<String>,
    /// Public URL of the file (a Zenodo record file, ...).
    #[arg(long)]
    pub url: Option<String>,
    /// Licence (`CC0-1.0`, `CC-BY-4.0`, or "donated with written permission, <date>").
    #[arg(long)]
    pub license: Option<String>,
    /// Depositor / source record ("Zenodo record 1234567: ...").
    #[arg(long)]
    pub source: Option<String>,
    /// The GitHub issue it came from.
    #[arg(long)]
    pub issue: Option<String>,
    /// Manifest tier. Default `hold` (never fetched automatically) until url and licence are
    /// confirmed; then `standard` (or `smoke` for a small file CI should read).
    #[arg(long, default_value = "hold")]
    pub tier: String,
    /// The `openreadout` binary used to inspect the file.
    #[arg(long, default_value = "target/debug/openreadout")]
    pub bin: PathBuf,
    /// Show what would be written, write nothing.
    #[arg(long)]
    pub dry_run: bool,
}

/// `corpus/intake/<id>.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub format: String,
    /// `open` (the reader does not handle it yet), `awaiting-file` (only a bundle), `validated`
    /// (fixed; the expectations now pin it), `closed` (won't fix, duplicate, ...).
    pub status: String,
    pub opened: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub issue: String,
    /// `file` or `bundle`.
    pub input: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sha256: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fingerprint: String,
    /// Assurance level when it arrived (`unreadable` when info failed).
    pub level_at_intake: String,
    /// Variant features not validated at intake (`kind=value (status; scope)`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unvalidated: Vec<String>,
    /// Decode stages that failed at intake (`stage: code: message`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failing_stages: Vec<String>,
    /// Manifest filenames of the vendor exports.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exports: Vec<String>,
    /// What the corpus test `intake` requires of the file.
    pub expect: Expect,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

/// The expectations the corpus test `intake` checks (they fail at intake and pass once fixed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Expect {
    /// `info` must succeed.
    pub opens: bool,
    /// The lowest acceptable assurance level: `validated`, `partially_validated` or
    /// `unvalidated`.
    pub min_level: String,
    /// `corpus/oracle/<id>.json` must exist (ground truth generated from the vendor export or
    /// an independent reader).
    pub oracle: bool,
}

impl Default for Expect {
    fn default() -> Self {
        Expect {
            opens: true,
            min_level: "partially_validated".into(),
            oracle: true,
        }
    }
}

const STATUSES: [&str; 4] = ["open", "awaiting-file", "validated", "closed"];

pub fn run(cmd: VariantCmd) -> Result<()> {
    let root = crate::root();
    match cmd {
        VariantCmd::Intake(a) => intake(&root, &crate::corpus_dir(), &a),
        VariantCmd::Status => status(&root),
        VariantCmd::Check => check(&root),
    }
}

fn records(root: &Path) -> Result<Vec<(PathBuf, Record)>> {
    let dir = root.join("corpus/intake");
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    for e in fs::read_dir(&dir)? {
        let p = e?.path();
        if p.extension().is_some_and(|x| x == "toml") {
            let text = fs::read_to_string(&p)?;
            let r: Record =
                toml::from_str(&text).with_context(|| format!("parse {}", p.display()))?;
            out.push((p, r));
        }
    }
    out.sort_by(|a, b| a.1.id.cmp(&b.1.id));
    Ok(out)
}

fn status(root: &Path) -> Result<()> {
    let rs = records(root)?;
    if rs.is_empty() {
        println!("no intake records (corpus/intake/)");
        return Ok(());
    }
    for (_, r) in rs {
        println!(
            "{:<40} {:<14} {:<22} at intake: {}{}",
            r.id,
            r.status,
            r.format,
            r.level_at_intake,
            if r.issue.is_empty() {
                String::new()
            } else {
                format!("  {}", r.issue)
            }
        );
        for u in r.unvalidated.iter().chain(&r.failing_stages) {
            println!("    {u}");
        }
    }
    Ok(())
}

fn check(root: &Path) -> Result<()> {
    let manifest = repo::manifest(root)?;
    let mut problems = Vec::new();
    let rs = records(root)?;
    for (p, r) in &rs {
        let name = p.file_name().unwrap_or_default().to_string_lossy();
        if name != format!("{}.toml", r.id) {
            problems.push(format!("{name}: id {} does not match the file name", r.id));
        }
        if !STATUSES.contains(&r.status.as_str()) {
            problems.push(format!(
                "{}: unknown status {:?} ({STATUSES:?})",
                r.id, r.status
            ));
        }
        if !matches!(
            r.expect.min_level.as_str(),
            "validated" | "partially_validated" | "unvalidated"
        ) {
            problems.push(format!(
                "{}: unknown expect.min_level {:?}",
                r.id, r.expect.min_level
            ));
        }
        let entries: Vec<&repo::Entry> = manifest.iter().filter(|e| e.id == r.id).collect();
        if entries.iter().any(|e| e.tier == crate::heldout::TIER) {
            problems.push(format!(
                "{}: a held-out entry cannot be an intake (clean-room rule 11)",
                r.id
            ));
        }
        if r.input == "file"
            && !entries
                .iter()
                .any(|e| e.role == "input" || e.role.is_empty())
        {
            problems.push(format!("{}: no manifest input entry with this id", r.id));
        }
        if r.status == "validated" && entries.iter().any(|e| e.tier == "hold") {
            problems.push(format!(
                "{}: validated, but its manifest entry is still on tier hold (confirm url and licence, move it to standard)",
                r.id
            ));
        }
        if r.input == "bundle" && r.status == "open" {
            problems.push(format!(
                "{}: a bundle alone cannot be open (status awaiting-file until a file arrives)",
                r.id
            ));
        }
    }
    if !problems.is_empty() {
        bail!("intake records:\n  {}", problems.join("\n  "));
    }
    println!("intake: {} records consistent", rs.len());
    Ok(())
}

fn sha256_path(p: &Path) -> Result<String> {
    if p.is_file() {
        return Ok(crate::sha256_file(p)?.0);
    }
    // a directory: SHA-256 over `relative path\0sha256\n` of every file, sorted
    let mut files = Vec::new();
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d)? {
            let q = e?.path();
            if q.is_dir() {
                stack.push(q);
            } else {
                files.push(q);
            }
        }
    }
    files.sort();
    let mut h = Sha256::new();
    for f in files {
        let rel = f
            .strip_prefix(p)
            .unwrap_or(&f)
            .to_string_lossy()
            .replace('\\', "/");
        h.update(rel.as_bytes());
        h.update([0]);
        h.update(crate::sha256_file(&f)?.0.as_bytes());
        h.update(b"\n");
    }
    Ok(hex::encode(h.finalize()))
}

fn run_json(bin: &Path, args: &[&str], input: &Path) -> Result<serde_json::Value> {
    let out = Command::new(bin)
        .args(args)
        .arg(input)
        .env_remove("OPENREADOUT_STRICT")
        .output()
        .with_context(|| {
            format!(
                "run {} (build it: cargo build -p openreadout)",
                bin.display()
            )
        })?;
    serde_json::from_slice(&out.stdout).with_context(|| {
        format!(
            "{} {}: not JSON: {}",
            bin.display(),
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    if src.is_dir() {
        fs::create_dir_all(dst)?;
        for e in fs::read_dir(src)? {
            let e = e?;
            copy_tree(&e.path(), &dst.join(e.file_name()))?;
        }
    } else {
        if let Some(p) = dst.parent() {
            fs::create_dir_all(p)?;
        }
        fs::copy(src, dst)
            .with_context(|| format!("copy {} to {}", src.display(), dst.display()))?;
    }
    Ok(())
}

fn q(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

/// One file an intake adds to the manifest.
struct NewEntry<'a> {
    id: &'a str,
    format: &'a str,
    /// `input` or `oracle-export`.
    role: &'a str,
    /// Relative to the corpus directory.
    filename: &'a str,
    /// Empty for a directory.
    sha: &'a str,
    size: u64,
    note: &'a str,
}

/// The manifest entry for an intake file; tier, URL, licence and source come from the intake
/// arguments.
fn manifest_entry(e: &NewEntry<'_>, a: &IntakeArgs) -> String {
    let mut s = String::from("\n[[file]]\n");
    let _ = writeln!(s, "id = {}", q(e.id));
    let _ = writeln!(s, "format = {}", q(e.format));
    let _ = writeln!(s, "role = {}", q(e.role));
    let _ = writeln!(s, "tier = {}", q(&a.tier));
    let _ = writeln!(s, "filename = {}", q(e.filename));
    let _ = writeln!(s, "url = {}", q(a.url.as_deref().unwrap_or("")));
    if !e.sha.is_empty() {
        let _ = writeln!(s, "sha256 = {}", q(e.sha));
    }
    if e.size > 0 {
        let _ = writeln!(s, "size = {}", e.size);
    }
    let _ = writeln!(
        s,
        "license = {}",
        q(a.license
            .as_deref()
            .unwrap_or("UNCONFIRMED: set before moving off tier hold"))
    );
    let _ = writeln!(
        s,
        "source = {}",
        q(a.source
            .as_deref()
            .unwrap_or("UNCONFIRMED: depositor and source record"))
    );
    let _ = writeln!(s, "notes = {}", q(e.note));
    s
}

fn level_rank(l: &str) -> u8 {
    match l {
        "validated" => 3,
        "partially_validated" => 2,
        "unvalidated" => 1,
        _ => 0,
    }
}

/// What a report says about the variant: (format, fingerprint, level, unvalidated features,
/// failing stages, input sha256).
struct Seen {
    format: Option<String>,
    fingerprint: String,
    level: String,
    unvalidated: Vec<String>,
    failing: Vec<String>,
}

fn seen(report: &serde_json::Value) -> Seen {
    let a = &report["assurance"];
    let unvalidated = a["variant"]
        .as_array()
        .map(|v| {
            v.iter()
                .filter(|f| f["status"] != "validated")
                .map(|f| {
                    let scope: Vec<&str> = f["scope"]
                        .as_array()
                        .map(|s| s.iter().filter_map(|x| x.as_str()).collect())
                        .unwrap_or_default();
                    format!(
                        "{}={} ({}; {})",
                        f["kind"].as_str().unwrap_or_default(),
                        f["value"].as_str().unwrap_or_default(),
                        f["status"].as_str().unwrap_or_default(),
                        if scope.is_empty() {
                            "descriptive".to_string()
                        } else {
                            scope.join(",")
                        }
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let mut unvalidated: Vec<String> = unvalidated;
    for u in a["undecoded"].as_array().into_iter().flatten() {
        unvalidated.push(format!(
            "undecoded {}: {}",
            u["structure"].as_str().unwrap_or_default(),
            u["detail"].as_str().unwrap_or_default()
        ));
    }
    let failing = report["stages"]
        .as_array()
        .map(|v| {
            v.iter()
                .filter(|s| s["status"] == "error" || s["status"] == "panic")
                .map(|s| {
                    format!(
                        "{}: {}: {}",
                        s["stage"].as_str().unwrap_or_default(),
                        s["error"]["code"].as_str().unwrap_or_default(),
                        s["error"]["message"].as_str().unwrap_or_default()
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let info_ok = report["stages"].as_array().is_some_and(|v| {
        v.iter()
            .any(|s| s["stage"] == "info" && s["status"] == "ok")
    });
    Seen {
        format: report["detection"]["format"].as_str().map(str::to_string),
        fingerprint: a["fingerprint"].as_str().unwrap_or_default().to_string(),
        level: if info_ok {
            a["level"].as_str().unwrap_or("unvalidated").to_string()
        } else {
            "unreadable".into()
        },
        unvalidated,
        failing,
    }
}

fn provenance_stub(r: &Record, filename: &str, a: &IntakeArgs) -> String {
    let mut s = format!(
        "\n## {} — intake `{}` (new variant, {})\n\n",
        r.opened, r.id, r.status
    );
    let _ = writeln!(
        s,
        "**Corpus files used:** `{}` (`{filename}`; {}; {}){}",
        r.id,
        a.license.as_deref().unwrap_or("licence UNCONFIRMED"),
        a.source.as_deref().unwrap_or("source UNCONFIRMED"),
        if r.exports.is_empty() {
            String::new()
        } else {
            format!(
                " and its vendor export{} {} (`oracle-export`, same id)",
                if r.exports.len() > 1 { "s" } else { "" },
                r.exports
                    .iter()
                    .map(|e| format!("`{e}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    );
    if !r.issue.is_empty() {
        let _ = writeln!(s, "\n**Reported in:** {}", r.issue);
    }
    let _ = writeln!(
        s,
        "\n**Variant at intake:** `{}` — {}.",
        if r.fingerprint.is_empty() {
            "(no fingerprint: info failed)"
        } else {
            &r.fingerprint
        },
        r.level_at_intake
    );
    for u in r.unvalidated.iter().chain(&r.failing_stages) {
        let _ = writeln!(s, "- {u}");
    }
    s.push_str(
        "\n**Prior art consulted:** TODO before changing the parser (URL + licence of each document or permissively licensed reader; \"none\" if none).\n\n**What was inferred from what:** TODO (which bytes of which file, compared with which export or oracle).\n",
    );
    s
}

/// What intake asks the CLI (`check --report`, `info --view format`), as JSON.
type Cli<'a> = &'a dyn Fn(&[&str], &Path) -> Result<serde_json::Value>;

fn intake(root: &Path, corpus: &Path, a: &IntakeArgs) -> Result<()> {
    let bin = if a.bin.is_absolute() {
        a.bin.clone()
    } else {
        root.join(&a.bin)
    };
    intake_with(root, corpus, a, &|args, p| run_json(&bin, args, p))
}

fn intake_with(root: &Path, corpus: &Path, a: &IntakeArgs, cli: Cli<'_>) -> Result<()> {
    let input = &a.input;
    if !input.exists() {
        bail!("{} does not exist", input.display());
    }
    let manifest = repo::manifest(root)?;
    let crates = repo::reader_crates(root)?;
    let bundle = read_bundle(input);
    let sha = if let Some(b) = &bundle {
        b["input"]["sha256"]
            .as_str()
            .or(b["input"]["sha256_first_64mib"].as_str())
            .map_or_else(
                || crate::sha256_file(input).map(|x| x.0),
                |s| Ok(s.to_string()),
            )?
    } else {
        sha256_path(input)?
    };
    refuse_known(&manifest, &sha)?;
    let report = if let Some(b) = &bundle {
        b.clone()
    } else {
        let v = cli(
            &["check", "--report", "--dry-run", "--json", "--include-text"],
            input,
        )?;
        let r = v["data"]["report"].clone();
        if !r.is_object() {
            bail!(
                "openreadout check --report failed on {}: {v}",
                input.display()
            );
        }
        r
    };
    let s = seen(&report);
    let format = a
        .format
        .clone()
        .or(s.format.clone())
        .context("no reader claimed the input: pass --format <id> (the reader that should read it; `openreadout self formats`)")?;
    if !crates.iter().any(|c| c.formats.contains(&format)) {
        bail!("unknown format id `{format}` (see `openreadout self formats`)");
    }
    let id =
        a.id.clone()
            .unwrap_or_else(|| format!("intake-{format}-{}", &sha[..8.min(sha.len())]));
    let existing = records(root)?.into_iter().find(|(_, r)| r.id == id);
    if manifest.iter().any(|e| e.id == id) {
        bail!("corpus id `{id}` is taken; pass --id");
    }
    let mut record = existing
        .as_ref()
        .map(|(_, r)| r.clone())
        .unwrap_or_default();
    record.id.clone_from(&id);
    record.format.clone_from(&format);
    if record.opened.is_empty() {
        record.opened = repo::today();
    }
    if let Some(i) = &a.issue {
        record.issue.clone_from(i);
    }
    record.sha256.clone_from(&sha);
    record.fingerprint.clone_from(&s.fingerprint);
    record.level_at_intake.clone_from(&s.level);
    record.unvalidated.clone_from(&s.unvalidated);
    record.failing_stages.clone_from(&s.failing);
    if bundle.is_some() {
        intake_bundle(root, a, record)
    } else {
        intake_file(root, corpus, a, cli, &crates, record)
    }
}

/// The input as a `check --report` bundle (alone or in its JSON envelope), if it is one.
fn read_bundle(input: &Path) -> Option<serde_json::Value> {
    if !(input.is_file() && input.extension().is_some_and(|e| e == "json")) {
        return None;
    }
    fs::read(input)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .map(|v| {
            if v["data"]["report"].is_object() {
                v["data"]["report"].clone()
            } else {
                v
            }
        })
        .filter(|v| v["report_version"].is_string())
}

/// Clean-room rule 11 and duplicates, checked before any reader touches the file.
fn refuse_known(manifest: &[repo::Entry], sha: &str) -> Result<()> {
    let Some(e) = manifest
        .iter()
        .find(|e| !e.sha256.is_empty() && e.sha256 == sha)
    else {
        return Ok(());
    };
    if e.tier == crate::heldout::TIER {
        bail!(
            "this file is the held-out corpus file `{}`: held-out files are never inputs to a parser (clean-room rule 11). A failure found on it is fixed on a NEW file from another source (docs/benchmark/heldout.md).",
            e.id
        );
    }
    bail!(
        "this file is already in the corpus as `{}` ({})",
        e.id,
        e.filename
    );
}

/// A report bundle without the file: keep the bundle for triage and wait for the file.
fn intake_bundle(root: &Path, a: &IntakeArgs, mut record: Record) -> Result<()> {
    let id = record.id.clone();
    let intake_dir = root.join("corpus/intake");
    record.input = "bundle".into();
    record.status = "awaiting-file".into();
    record.expect = Expect::default();
    let dst = intake_dir.join(format!("{id}.report.json"));
    let plan = vec![
        format!("copy the bundle to {}", rel(root, &dst)),
        format!(
            "write {}",
            rel(root, &intake_dir.join(format!("{id}.toml")))
        ),
    ];
    if !a.dry_run {
        fs::create_dir_all(&intake_dir)?;
        fs::copy(&a.input, &dst)?;
        fs::write(
            intake_dir.join(format!("{id}.toml")),
            toml::to_string_pretty(&record)?,
        )?;
    }
    print_plan(&plan, a.dry_run);
    println!(
        "\nNext steps for {id} ({}; a bundle, no file yet):",
        record.format
    );
    println!(
        "  1. Triage from the bundle: {} (stages, fingerprint, structure).",
        rel(root, &dst)
    );
    println!(
        "     It is triage information, not a parser input (clean-room rule 1): no parser change is derived from it."
    );
    println!(
        "  2. Ask the reporter (issue template, docs/maintaining.md § Getting a file) for a public deposit"
    );
    println!(
        "     (Zenodo, CC0/CC-BY) or a file donated with written permission, plus the vendor export."
    );
    println!(
        "  3. When it arrives: cargo xtask variant intake FILE --id {id} --export EXPORT --url URL --license LICENCE --source SOURCE"
    );
    Ok(())
}

/// A file or data-set directory: copy it (with companions and vendor exports) into the corpus,
/// append manifest entries, write the intake record and a provenance stub.
fn intake_file(
    root: &Path,
    corpus: &Path,
    a: &IntakeArgs,
    cli: Cli<'_>,
    crates: &[repo::ReaderCrate],
    mut record: Record,
) -> Result<()> {
    let input = &a.input;
    let id = record.id.clone();
    let format = record.format.clone();
    let intake_dir = root.join("corpus/intake");
    let doc = repo::doc_of(root, crates, &format);
    let krate = repo::crate_of(crates, &format).map(|c| c.name.clone());
    let mut plan: Vec<String> = Vec::new();
    record.input = "file".into();
    if record.status.is_empty() || record.status == "awaiting-file" {
        record.status = "open".into();
    }
    let name = input
        .file_name()
        .context("input has no file name")?
        .to_string_lossy()
        .into_owned();
    let rel_dir = format!("intake/{id}");
    let filename = format!("{rel_dir}/{name}");
    let dest_dir = corpus.join(&rel_dir);
    plan.push(format!(
        "copy {} to {}",
        input.display(),
        dest_dir.join(&name).display()
    ));
    for c in &a.companion {
        plan.push(format!("copy companion {} next to it", c.display()));
    }
    let size = if input.is_file() {
        fs::metadata(input)?.len()
    } else {
        0
    };
    let note = format!(
        "intake {} ({}): {}; {} at intake",
        record.opened,
        if record.issue.is_empty() {
            "no issue"
        } else {
            &record.issue
        },
        if record.fingerprint.is_empty() {
            "no fingerprint"
        } else {
            &record.fingerprint
        },
        record.level_at_intake
    );
    let mut entries = manifest_entry(
        &NewEntry {
            id: &id,
            format: &format,
            role: "input",
            filename: &filename,
            sha: if input.is_file() { &record.sha256 } else { "" },
            size,
            note: &note,
        },
        a,
    );
    record.exports.clear();
    for ex in &a.export {
        let (entry, efile) = export_entry(&id, &rel_dir, ex, a, cli)?;
        entries.push_str(&entry);
        record.exports.push(efile);
        plan.push(format!("copy export {} next to it", ex.display()));
    }
    let manifest_path = root.join("corpus/manifest.toml");
    plan.push(format!(
        "append {} entr{} to corpus/manifest.toml",
        1 + a.export.len(),
        if a.export.is_empty() { "y" } else { "ies" }
    ));
    record.expect = Expect {
        opens: true,
        min_level: if level_rank(&record.level_at_intake) >= level_rank("partially_validated") {
            "validated".into()
        } else {
            "partially_validated".into()
        },
        oracle: true,
    };
    plan.push(format!(
        "write corpus/intake/{id}.toml (expects: opens, level >= {}, oracle)",
        record.expect.min_level
    ));
    let prov = doc
        .as_ref()
        .map(|d| root.join(format!("docs/provenance/{d}.md")));
    if let Some(p) = &prov {
        plan.push(format!("append a dated stub to {}", rel(root, p)));
    }
    if !a.dry_run {
        copy_tree(input, &dest_dir.join(&name))?;
        for c in a.companion.iter().chain(&a.export) {
            let n = c.file_name().context("no file name")?;
            copy_tree(c, &dest_dir.join(n))?;
        }
        let mut m = fs::read_to_string(&manifest_path)?;
        if !m.ends_with('\n') {
            m.push('\n');
        }
        m.push_str(&entries);
        fs::write(&manifest_path, m)?;
        fs::create_dir_all(&intake_dir)?;
        fs::write(
            intake_dir.join(format!("{id}.toml")),
            toml::to_string_pretty(&record)?,
        )?;
        if let Some(p) = &prov {
            let mut t = fs::read_to_string(p).unwrap_or_default();
            t.push_str(&provenance_stub(&record, &filename, a));
            fs::write(p, t)?;
        }
    }
    print_plan(&plan, a.dry_run);
    next_steps(
        root,
        &record,
        &filename,
        krate.as_deref(),
        doc.as_deref(),
        a,
    );
    Ok(())
}

/// The manifest entry of one vendor export (an `oracle-export` sharing the input's id), and
/// its path relative to the corpus directory.
fn export_entry(
    id: &str,
    rel_dir: &str,
    ex: &Path,
    a: &IntakeArgs,
    cli: Cli<'_>,
) -> Result<(String, String)> {
    let ename = ex
        .file_name()
        .context("export has no file name")?
        .to_string_lossy()
        .into_owned();
    let efile = format!("{rel_dir}/{ename}");
    let det = cli(&["info", "--view", "format", "--json"], ex).ok();
    let eformat = det
        .as_ref()
        .and_then(|d| d["data"]["format"].as_str())
        .map_or_else(
            || {
                ex.extension()
                    .map(|e| e.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default()
            },
            str::to_string,
        );
    let (esha, esize) = if ex.is_file() {
        crate::sha256_file(ex)?
    } else {
        (String::new(), 0)
    };
    let entry = manifest_entry(
        &NewEntry {
            id,
            format: &eformat,
            role: "oracle-export",
            filename: &efile,
            sha: &esha,
            size: esize,
            note: &format!("vendor export of `{id}` (the ground truth for its oracle)"),
        },
        a,
    );
    Ok((entry, efile))
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).display().to_string()
}

fn print_plan(plan: &[String], dry: bool) {
    println!("{}", if dry { "would:" } else { "did:" });
    for p in plan {
        println!("  - {p}");
    }
}

/// Oracle commands this format's notes and provenance log already use (`uv run ...`).
fn oracle_commands(root: &Path, doc: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let Some(doc) = doc else { return out };
    for p in [
        root.join(format!("docs/formats/{doc}.md")),
        root.join(format!("docs/provenance/{doc}.md")),
    ] {
        let Ok(t) = fs::read_to_string(p) else {
            continue;
        };
        for line in t.lines() {
            for (i, _) in line.match_indices("uv run") {
                let tail = &line[i..];
                let end = tail.find('`').unwrap_or(tail.len());
                let cmd = tail[..end].trim().to_string();
                if cmd.len() > 12 && !out.contains(&cmd) {
                    out.push(cmd);
                }
            }
        }
    }
    out.truncate(6);
    out
}

fn next_steps(
    root: &Path,
    r: &Record,
    filename: &str,
    krate: Option<&str>,
    doc: Option<&str>,
    a: &IntakeArgs,
) {
    let id = &r.id;
    let path = format!("../corpus/files/{filename}");
    println!(
        "\nNext steps for {id} ({}; crate openreadout-{}):",
        r.format,
        krate.unwrap_or("?")
    );
    if r.unvalidated.is_empty() && r.failing_stages.is_empty() {
        println!(
            "  (at intake: {}, nothing unvalidated; the intake pins it once its oracle exists)",
            r.level_at_intake
        );
    } else {
        println!("  at intake: {} — {}", r.level_at_intake, r.fingerprint);
        for u in r.unvalidated.iter().chain(&r.failing_stages) {
            println!("    {u}");
        }
    }
    let mut n = 0;
    let mut step = |s: String| {
        n += 1;
        println!("  {n}. {s}");
    };
    if a.url.is_none() || a.license.is_none() || a.tier == "hold" {
        step(format!(
            "Licence: set url, license and source of `{id}` in corpus/manifest.toml (a public deposit, or \"donated with written permission, <date>\"), then move its tier from hold to standard (smoke if small and CI should read it). Nothing medical or identifying (clean-room rule 10)."
        ));
    }
    let export = r
        .exports
        .first()
        .map(|e| format!("--export ../corpus/files/{e} "))
        .unwrap_or_default();
    step(format!(
        "Ground truth: cd oracle && uv run python gen.py {export}--id {id} {path}{}",
        if r.exports.is_empty() {
            "   (no vendor export: ask for one in the issue; without an independent reading the variant can only reach `seen`)"
        } else {
            ""
        }
    ));
    let cmds = oracle_commands(root, doc);
    if !cmds.is_empty() {
        println!("     commands this format's notes use:");
        for c in cmds {
            println!("       {c}");
        }
    }
    step(format!(
        "Reproduce: cargo test -p openreadout-corpus-tests --features corpus --test intake (fails now: expects opens, level >= {}, an oracle); inspect with openreadout info --view structure|full or openreadout check --report: {}",
        r.expect.min_level,
        crate::corpus_dir().join(filename).display()
    ));
    if let Some(k) = krate {
        step(format!(
            "Read crates/openreadout-{k}/MAINTAINING.md (decode pipeline, where variants branch, fragile spots)."
        ));
    }
    if let Some(d) = doc {
        step(format!(
            "Provenance: complete the stub appended to docs/provenance/{d}.md (prior art, what was inferred from what) BEFORE changing the parser; new public names go in the vocabulary table of docs/formats/{d}.md (cargo xtask vocab-check)."
        ));
    }
    step(format!(
        "Fix the reader, then: CORPUS_ONLY={id} cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test corpus corpus_matches_oracle -- --nocapture"
    ));
    step("Evidence: cargo build --release -p openreadout && CORPUS_RESULTS=/tmp/results.jsonl cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test corpus corpus_matches_oracle && cargo xtask assurance-audit refresh --results /tmp/results.jsonl && cargo xtask assurance-audit --write".into());
    step("Regressions: cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test snapshots, then cargo xtask snapshot review / accept (every changed output on every corpus file is shown).".into());
    step(format!(
        "Close: set status = \"validated\" in corpus/intake/{id}.toml (the intake test then pins the variant), add a CHANGELOG line, and link the PR in the issue."
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> serde_json::Value {
        serde_json::json!({
            "report_version": "1",
            "input": {"sha256": "ab".repeat(32)},
            "detection": {"format": "czi"},
            "stages": [
                {"stage": "detect", "status": "ok"},
                {"stage": "info", "status": "ok"},
                {"stage": "first_read", "status": "error",
                 "error": {"code": "unsupported_feature", "message": "czi: unsupported feature: codec 7"}}
            ],
            "assurance": {
                "level": "unvalidated",
                "fingerprint": "czi|codec=jpeg_xl|format_version=1.0",
                "variant": [
                    {"kind": "codec", "value": "jpeg_xl", "scope": ["pixels"], "status": "unseen"},
                    {"kind": "format_version", "value": "1.0", "scope": ["metadata"], "status": "validated"}
                ],
                "undecoded": [{"structure": "subblock 9", "detail": "unknown"}]
            }
        })
    }

    #[test]
    fn a_report_names_what_is_unvalidated_and_what_failed() {
        let s = seen(&report());
        assert_eq!(s.format.as_deref(), Some("czi"));
        assert_eq!(s.level, "unvalidated");
        assert_eq!(
            s.unvalidated,
            [
                "codec=jpeg_xl (unseen; pixels)",
                "undecoded subblock 9: unknown"
            ]
        );
        assert_eq!(
            s.failing,
            ["first_read: unsupported_feature: czi: unsupported feature: codec 7"]
        );
    }

    #[derive(Deserialize)]
    struct M {
        file: Vec<repo::Entry>,
    }

    #[test]
    fn records_round_trip_and_manifest_entries_parse() {
        let r = Record {
            id: "intake-czi-abababab".into(),
            format: "czi".into(),
            status: "open".into(),
            opened: "2026-09-26".into(),
            input: "file".into(),
            level_at_intake: "unvalidated".into(),
            unvalidated: vec!["codec=jpeg_xl (unseen; pixels)".into()],
            ..Record::default()
        };
        let text = toml::to_string_pretty(&r).unwrap();
        let back: Record = toml::from_str(&text).unwrap();
        assert_eq!(back.id, r.id);
        assert_eq!(back.expect.min_level, "partially_validated");
        let a = IntakeArgs {
            input: "x.czi".into(),
            export: vec![],
            companion: vec![],
            id: None,
            format: None,
            url: None,
            license: None,
            source: None,
            issue: None,
            tier: "hold".into(),
            bin: "b".into(),
            dry_run: true,
        };
        let e = manifest_entry(
            &NewEntry {
                id: "intake-czi-abababab",
                format: "czi",
                role: "input",
                filename: "intake/x/a \"b\".czi",
                sha: "00",
                size: 5,
                note: "n",
            },
            &a,
        );
        let m: M = toml::from_str(&e).unwrap();
        assert_eq!(m.file[0].filename, "intake/x/a \"b\".czi");
        assert_eq!(m.file[0].tier, "hold");
        let stub = provenance_stub(&r, "intake/x/a.czi", &a);
        assert!(stub.contains("intake `intake-czi-abababab`"));
        assert!(stub.contains("Prior art consulted:** TODO"));
    }

    /// A throwaway repository with one ND2 reader crate, its notes and a manifest.
    fn fake_repo() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        for d in [
            "crates/openreadout-nd2/src",
            "docs/formats",
            "docs/provenance",
            "corpus/files",
        ] {
            fs::create_dir_all(r.join(d)).unwrap();
        }
        fs::write(
            r.join("crates/openreadout-nd2/src/assurance.rs"),
            "static P: AssuranceProfile = AssuranceProfile {\n    format_id: \"nd2\",\n};\n",
        )
        .unwrap();
        fs::write(r.join("docs/formats/nd2.md"), "# ND2\n").unwrap();
        fs::write(r.join("docs/provenance/nd2.md"), "# Provenance log — ND2\n").unwrap();
        fs::write(
            r.join("corpus/manifest.toml"),
            format!(
                "[[file]]\nid = \"ho-x\"\nformat = \"nd2\"\nrole = \"heldout\"\ntier = \"heldout\"\nfilename = \"heldout/x.nd2\"\nsha256 = \"{}\"\n",
                "cd".repeat(32)
            ),
        )
        .unwrap();
        t
    }

    fn args(input: PathBuf, export: Vec<PathBuf>) -> IntakeArgs {
        IntakeArgs {
            input,
            export,
            companion: vec![],
            id: None,
            format: None,
            url: None,
            license: None,
            source: None,
            issue: Some("https://example.org/issues/7".into()),
            tier: "hold".into(),
            bin: "unused".into(),
            dry_run: false,
        }
    }

    #[test]
    fn intake_writes_entry_record_stub_and_copies() {
        let repo = fake_repo();
        let root = repo.path();
        let src = tempfile::tempdir().unwrap();
        let file = src.path().join("new.nd2");
        fs::write(&file, b"not really an nd2").unwrap();
        let export = src.path().join("new.ome.tiff");
        fs::write(&export, b"export").unwrap();
        let cli = |args: &[&str], _: &Path| -> Result<serde_json::Value> {
            Ok(if args.contains(&"--report") {
                serde_json::json!({"data": {"report": report()}})
            } else {
                serde_json::json!({"data": {"format": "tiff"}})
            })
        };
        let corpus = root.join("corpus/files");
        let mut a = args(file.clone(), vec![export]);
        // the report says czi; the maintainer names the reader that should read it
        a.format = Some("nd2".into());
        intake_with(root, &corpus, &a, &cli).unwrap();
        let sha = crate::sha256_file(&file).unwrap().0;
        let id = format!("intake-nd2-{}", &sha[..8]);
        // the file and its export are copied, the manifest has both entries
        assert!(corpus.join(format!("intake/{id}/new.nd2")).exists());
        assert!(corpus.join(format!("intake/{id}/new.ome.tiff")).exists());
        let m = repo::manifest(root).unwrap();
        let mine: Vec<_> = m.iter().filter(|e| e.id == id).collect();
        assert_eq!(mine.len(), 2);
        assert_eq!(mine[0].role, "input");
        assert_eq!(mine[0].tier, "hold");
        assert_eq!(mine[0].sha256, sha);
        assert_eq!(mine[1].role, "oracle-export");
        assert_eq!(mine[1].format, "tiff");
        // the record holds what failed and the expectations
        let r: Record = toml::from_str(
            &fs::read_to_string(root.join(format!("corpus/intake/{id}.toml"))).unwrap(),
        )
        .unwrap();
        assert_eq!(r.status, "open");
        assert_eq!(r.level_at_intake, "unvalidated");
        assert!(r.expect.opens && r.expect.oracle);
        assert_eq!(r.expect.min_level, "partially_validated");
        assert_eq!(r.issue, "https://example.org/issues/7");
        check(root).unwrap();
        let log = fs::read_to_string(root.join("docs/provenance/nd2.md")).unwrap();
        assert!(log.contains(&format!("intake `{id}`")), "{log}");
        assert!(log.contains("codec=jpeg_xl (unseen; pixels)"), "{log}");
    }

    #[test]
    fn intake_refuses_held_out_files_and_duplicates() {
        let repo = fake_repo();
        let root = repo.path();
        let src = tempfile::tempdir().unwrap();
        let file = src.path().join("x.nd2");
        fs::write(&file, b"held out").unwrap();
        let sha = crate::sha256_file(&file).unwrap().0;
        let text = fs::read_to_string(root.join("corpus/manifest.toml")).unwrap();
        fs::write(
            root.join("corpus/manifest.toml"),
            text.replace(&"cd".repeat(32), &sha),
        )
        .unwrap();
        let never = |_: &[&str], _: &Path| -> Result<serde_json::Value> {
            panic!("no reader may run on a held-out file")
        };
        let err = intake_with(
            root,
            &root.join("corpus/files"),
            &args(file, vec![]),
            &never,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("clean-room rule 11"), "{err}");
    }

    #[test]
    fn the_committed_records_are_consistent() {
        check(&crate::root()).unwrap();
    }
}
