//! `cargo xtask assurance-audit`: derive every reader's validated-variant table and confidence
//! level from the development corpus, and check the declared ones match (docs/assurance.md).
//!
//! Two steps:
//!
//! 1. `refresh` runs the built CLI's `info --json` on every development-corpus input present on
//!    disk (never a held-out file), records the variant features each file's assurance profile
//!    observes, joins the corpus test's per-file oracle results (`CORPUS_RESULTS` from
//!    `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test corpus
//!    corpus_matches_oracle`) and writes `corpus/assurance/evidence.json` (committed).
//! 2. The audit (no argument; CI-able, needs no corpus) recomputes from that evidence the
//!    table between the `BEGIN GENERATED <format>` / `END GENERATED <format>` markers of every
//!    `crates/*/src/assurance.rs` and the confidence rubric, and fails when a table, a
//!    confidence or the published evidence page (`book/src/project/evidence.md`) is stale.
//!    `--write` rewrites them.
//!
//! A feature value is *validated* when a development file with that value matched an
//! independent oracle on at least one of the outputs the feature affects (descriptive features,
//! with no scope, need a passing file); *seen* when files with it were only read.
//!
//! A second evidence class, `vendor_stored_result`, comes from results the vendor software
//! computed from the raw data and stored in the file (Chromeleon's stored peaks), reproduced
//! from our decoded values within a tight tolerance (results lines with `"evidence":
//! "vendor_stored_result"`). The vendor software is an independent implementation that read
//! the same raw data, so such a file validates the features whose scope the results constrain
//! (`traces`: the values, time axis and scaling of the signals they were computed from) and
//! counts as a confirmed file in the rubric; it validates nothing descriptive, no metadata and no
//! tracked field.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const EVIDENCE: &str = "corpus/assurance/evidence.json";
const BOOK_PAGE: &str = "book/src/project/evidence.md";

#[derive(Debug, Deserialize)]
struct Manifest {
    file: Vec<Entry>,
}

#[derive(Debug, Deserialize, Clone)]
struct Entry {
    id: String,
    format: String,
    #[serde(default)]
    tier: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    filename: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    source: String,
}

/// One feature as the evidence file stores it: `[kind, value, [scope...]]`.
type FeatureRow = (String, String, Vec<String>);

/// One development-corpus file.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileEvidence {
    id: String,
    /// Format id of the reader that read it (the profile's id).
    format: String,
    /// Depositor key (source record), for counting independent sources.
    source: String,
    /// Written by a tool on our side (synthetic fixtures): validates, but is no depositor.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    synthetic: bool,
    /// Corpus-test status: `pass`, `FAIL`, `skip`, `oracle-error`, `none` (no oracle),
    /// `unreadable` (`info` failed).
    oracle: String,
    #[serde(default)]
    independent: bool,
    /// Outputs the oracle comparison covered.
    #[serde(default)]
    compared: Vec<String>,
    /// Tracked normalized fields the comparison checked (core `TRACKED_FIELDS`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    fields: Vec<String>,
    /// Evidence class `vendor_stored_result`: `pass` or `FAIL` when results the vendor software
    /// stored in the file were recomputed from our decoded values (none: not applicable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stored_result: Option<String>,
    /// Outputs those stored results constrain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    stored_result_compared: Vec<String>,
    /// The (kind, value) features of the signals the stored results were computed from, when
    /// the results line names them: only those are confirmed (absent: every feature in scope).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stored_result_features: Option<Vec<(String, String)>>,
    /// Outputs `--strict` refuses whatever the corpus holds: structures met but not decoded, and
    /// vendor calibrations not applied (from the file's assurance block).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    fixed_refuses: Vec<String>,
    /// Fields whose values were assumed rather than read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    assumed: Vec<String>,
    features: Vec<FeatureRow>,
    /// Normalized fields with inferred meaning / fields with provenance.
    #[serde(default)]
    inferred: (u32, u32),
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Evidence {
    note: String,
    files: Vec<FileEvidence>,
    /// Held-out agreement per format (counts only; measured, never used to fix a reader).
    #[serde(default)]
    heldout: BTreeMap<String, HeldoutCounts>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct HeldoutCounts {
    pass: u32,
    fail: u32,
}

fn manifest(root: &Path) -> Result<Manifest> {
    let text = fs::read_to_string(root.join("corpus/manifest.toml")).context("read manifest")?;
    toml::from_str(&text).context("parse manifest")
}

/// The depositor of an entry: its source record (Zenodo record, MetaboLights study, GitHub
/// repository, OME sample directory, ...), else the `source` text before any parenthesis.
fn depositor(e: &Entry) -> (String, bool) {
    let synthetic = e.source.to_ascii_lowercase().starts_with("synthetic");
    if synthetic {
        return ("synthetic".into(), true);
    }
    if let Some(k) = crate::heldout::record_key(&e.url) {
        // a depositor is the repository, not one of its held-out-separated trees
        return (k.split('#').next().unwrap_or(&k).to_string(), false);
    }
    // Source texts name records too ("Zenodo record 7015307: ...", "nmrXiv study S275, ...",
    // "... (github.com/ProteoWizard/pwiz, commit ...)" for files unpacked from a bundle).
    let s = e.source.as_str();
    let lower = s.to_ascii_lowercase();
    if let Some(i) = lower.find("github.com/") {
        let repo: Vec<String> = lower[i + 11..]
            .split('/')
            .take(2)
            .map(|p| {
                p.chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || "-_.".contains(*c))
                    .collect()
            })
            .collect();
        if repo.len() == 2 && repo.iter().all(|p| !p.is_empty()) {
            return (format!("github:{}/{}", repo[0], repo[1]), false);
        }
    }
    let lower = s.to_ascii_lowercase();
    for (marker, prefix) in [
        ("zenodo record ", "zenodo:"),
        ("zenodo ", "zenodo:"),
        ("nmrxiv study ", "nmrxiv:"),
        ("dandi ", "dandi:"),
        ("figshare 10.6084/m9.figshare.", "figshare:"),
    ] {
        if let Some(i) = lower.find(marker) {
            let rest = &s[i + marker.len()..];
            let id: String = rest
                .chars()
                .take_while(char::is_ascii_alphanumeric)
                .collect();
            if !id.is_empty() {
                return (format!("{prefix}{id}"), false);
            }
        }
    }
    // A G-Node GIN repository named in the source text (`owner/repo on G-Node GIN …`, the form
    // directory entries without a URL use) is the same record as its `gin.g-node.org` URLs.
    if let Some(i) = lower.find(" on g-node gin") {
        let repo = lower[..i].split_whitespace().last().unwrap_or_default();
        if repo.contains('/') {
            return (format!("gin:{repo}"), false);
        }
    }
    let head = s.split(['(', ',', ':']).next().unwrap_or(s).trim();
    (head.to_ascii_lowercase(), false)
}

/// The development inputs the corpus test compares: `role = "input"` and depositor mzML/mzXML
/// exports, never a held-out entry.
fn development_inputs(m: &Manifest) -> Vec<&Entry> {
    m.file
        .iter()
        .filter(|e| e.tier != crate::heldout::TIER && e.role != crate::heldout::ROLE)
        .filter(|e| {
            e.role.is_empty()
                || e.role == "input"
                || (e.role == "oracle-export" && (e.format == "mzml" || e.format == "mzxml"))
        })
        .collect()
}

#[derive(Deserialize)]
struct ResultLine {
    id: String,
    format: String,
    status: String,
    #[serde(default)]
    independent: bool,
    #[serde(default)]
    compared: Vec<String>,
    #[serde(default)]
    fields: Vec<String>,
    /// `vendor_stored_result` for vendor-stored results reproduced; absent for an oracle.
    #[serde(default)]
    evidence: Option<String>,
    /// Vendor-stored results: the (kind, value) features of the signals they were computed
    /// from. Absent: every feature in the compared scopes.
    #[serde(default)]
    features: Option<Vec<(String, String)>>,
}

const VENDOR_STORED: &str = "vendor_stored_result";

/// Every results line of a file, by evidence class: the oracle comparisons and the
/// vendor-stored results.
#[derive(Default)]
struct Joined {
    oracle: Option<ResultLine>,
    stored: Option<ResultLine>,
}

fn join_line(slot: &mut Option<ResultLine>, r: ResultLine) {
    match slot {
        None => *slot = Some(r),
        Some(o) => {
            if r.status == "FAIL" {
                o.status = r.status;
            }
            o.independent |= r.independent;
            for c in r.compared {
                if !o.compared.contains(&c) {
                    o.compared.push(c);
                }
            }
            for c in r.fields {
                if !o.fields.contains(&c) {
                    o.fields.push(c);
                }
            }
            o.features = match (o.features.take(), r.features) {
                (Some(mut a), Some(b)) => {
                    for f in b {
                        if !a.contains(&f) {
                            a.push(f);
                        }
                    }
                    Some(a)
                }
                _ => None,
            };
        }
    }
}

/// `refresh`: run the CLI on every development input and write the evidence file.
pub fn refresh(
    root: &Path,
    corpus: &Path,
    bin: &Path,
    results: &[PathBuf],
    heldout: Option<&Path>,
) -> Result<()> {
    let m = manifest(root)?;
    let mut by_result: BTreeMap<(String, String), Joined> = BTreeMap::new();
    // Several result files (the corpus test, the m/z agreement test, vendor-stored results) may
    // report one file: within a class it fails when any comparison failed, and counts every
    // output any comparison covered.
    for results in results {
        let text = fs::read_to_string(results)
            .with_context(|| format!("read corpus results {}", results.display()))?;
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let r: ResultLine = serde_json::from_str(line).context("parse a results line")?;
            let j = by_result
                .entry((r.id.clone(), r.format.clone()))
                .or_default();
            if r.evidence.as_deref() == Some(VENDOR_STORED) {
                join_line(&mut j.stored, r);
            } else {
                join_line(&mut j.oracle, r);
            }
        }
    }
    let inputs: Vec<&Entry> = development_inputs(&m)
        .into_iter()
        .filter(|e| corpus.join(&e.filename).exists())
        .collect();
    println!("refresh: {} development inputs on disk", inputs.len());
    let threads = 4usize;
    let chunks: Vec<Vec<&Entry>> = (0..threads)
        .map(|t| inputs.iter().skip(t).step_by(threads).copied().collect())
        .collect();
    let mut files: Vec<FileEvidence> = Vec::new();
    std::thread::scope(|s| -> Result<()> {
        let handles: Vec<_> = chunks
            .iter()
            .map(|chunk| {
                let by_result = &by_result;
                s.spawn(move || -> Result<Vec<FileEvidence>> {
                    let mut out = Vec::new();
                    for e in chunk {
                        out.push(observe(e, corpus, bin, by_result)?);
                    }
                    Ok(out)
                })
            })
            .collect();
        for h in handles {
            files.extend(h.join().map_err(|_| anyhow::anyhow!("worker panicked"))??);
        }
        Ok(())
    })?;
    files.sort_by(|a, b| (&a.format, &a.id).cmp(&(&b.format, &b.id)));
    let mut ev = Evidence {
        note: "generated by `cargo xtask assurance-audit refresh` (docs/assurance.md); development corpus only".into(),
        files,
        heldout: BTreeMap::new(),
    };
    if let Some(h) = heldout {
        ev.heldout = heldout_counts(h)?;
    } else if let Ok(old) = load(root) {
        ev.heldout = old.heldout;
    }
    let path = root.join(EVIDENCE);
    fs::create_dir_all(path.parent().expect("has a parent"))?;
    fs::write(&path, serde_json::to_string_pretty(&ev)? + "\n")?;
    println!("wrote {} ({} files)", EVIDENCE, ev.files.len());
    Ok(())
}

fn heldout_counts(path: &Path) -> Result<BTreeMap<String, HeldoutCounts>> {
    let mut out: BTreeMap<String, HeldoutCounts> = BTreeMap::new();
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let v: serde_json::Value = serde_json::from_str(line)?;
        let format = v["format"].as_str().unwrap_or_default().to_string();
        let c = out.entry(format).or_default();
        match v["status"].as_str() {
            Some("pass") => c.pass += 1,
            Some("FAIL" | "PANIC") => c.fail += 1,
            _ => {}
        }
    }
    Ok(out)
}

fn observe(
    e: &Entry,
    corpus: &Path,
    bin: &Path,
    results: &BTreeMap<(String, String), Joined>,
) -> Result<FileEvidence> {
    let path = corpus.join(&e.filename);
    let out = Command::new(bin)
        .args(["info", "--json", "--compact"])
        .arg(&path)
        .env_remove("OPENREADOUT_STRICT")
        .output()
        .with_context(|| format!("run {}", bin.display()))?;
    let (source, synthetic) = depositor(e);
    let joined = results.get(&(e.id.clone(), e.format.clone()));
    let result = joined.and_then(|j| j.oracle.as_ref());
    let stored = joined.and_then(|j| j.stored.as_ref());
    let (stored_result, stored_result_compared, stored_result_features) = match stored {
        Some(r) => (Some(r.status.clone()), r.compared.clone(), r.features.clone()),
        None => (None, Vec::new(), None),
    };
    let (oracle, independent, compared, fields) = match result {
        Some(r) => (
            r.status.clone(),
            r.independent,
            r.compared.clone(),
            r.fields.clone(),
        ),
        None => ("none".to_string(), false, Vec::new(), Vec::new()),
    };
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    let data = &v["data"];
    if v["ok"].as_bool() != Some(true) {
        return Ok(FileEvidence {
            id: e.id.clone(),
            format: e.format.clone(),
            source,
            synthetic,
            oracle: "unreadable".into(),
            independent: false,
            compared: Vec::new(),
            fields: Vec::new(),
            stored_result: None,
            stored_result_compared: Vec::new(),
            stored_result_features: None,
            fixed_refuses: Vec::new(),
            assumed: Vec::new(),
            features: Vec::new(),
            inferred: (0, 0),
        });
    }
    let features = data["assurance"]["variant"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|f| {
                    (
                        f["kind"].as_str().unwrap_or_default().to_string(),
                        f["value"].as_str().unwrap_or_default().to_string(),
                        f["scope"]
                            .as_array()
                            .map(|s| {
                                s.iter()
                                    .filter_map(|x| x.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let inf = &data["assurance"]["inferred_fields"];
    let strs = |v: &serde_json::Value| -> Vec<String> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut fixed_refuses: Vec<String> = Vec::new();
    let undecoded = data["assurance"]["undecoded"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let calibrations = data["assurance"]["calibrations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for u in undecoded.iter().chain(
        calibrations
            .iter()
            .filter(|c| c["status"].as_str() == Some("not_applied")),
    ) {
        for sc in strs(&u["scope"]) {
            if !fixed_refuses.contains(&sc) {
                fixed_refuses.push(sc);
            }
        }
    }
    fixed_refuses.sort();
    let assumed: Vec<String> = data["assurance"]["assumed"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x["field"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Ok(FileEvidence {
        id: e.id.clone(),
        format: data["format"]["id"]
            .as_str()
            .unwrap_or(&e.format)
            .to_string(),
        source,
        synthetic,
        oracle,
        independent,
        compared,
        fields,
        stored_result,
        stored_result_compared,
        stored_result_features,
        fixed_refuses,
        assumed,
        features,
        inferred: (
            u32::try_from(inf["count"].as_u64().unwrap_or(0)).unwrap_or(u32::MAX),
            u32::try_from(inf["of"].as_u64().unwrap_or(0)).unwrap_or(u32::MAX),
        ),
    })
}

fn load(root: &Path) -> Result<Evidence> {
    let text = fs::read_to_string(root.join(EVIDENCE))
        .with_context(|| format!("read {EVIDENCE}; run `cargo xtask assurance-audit refresh`"))?;
    serde_json::from_str(&text).context("parse evidence")
}

/// Whether each variant-feature value the development corpus reaches, as (format, kind,
/// value), is validated: the rule the generated tables use (`validates_feature`).
pub fn feature_validation(root: &Path) -> Result<BTreeMap<(String, String, String), bool>> {
    let ev = load(root)?;
    let mut out: BTreeMap<(String, String, String), bool> = BTreeMap::new();
    for f in ev.files.iter().filter(|f| f.oracle != "unreadable") {
        for (kind, value, scope) in &f.features {
            *out.entry((f.format.clone(), kind.clone(), value.clone()))
                .or_insert(false) |= validates_feature(f, kind, value, scope);
        }
    }
    Ok(out)
}

/// Corpus coverage of one feature value.
#[derive(Debug, Default, Clone)]
struct Row {
    files: u32,
    sources: BTreeSet<String>,
    seen: u32,
}

/// Everything the rubric and the tables need about one format.
#[derive(Debug, Default)]
struct FormatEvidence {
    rows: BTreeMap<(String, String), Row>,
    files: u32,
    with_oracle: u32,
    passes: u32,
    fails: u32,
    independent_passes: u32,
    /// Confirmed files whose only independent evidence is vendor-stored results.
    stored_only: u32,
    sources: BTreeSet<String>,
    all_sources: BTreeSet<String>,
    versions: BTreeSet<String>,
    inferred: (u64, u64),
}

fn validates(f: &FileEvidence, scope: &[String]) -> bool {
    f.oracle == "pass"
        && f.independent
        && (scope.is_empty() || scope.iter().any(|s| f.compared.contains(s)))
}

/// Does file `f` validate feature (`kind`, `value`, `scope`)? A tracked field, and a rule that
/// derives one (`<field> by <rule>`), only when the file's comparison checked that field.
fn validates_feature(f: &FileEvidence, kind: &str, value: &str, scope: &[String]) -> bool {
    let field = match kind {
        "field" => Some(value),
        "derivation" => Some(value.split_once(" by ").map_or(value, |(field, _)| field)),
        _ => None,
    };
    let by_oracle = validates(f, scope) && field.is_none_or(|fd| f.fields.iter().any(|c| c == fd));
    // Vendor-stored results confirm only the outputs they were computed from, and when the
    // results line names the signals' features, only those.
    let by_stored = f.stored_result.as_deref() == Some("pass")
        && field.is_none()
        && !scope.is_empty()
        && scope.iter().any(|s| f.stored_result_compared.contains(s))
        && f.stored_result_features
            .as_ref()
            .is_none_or(|fs| fs.iter().any(|(k, v)| k == kind && v == value));
    by_oracle || by_stored
}

/// The file failed a comparison: its oracle, or its vendor-stored results.
fn file_fails(f: &FileEvidence) -> bool {
    f.oracle == "FAIL" || f.stored_result.as_deref() == Some("FAIL")
}

/// The file is confirmed by independent evidence: an independent oracle, or vendor-stored
/// results reproduced (and nothing failed).
fn file_confirmed(f: &FileEvidence) -> bool {
    !file_fails(f)
        && ((f.oracle == "pass" && f.independent) || f.stored_result.as_deref() == Some("pass"))
}

fn by_format(ev: &Evidence) -> BTreeMap<String, FormatEvidence> {
    let mut out: BTreeMap<String, FormatEvidence> = BTreeMap::new();
    for f in &ev.files {
        if f.oracle == "unreadable" {
            continue;
        }
        let fe = out.entry(f.format.clone()).or_default();
        fe.files += 1;
        if !f.synthetic {
            fe.all_sources.insert(f.source.clone());
        }
        let stored = f.stored_result.is_some();
        if matches!(f.oracle.as_str(), "pass" | "FAIL") || stored {
            fe.with_oracle += 1;
        }
        if file_fails(f) {
            fe.fails += 1;
        } else if f.oracle == "pass" || f.stored_result.as_deref() == Some("pass") {
            fe.passes += 1;
        }
        let file_validates = file_confirmed(f);
        if file_validates {
            fe.independent_passes += 1;
            if !(f.oracle == "pass" && f.independent) {
                fe.stored_only += 1;
            }
            if !f.synthetic {
                fe.sources.insert(f.source.clone());
            }
        }
        fe.inferred.0 += u64::from(f.inferred.0);
        fe.inferred.1 += u64::from(f.inferred.1);
        for (kind, value, scope) in &f.features {
            let r = fe.rows.entry((kind.clone(), value.clone())).or_default();
            r.seen += 1;
            if validates_feature(f, kind, value, scope) {
                r.files += 1;
                if !f.synthetic {
                    r.sources.insert(f.source.clone());
                }
                if matches!(
                    kind.as_str(),
                    "format_version" | "writer" | "writer_version"
                ) {
                    fe.versions.insert(format!("{kind}={value}"));
                }
            }
        }
    }
    out
}

/// The rubric (docs/assurance.md), from the evidence of one format and its documentation basis.
fn rubric(
    fe: Option<&FormatEvidence>,
    basis: &str,
    heldout: Option<&HeldoutCounts>,
) -> (&'static str, String) {
    let Some(fe) = fe else {
        return ("Low", "no development file".into());
    };
    let agreement = if fe.with_oracle == 0 {
        0.0
    } else {
        f64::from(fe.passes) / f64::from(fe.with_oracle)
    };
    let sources = u32::try_from(fe.sources.len()).unwrap_or(u32::MAX);
    let versions = fe.versions.len();
    let inferred = if fe.inferred.1 == 0 {
        0.0
    } else {
        fe.inferred.0 as f64 / fe.inferred.1 as f64
    };
    let documented = matches!(basis, "open_spec" | "vendor_docs");
    let heldout_fail = heldout.is_some_and(|h| h.fail > 0);
    let high = fe.independent_passes >= 5
        && sources >= 3
        && versions >= 2
        && fe.fails == 0
        && (documented || (sources >= 5 && fe.independent_passes >= 10))
        && (inferred <= 0.5 || sources >= 5)
        && !heldout_fail;
    let medium = fe.independent_passes >= 3 && (sources >= 2 || versions >= 3) && agreement >= 0.9;
    let why = format!(
        "{} independent passes from {sources} sources, {versions} versions, agreement {:.0}%, inferred {:.0}%, basis {basis}{}",
        fe.independent_passes,
        agreement * 100.0,
        inferred * 100.0,
        if heldout_fail {
            ", held-out failures"
        } else {
            ""
        }
    );
    if high {
        ("High", why)
    } else if medium {
        ("Medium", why)
    } else {
        ("Low", why)
    }
}

/// `CZI_` for `czi`, `THERMO_RAW_` for `thermo-raw`.
fn ident(format: &str) -> String {
    format.to_ascii_uppercase().replace('-', "_")
}

fn kind_variant(kind: &str) -> String {
    kind.split('_')
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
                .unwrap_or_default()
        })
        .collect()
}

fn rust_str(s: &str) -> String {
    format!("{s:?}")
}

fn block(format: &str, fe: Option<&FormatEvidence>, level: &str) -> String {
    let id = ident(format);
    let mut s = format!(
        "// BEGIN GENERATED {format} (cargo xtask assurance-audit --write; do not edit)\nconst {id}_CONFIDENCE: Confidence = Confidence::{level};\n"
    );
    let rows: Vec<(&(String, String), &Row)> =
        fe.map(|f| f.rows.iter().collect()).unwrap_or_default();
    if rows.is_empty() {
        let _ = writeln!(s, "const {id}_VALIDATED: &[Validated] = &[];");
    } else {
        // One line per row whatever its length: rustfmt leaves the table alone.
        let _ = writeln!(
            s,
            "#[rustfmt::skip]\nconst {id}_VALIDATED: &[Validated] = &["
        );
        for ((kind, value), r) in rows {
            let _ = writeln!(
                s,
                "    a::row(K::{}, {}, {}, {}, {}),",
                kind_variant(kind),
                rust_str(value),
                r.files,
                r.sources.len(),
                r.seen
            );
        }
        s.push_str("];\n");
    }
    let _ = write!(s, "// END GENERATED {format}");
    s
}

/// Every `crates/*/src/assurance.rs` with the formats it has generated blocks for.
fn profile_files(root: &Path) -> Result<Vec<(PathBuf, Vec<String>)>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(root.join("crates"))? {
        let p = entry?.path().join("src/assurance.rs");
        if !p.exists() {
            continue;
        }
        let text = fs::read_to_string(&p)?;
        let formats: Vec<String> = text
            .lines()
            .filter_map(|l| l.strip_prefix("// BEGIN GENERATED "))
            .map(|r| r.split_whitespace().next().unwrap_or_default().to_string())
            .collect();
        out.push((p, formats));
    }
    out.sort();
    Ok(out)
}

/// The `basis: Basis::X` of the profile whose `format_id` is `format`.
fn basis_of(text: &str, format: &str) -> String {
    let needle = format!("format_id: \"{format}\"");
    let Some(i) = text.find(&needle) else {
        return "unknown".into();
    };
    let rest = &text[i..];
    rest.find("basis: Basis::").map_or_else(
        || "unknown".into(),
        |j| {
            let v: String = rest[j + 14..]
                .chars()
                .take_while(char::is_ascii_alphanumeric)
                .collect();
            let mut snake = String::new();
            for (k, c) in v.chars().enumerate() {
                if c.is_ascii_uppercase() && k > 0 {
                    snake.push('_');
                }
                snake.push(c.to_ascii_lowercase());
            }
            snake
        },
    )
}

fn replace_block(text: &str, format: &str, new: &str) -> Result<String> {
    let begin = format!("// BEGIN GENERATED {format} ");
    let end = format!("// END GENERATED {format}");
    let b = text
        .find(&begin)
        .with_context(|| format!("no BEGIN marker for {format}"))?;
    let e = text[b..]
        .find(&end)
        .with_context(|| format!("no END marker for {format}"))?
        + b
        + end.len();
    Ok(format!("{}{}{}", &text[..b], new, &text[e..]))
}

struct PageRow {
    format: String,
    level: String,
    basis: String,
    fe_line: String,
}

fn page(rows: &[PageRow], ev: &Evidence) -> String {
    let mut s = String::from(
        "# Evidence per format\n\n<!-- generated by `cargo xtask assurance-audit --write` from corpus/assurance/evidence.json; do not edit -->\n\nEach reader's confidence level is computed from the development corpus by the [confidence rubric](https://github.com/openreadout/openreadout/blob/main/docs/assurance.md#the-confidence-rubric), not assigned by hand. *Files* counts development files read; *confirmed* those that match an independent reader (or whose vendor-stored results, such as Chromeleon's stored peaks, our decoded values reproduce: they confirm those signals only, see *Vendor-stored results* in the assurance chapter); *sources* distinct depositors among them; *versions* distinct format versions, writers and writer versions confirmed; *agreement* the share of files with an oracle that match it; *inferred* the share of normalized fields whose meaning was reverse-engineered rather than specified; *held-out* the generalization benchmark (pass/fail, measured on files never used for development).\n\n| format | level | basis | files | confirmed | sources | versions | agreement | inferred | held-out |\n| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n",
    );
    let per = by_format(ev);
    for r in rows {
        let fe = per.get(&r.format);
        let h = ev.heldout.get(&r.format);
        let _ = writeln!(
            s,
            "| `{}` | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            r.format,
            r.level.to_ascii_lowercase(),
            r.basis.replace('_', " "),
            fe.map_or(0, |f| f.files),
            fe.map_or("0".to_string(), |f| if f.stored_only > 0 {
                format!(
                    "{} ({} by stored results)",
                    f.independent_passes, f.stored_only
                )
            } else {
                f.independent_passes.to_string()
            }),
            fe.map_or(0, |f| f.sources.len()),
            fe.map_or(0, |f| f.versions.len()),
            fe.map_or("-".to_string(), |f| if f.with_oracle == 0 {
                "-".into()
            } else {
                format!(
                    "{:.0}%",
                    100.0 * f64::from(f.passes) / f64::from(f.with_oracle)
                )
            }),
            fe.map_or("-".to_string(), |f| if f.inferred.1 == 0 {
                "-".into()
            } else {
                format!("{:.0}%", 100.0 * f.inferred.0 as f64 / f.inferred.1 as f64)
            }),
            h.map_or("-".to_string(), |h| format!("{}/{}", h.pass, h.fail)),
        );
        let _ = &r.fe_line;
    }
    s.push_str("\nThe validated-variant tables themselves (every format version, writer, codec, layout and acquisition mode confirmed, with file and source counts) are in each reader crate's `src/assurance.rs`; `openreadout info FILE --json` → `assurance` shows which of them a given file uses.\n");
    s
}

/// The audit: recompute tables, levels and the evidence page; fail on drift, or rewrite them.
pub fn audit(root: &Path, write: bool) -> Result<()> {
    let ev = load(root)?;
    let per = by_format(&ev);
    let mut stale = Vec::new();
    let mut seen_formats: BTreeSet<String> = BTreeSet::new();
    let mut rows: Vec<PageRow> = Vec::new();
    for (path, formats) in profile_files(root)? {
        let text = fs::read_to_string(&path)?;
        let mut new_text = text.clone();
        for format in &formats {
            seen_formats.insert(format.clone());
            let basis = basis_of(&text, format);
            let (level, why) = rubric(per.get(format), &basis, ev.heldout.get(format));
            let expected = block(format, per.get(format), level);
            new_text = replace_block(&new_text, format, &expected)?;
            rows.push(PageRow {
                format: format.clone(),
                level: level.to_string(),
                basis,
                fe_line: why.clone(),
            });
            println!("{format:<24} {level:<7} {why}");
        }
        if new_text != text {
            if write {
                fs::write(&path, &new_text)?;
                println!(
                    "rewrote {}",
                    path.strip_prefix(root).unwrap_or(&path).display()
                );
            } else {
                stale.push(
                    path.strip_prefix(root)
                        .unwrap_or(&path)
                        .display()
                        .to_string(),
                );
            }
        }
    }
    for f in per.keys() {
        if !seen_formats.contains(f) {
            stale.push(format!(
                "format `{f}` has corpus evidence but no assurance profile block"
            ));
        }
    }
    rows.sort_by(|a, b| a.format.cmp(&b.format));
    let page_text = page(&rows, &ev);
    let page_path = root.join(BOOK_PAGE);
    let current = fs::read_to_string(&page_path).unwrap_or_default();
    if current != page_text {
        if write {
            fs::write(&page_path, &page_text)?;
            println!("rewrote {BOOK_PAGE}");
        } else {
            stale.push(BOOK_PAGE.to_string());
        }
    }
    let manifest = manifest(root)?;
    let missing: BTreeSet<&str> = development_inputs(&manifest)
        .iter()
        .map(|e| e.format.as_str())
        .filter(|f| !matches!(*f, "zip" | "flowjo-wsp" | "gating-ml") && !seen_formats.contains(*f))
        .collect();
    for f in missing {
        stale.push(format!(
            "manifest format `{f}` has no assurance profile block"
        ));
    }
    if !stale.is_empty() {
        bail!(
            "assurance tables are stale or missing (run `cargo xtask assurance-audit --write`):\n  {}",
            stale.join("\n  ")
        );
    }
    println!(
        "assurance: {} formats, {} development files of evidence, all tables current",
        seen_formats.len(),
        ev.files.len()
    );
    Ok(())
}

// ---------- leave-one-depositor-out cross-validation (`assurance-audit cv`) ----------

/// What `--strict` would do with one file whose depositor's evidence is held out.
#[derive(Debug, Default, Clone, Serialize)]
struct CvVerdict {
    id: String,
    format: String,
    source: String,
    /// The file's own comparison: `pass` (confirmed), `FAIL`, or `none`.
    truth: String,
    /// Outputs its comparison checked.
    compared: Vec<String>,
    level: String,
    refused: Vec<String>,
    withheld: Vec<String>,
    /// The kinds of the features that made it refuse (`codec`, `writer_version`, ...), and the
    /// refusals that come from the file alone (`undecoded`, `calibration`).
    causes: Vec<String>,
}

fn status_of(rows: &BTreeMap<(String, String), Row>, kind: &str, value: &str) -> &'static str {
    match rows.get(&(kind.to_string(), value.to_string())) {
        Some(r) if r.files > 0 => "validated",
        Some(r) if r.seen > 0 => "seen",
        _ => "unseen",
    }
}

/// The core's `version_key`: family (text before the first digit) and numeric components.
fn version_key(value: &str) -> Option<(String, Vec<u64>)> {
    let start = value.find(|c: char| c.is_ascii_digit())?;
    let family = value[..start].trim().to_ascii_lowercase();
    let nums: Vec<u64> = value[start..]
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .take(3)
        .filter_map(|p| p.parse().ok())
        .collect();
    (!nums.is_empty()).then_some((family, nums))
}

/// The core's bracketing rule: an unseen format or writer version with validated versions of
/// the same family and major version on both sides is not refused.
fn bracketed(rows: &BTreeMap<(String, String), Row>, kind: &str, value: &str) -> bool {
    if kind != "format_version" && kind != "writer_version" {
        return false;
    }
    let Some((family, v)) = version_key(value) else {
        return false;
    };
    let (mut lo, mut hi) = (false, false);
    for ((k, other), r) in rows {
        if k != kind || r.files == 0 {
            continue;
        }
        if let Some((fam, w)) = version_key(other)
            && fam == family
            && w.first() == v.first()
        {
            lo |= w < v;
            hi |= w > v;
        }
    }
    lo && hi
}

/// The assurance of `f` under the tables `rows` (the core's `assess_with`, on the evidence).
fn verdict(f: &FileEvidence, rows: &BTreeMap<(String, String), Row>) -> CvVerdict {
    let mut refused: BTreeSet<String> = f.fixed_refuses.iter().cloned().collect();
    let mut causes: BTreeSet<String> = BTreeSet::new();
    if !f.fixed_refuses.is_empty() {
        causes.insert("undecoded or uncalibrated".into());
    }
    let mut withheld: BTreeSet<String> = f.assumed.iter().cloned().collect();
    let mut partial = !f.assumed.is_empty();
    for (kind, value, scope) in &f.features {
        let status = status_of(rows, kind, value);
        if status == "validated" {
            continue;
        }
        if kind == "field" || kind == "derivation" {
            partial |= kind == "derivation";
            withheld.insert(
                value
                    .split_once(" by ")
                    .map_or(value.as_str(), |(fd, _)| fd)
                    .to_string(),
            );
            continue;
        }
        match (status, scope.is_empty()) {
            ("unseen", false) if !bracketed(rows, kind, value) => {
                refused.extend(scope.iter().cloned());
                causes.insert(kind.clone());
            }
            _ => partial = true,
        }
    }
    let level = if !refused.is_empty() {
        "unvalidated"
    } else if partial {
        "partially_validated"
    } else {
        "validated"
    };
    let truth = if file_fails(f) {
        "FAIL"
    } else if file_confirmed(f) {
        "pass"
    } else {
        "none"
    };
    let mut compared = f.compared.clone();
    for s in &f.stored_result_compared {
        if !compared.contains(s) {
            compared.push(s.clone());
        }
    }
    CvVerdict {
        id: f.id.clone(),
        format: f.format.clone(),
        source: f.source.clone(),
        truth: truth.into(),
        compared,
        level: level.into(),
        refused: refused.into_iter().collect(),
        withheld: withheld.into_iter().collect(),
        causes: causes.into_iter().collect(),
    }
}

/// Every development file assessed with the tables its own depositor did not contribute to
/// (synthetic fixtures stay in: they are ours, not a depositor's).
fn leave_one_depositor_out(ev: &Evidence) -> Vec<CvVerdict> {
    let mut by_fmt: BTreeMap<&str, Vec<&FileEvidence>> = BTreeMap::new();
    for f in ev.files.iter().filter(|f| f.oracle != "unreadable") {
        by_fmt.entry(f.format.as_str()).or_default().push(f);
    }
    let mut out = Vec::new();
    for (format, files) in by_fmt {
        let depositors: BTreeSet<&str> = files
            .iter()
            .filter(|f| !f.synthetic)
            .map(|f| f.source.as_str())
            .collect();
        for d in depositors {
            let rest = Evidence {
                files: files
                    .iter()
                    .filter(|f| f.synthetic || f.source != d)
                    .map(|f| (*f).clone())
                    .collect(),
                ..Evidence::default()
            };
            let per = by_format(&rest);
            let empty = BTreeMap::new();
            let rows = per.get(format).map_or(&empty, |fe| &fe.rows);
            for f in files.iter().filter(|f| !f.synthetic && f.source == d) {
                out.push(verdict(f, rows));
            }
        }
    }
    out
}

/// `assurance-audit cv`: leave-one-depositor-out cross-validation of the assurance signal on
/// the development corpus (docs/assurance.md § Cross-validation). Prints, over files an
/// independent reader confirmed (their values are right), how often `--strict` would refuse an
/// output their comparison checked, and why; over files whose comparison fails, how often it
/// would refuse the failing outputs. `--json` writes every file's verdict.
pub fn cross_validate(root: &Path, json: Option<&Path>) -> Result<()> {
    let ev = load(root)?;
    let verdicts = leave_one_depositor_out(&ev);
    let refuses_checked = |v: &CvVerdict| v.refused.iter().any(|s| v.compared.contains(s));
    let confirmed: Vec<&CvVerdict> = verdicts.iter().filter(|v| v.truth == "pass").collect();
    let failing: Vec<&CvVerdict> = verdicts.iter().filter(|v| v.truth == "FAIL").collect();
    let pct = |a: usize, b: usize| {
        if b == 0 {
            "-".to_string()
        } else {
            format!("{:.1}%", 100.0 * a as f64 / b as f64)
        }
    };
    let n_ref = confirmed.iter().filter(|v| refuses_checked(v)).count();
    let n_any = confirmed.iter().filter(|v| !v.refused.is_empty()).count();
    let n_wh = confirmed.iter().filter(|v| !v.withheld.is_empty()).count();
    let mut levels: BTreeMap<&str, usize> = BTreeMap::new();
    for v in &confirmed {
        *levels.entry(v.level.as_str()).or_default() += 1;
    }
    println!(
        "leave-one-depositor-out over {} development files ({} confirmed by an independent reader, {} failing, {} without an oracle)",
        verdicts.len(),
        confirmed.len(),
        failing.len(),
        verdicts.len() - confirmed.len() - failing.len()
    );
    println!(
        "confirmed files --strict would refuse on an output their comparison checked: {n_ref} ({}); refusing any output: {n_any} ({}); withholding a field: {n_wh} ({})",
        pct(n_ref, confirmed.len()),
        pct(n_any, confirmed.len()),
        pct(n_wh, confirmed.len())
    );
    println!("levels of the confirmed files: {levels:?}");
    let mut causes: BTreeMap<&str, usize> = BTreeMap::new();
    for v in confirmed.iter().filter(|v| !v.refused.is_empty()) {
        for c in &v.causes {
            *causes.entry(c.as_str()).or_default() += 1;
        }
    }
    println!("what made confirmed files refused (a file can have several causes): {causes:?}");
    let caught = failing.iter().filter(|v| refuses_checked(v)).count();
    println!(
        "failing files whose failing outputs --strict would refuse: {caught} of {} ({})",
        failing.len(),
        pct(caught, failing.len())
    );
    let mut per_fmt: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for v in &confirmed {
        let e = per_fmt.entry(v.format.as_str()).or_default();
        e.0 += 1;
        if refuses_checked(v) {
            e.1 += 1;
        }
    }
    println!("per format (confirmed files, of which refused):");
    for (f, (n, r)) in &per_fmt {
        if *r > 0 {
            println!("  {f:<28} {n:>4} {r:>4} ({})", pct(*r, *n));
        }
    }
    if let Some(p) = json {
        fs::write(p, serde_json::to_string_pretty(&verdicts)? + "\n")?;
        println!("wrote {}", p.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(
        oracle: &str,
        independent: bool,
        compared: &[&str],
        features: &[(&str, &str, &[&str])],
    ) -> FileEvidence {
        FileEvidence {
            id: "x".into(),
            format: "fmt".into(),
            source: "zenodo:1".into(),
            synthetic: false,
            oracle: oracle.into(),
            independent,
            compared: compared.iter().map(|s| (*s).to_string()).collect(),
            fields: Vec::new(),
            stored_result: None,
            stored_result_compared: Vec::new(),
            stored_result_features: None,
            fixed_refuses: Vec::new(),
            assumed: Vec::new(),
            features: features
                .iter()
                .map(|(k, v, s)| {
                    (
                        (*k).into(),
                        (*v).into(),
                        s.iter().map(|x| (*x).to_string()).collect(),
                    )
                })
                .collect(),
            inferred: (1, 10),
        }
    }

    #[test]
    fn fields_and_rules_need_an_oracle_that_checked_the_field() {
        let mut checked = file(
            "pass",
            true,
            &["metadata", "tables"],
            &[
                ("field", "tables[].extra.reads[].mode", &[]),
                (
                    "derivation",
                    "tables[].extra.reads[].mode by label keywords",
                    &[],
                ),
                ("field", "experiment.acquisition.started_at", &[]),
            ],
        );
        checked.fields = vec!["tables[].extra.reads[].mode".into()];
        let ev = Evidence {
            files: vec![checked],
            ..Evidence::default()
        };
        let per = by_format(&ev);
        let rows = &per["fmt"].rows;
        assert_eq!(
            rows[&("field".into(), "tables[].extra.reads[].mode".into())].files,
            1
        );
        assert_eq!(
            rows[&(
                "derivation".into(),
                "tables[].extra.reads[].mode by label keywords".into()
            )]
                .files,
            1
        );
        // The oracle passed, but it never compared the acquisition time.
        assert_eq!(
            rows[&("field".into(), "experiment.acquisition.started_at".into())].files,
            0
        );
    }

    #[test]
    fn vendor_stored_results_confirm_only_the_signals_they_were_computed_from() {
        let mut f = file(
            "none",
            false,
            &[],
            &[
                ("sample_layout", "f64", &["traces"]),
                ("writer_version", "Chromeleon 7.2", &[]),
                ("layout", "cmbx", &["metadata", "tables"]),
                ("field", "experiment.acquisition.started_at", &[]),
            ],
        );
        f.stored_result = Some("pass".into());
        f.stored_result_compared = vec!["traces".into()];
        let ev = Evidence {
            files: vec![f.clone()],
            ..Evidence::default()
        };
        let per = by_format(&ev);
        let fe = &per["fmt"];
        assert_eq!(fe.independent_passes, 1);
        assert_eq!(fe.stored_only, 1);
        assert_eq!(fe.rows[&("sample_layout".into(), "f64".into())].files, 1);
        assert_eq!(
            fe.rows[&("writer_version".into(), "Chromeleon 7.2".into())].files,
            0
        );
        assert_eq!(fe.rows[&("layout".into(), "cmbx".into())].files, 0);
        assert_eq!(
            fe.rows[&("field".into(), "experiment.acquisition.started_at".into())].files,
            0
        );
        // Naming the integrated signals' features confirms those and no other trace feature.
        let mut g = f.clone();
        g.features.push(("layout".into(), "10 Hz signal in bar".into(), vec!["traces".into()]));
        g.stored_result_features = Some(vec![("sample_layout".into(), "f64".into())]);
        let ev = Evidence {
            files: vec![g],
            ..Evidence::default()
        };
        let per = by_format(&ev);
        assert_eq!(per["fmt"].rows[&("sample_layout".into(), "f64".into())].files, 1);
        assert_eq!(
            per["fmt"].rows[&("layout".into(), "10 Hz signal in bar".into())].files,
            0
        );
        // A stored result the decoded values do not reproduce is a failure.
        f.stored_result = Some("FAIL".into());
        let ev = Evidence {
            files: vec![f],
            ..Evidence::default()
        };
        assert_eq!(by_format(&ev)["fmt"].fails, 1);
    }

    #[test]
    fn a_feature_is_validated_only_on_the_outputs_compared() {
        let ev = Evidence {
            files: vec![
                file(
                    "pass",
                    true,
                    &["metadata"],
                    &[("codec", "jpeg", &["pixels"]), ("writer", "ZEN", &[])],
                ),
                file(
                    "pass",
                    true,
                    &["metadata", "pixels"],
                    &[("codec", "zstd", &["pixels"])],
                ),
                file(
                    "pass",
                    false,
                    &["metadata", "pixels"],
                    &[("codec", "lzw", &["pixels"])],
                ),
            ],
            ..Evidence::default()
        };
        let per = by_format(&ev);
        let rows = &per["fmt"].rows;
        assert_eq!(rows[&("codec".into(), "jpeg".into())].files, 0);
        assert_eq!(rows[&("codec".into(), "jpeg".into())].seen, 1);
        assert_eq!(rows[&("writer".into(), "ZEN".into())].files, 1);
        assert_eq!(rows[&("codec".into(), "zstd".into())].files, 1);
        assert_eq!(rows[&("codec".into(), "lzw".into())].files, 0);
    }

    #[test]
    fn generated_block_is_stable_rust() {
        let ev = Evidence {
            files: vec![file(
                "pass",
                true,
                &["pixels"],
                &[("sample_layout", "uint8x3", &["pixels"])],
            )],
            ..Evidence::default()
        };
        let per = by_format(&ev);
        let b = block("thermo-raw", per.get("fmt"), "Medium");
        assert!(b.contains("const THERMO_RAW_CONFIDENCE: Confidence = Confidence::Medium;"));
        assert!(b.contains("a::row(K::SampleLayout, \"uint8x3\", 1, 1, 1),"));
        let text = format!("x\n{}\ny\n", block("thermo-raw", None, "Low"));
        let replaced = replace_block(&text, "thermo-raw", &b).unwrap();
        assert!(replaced.starts_with("x\n// BEGIN GENERATED thermo-raw"));
        assert!(replaced.ends_with("// END GENERATED thermo-raw\ny\n"));
    }

    #[test]
    fn rubric_levels() {
        let mut fe = FormatEvidence::default();
        assert_eq!(rubric(Some(&fe), "open_spec", None).0, "Low");
        fe.independent_passes = 10;
        fe.passes = 10;
        fe.with_oracle = 10;
        fe.sources = ["a", "b", "c"].iter().map(|s| (*s).to_string()).collect();
        fe.versions = ["v1", "v2"].iter().map(|s| (*s).to_string()).collect();
        fe.inferred = (1, 10);
        assert_eq!(rubric(Some(&fe), "open_spec", None).0, "High");
        // reverse-engineered needs five independent sources for high
        assert_eq!(rubric(Some(&fe), "reverse_engineered", None).0, "Medium");
        // a held-out failure caps it at medium
        let h = HeldoutCounts { pass: 3, fail: 1 };
        assert_eq!(rubric(Some(&fe), "open_spec", Some(&h)).0, "Medium");
    }

    #[test]
    fn depositors() {
        let e = |url: &str, source: &str| Entry {
            id: "i".into(),
            format: "f".into(),
            tier: String::new(),
            role: String::new(),
            filename: String::new(),
            url: url.into(),
            source: source.into(),
        };
        assert_eq!(
            depositor(&e("https://zenodo.org/records/123/files/a", "")).0,
            "zenodo:123"
        );
        assert_eq!(
            depositor(&e("", "nmrXiv study S275, experiment 1")).0,
            "nmrxiv:S275"
        );
        assert!(depositor(&e("", "synthetic (pylibCZIrw)")).1);
        // a GIN repository named in the source text is the record its URLs name
        assert_eq!(
            depositor(&e(
                "",
                "NeuralEnsemble/ephy_testing_data on G-Node GIN (python-neo test data)"
            ))
            .0,
            depositor(&e(
                "https://gin.g-node.org/NeuralEnsemble/ephy_testing_data/raw/bf392aa/x.ncs",
                ""
            ))
            .0
        );
        assert_eq!(
            depositor(&e(
                "",
                "ProteoWizard vendor-reader test data (github.com/ProteoWizard/pwiz, commit 699dd48953f8)"
            ))
            .0,
            "github:proteowizard/pwiz"
        );
    }
}
