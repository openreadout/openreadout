//! `cargo xtask heldout-check`: keep the held-out corpus held out.
//!
//! Held-out files (`tier = "heldout"` in corpus/manifest.toml; their inputs carry
//! `role = "heldout"`) measure whether the readers generalize to files nobody developed against
//! (docs/benchmark/heldout.md). They must never be used to develop or debug a reader, so:
//!
//! 1. no provenance log (`docs/provenance/*.md`), format note (`docs/formats/*.md`) or reader source
//!    (`crates/*/src/**/*.rs`) may cite a held-out file: its corpus id, its download URL or its
//!    (distinctive) file name, or the number of its source record ("figshare 31095109",
//!    "MTBLS12457", "Zenodo 4563053", "S-BIAD1129"). Only the reserved record itself counts: another
//!    record by the same lab is a different source (rule 3) and may be cited. A line that says
//!    the record is held out ("Zenodo 15845146 (held out, never opened)") is a disclosure, not a
//!    citation. Records of `exposed` entries are left out, since development work already used them;
//! 2. the development corpus must not share a source record with the held-out set (the same Zenodo
//!    record, MetaboLights study, PRIDE project, EMPIAR entry, OME sample directory, GitHub
//!    repository, Cell Painting Gallery dataset site or BioImage Archive study): a fix for a held-out failure is developed on a NEW file, never on a sibling of
//!    the held-out one;
//! 3. every held-out entry is internally consistent (inputs have `role = "heldout"`, exports and
//!    bundles are on the held-out tier too, ids are unique to the held-out set).
//!
//! A held-out input whose source record was used for development before the draw reserved it
//! carries `exposed = "<YYYY-MM-DD>: <who> developed on files from this record …; <what was
//! inferred>"`. It stays in the held-out set (its files remain measured), but it does not count
//! toward the generalization numbers (evals/heldout_summary.py reports it separately). The check
//! prints every exposed entry and refuses an `exposed` value that is not a dated statement or that
//! sits on anything but a held-out input.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Manifest {
    file: Vec<Entry>,
}

#[derive(Debug, Deserialize, Clone)]
struct Entry {
    id: String,
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
    /// Development exposure recorded after the fact (module docs).
    #[serde(default)]
    exposed: Option<String>,
}

/// The tier every held-out entry (input, paired export, bundle) is on.
pub const TIER: &str = "heldout";
/// The role of a held-out input (a file the benchmark and the generalization report read).
pub const ROLE: &str = "heldout";

/// A source record a URL comes from: files from one record share a depositor and usually an
/// instrument, so a record is either held out entirely or not at all.
pub(crate) fn record_key(url: &str) -> Option<String> {
    let lower = url.to_ascii_lowercase();
    let after = |marker: &str| lower.find(marker).map(|i| &lower[i + marker.len()..]);
    let digits = |s: &str| -> String { s.chars().take_while(char::is_ascii_digit).collect() };
    let segs = |s: &str, n: usize| -> String { s.split('/').take(n).collect::<Vec<_>>().join("/") };
    for marker in [
        "zenodo.org/api/records/",
        "zenodo.org/records/",
        "zenodo.org/record/",
    ] {
        if let Some(rest) = after(marker) {
            let d = digits(rest);
            if !d.is_empty() {
                return Some(format!("zenodo:{d}"));
            }
        }
    }
    if let Some(i) = lower.find("/mtbls") {
        let d = digits(&lower[i + 6..]);
        if !d.is_empty() {
            return Some(format!("metabolights:MTBLS{d}"));
        }
    }
    if let Some(i) = lower.find("pxd") {
        let d = digits(&lower[i + 3..]);
        if d.len() == 6 {
            return Some(format!("pride:PXD{d}"));
        }
    }
    if let Some(i) = lower.find("empiar") {
        let d: String = lower[i + 6..]
            .trim_start_matches(|c: char| !c.is_ascii_digit())
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if d.len() == 5 {
            return Some(format!("empiar:{d}"));
        }
    }
    if let Some(rest) = after("downloads.openmicroscopy.org/images/") {
        return Some(format!("ome-sample:{}", segs(rest, 2)));
    }
    if let Some(rest) = after("raw.githubusercontent.com/") {
        return Some(github_record(rest, 3));
    }
    if let Some(rest) = after("github.com/") {
        return Some(github_record(rest, 4));
    }
    if let Some(rest) = after("cellpainting-gallery.s3.amazonaws.com/") {
        // a Cell Painting Gallery dataset and its contributing site (`cpg0016-jump/source_4`)
        return Some(format!("cell-painting-gallery:{}", segs(rest, 2)));
    }
    if let Some(i) = lower.rfind("/s-biad") {
        let d = digits(&lower[i + 7..]);
        if !d.is_empty() {
            return Some(format!("bioimage-archive:S-BIAD{d}"));
        }
    }
    if let Some(i) = lower.rfind("/s-bsst") {
        // a BioStudies study (`S-BSST1202`), stored in the same archive as S-BIAD studies
        let d = digits(&lower[i + 7..]);
        if !d.is_empty() {
            return Some(format!("biostudies:S-BSST{d}"));
        }
    }
    if let Some(rest) = after("flowrepository.org/")
        && let Some(j) = rest.find("fr-fcm-")
    {
        let id: String = rest[j..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        return Some(format!("flowrepository:{id}"));
    }
    if let Some(rest) = after("gin.g-node.org/") {
        // a G-Node GIN repository (`owner/repo`)
        return Some(format!("gin:{}", segs(rest, 2)));
    }
    if let Some(i) = lower.find("msv") {
        let d = digits(&lower[i + 3..]);
        if d.len() == 9 {
            return Some(format!("massive:MSV{d}"));
        }
    }
    if let Some(rest) = after("dandiarchive.org/api/dandisets/") {
        let d = digits(rest);
        if !d.is_empty() {
            return Some(format!("dandi:{d}"));
        }
    }
    None
}

/// Source records named in an entry's `source` text. Some downloads carry no record id in their
/// URL (a figshare file id, a DANDI asset UUID, an nmrXiv archive UUID, a Dataverse file id), and
/// some development entries come out of bundles and have no URL at all; their `source` names the
/// record ("Figshare 28050092 '…'", "DANDI 000293", "nmrXiv study S275 (project P52)",
/// "Harvard Dataverse doi:10.7910/DVN/QE5CEQ", "Zenodo record 14316687", "PRIDE PXD074950").
/// Keys have the same form as `record_key`'s, so a record found in a URL on one side and in a
/// source text on the other still matches.
pub(crate) fn source_keys(source: &str) -> Vec<String> {
    let lower = source.to_ascii_lowercase();
    let mut out = Vec::new();
    // the first run of at least `min` digits within `window` bytes after each `marker`
    let ids_after = |marker: &str, min: usize, window: usize| -> Vec<String> {
        let mut ids = Vec::new();
        let mut from = 0;
        while let Some(i) = lower[from..].find(marker) {
            let start = from + i + marker.len();
            let end = (start + window).min(lower.len());
            let tail = lower.get(start..end).unwrap_or_default();
            let mut run = String::new();
            for c in tail.chars() {
                if c.is_ascii_digit() {
                    run.push(c);
                } else if run.len() >= min {
                    break;
                } else {
                    run.clear();
                }
            }
            if run.len() >= min {
                ids.push(run);
            }
            from = start;
        }
        ids
    };
    for d in ids_after("zenodo", 5, 24) {
        out.push(format!("zenodo:{d}"));
    }
    for d in ids_after("figshare", 6, 48) {
        out.push(format!("figshare:{d}"));
    }
    for d in ids_after("dandi", 6, 12) {
        if d.len() == 6 {
            out.push(format!("dandi:{d}"));
        }
    }
    for d in ids_after("pxd", 6, 6) {
        if d.len() == 6 {
            out.push(format!("pride:PXD{d}"));
        }
    }
    for d in ids_after("mtbls", 1, 8) {
        out.push(format!("metabolights:MTBLS{d}"));
    }
    for d in ids_after("msv", 9, 9) {
        out.push(format!("massive:MSV{d}"));
    }
    if lower.contains("nmrxiv") {
        for d in ids_after("project p", 1, 5) {
            out.push(format!("nmrxiv:P{d}"));
        }
    }
    // Dataverse persistent ids: `doi:10.7910/DVN/QE5CEQ`, `doi:10.15139/S3/4ESRC1`
    if lower.contains("dataverse") {
        let mut from = 0;
        while let Some(i) = lower[from..].find("doi:10.") {
            let start = from + i + 4;
            let doi: String = lower[start..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '/' | '-'))
                .collect();
            out.push(format!("dataverse:{}", doi.trim_end_matches('.')));
            from = start;
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Every source record an entry comes from: its URL's and the ones its `source` names.
fn keys_of(e: &Entry) -> Vec<String> {
    let mut k = source_keys(&e.source);
    k.extend(record_key(&e.url));
    k.sort();
    k.dedup();
    k
}

/// A GitHub repository is one record, except ProteoWizard's: it collects test data from
/// unrelated contributors, and its top-level trees are separate records (`pwiz/data/...`, the
/// vendor-reader test data, against `pwiz_tools/...`, Skyline's test data): `github:owner/repo`,
/// or `github:proteowizard/pwiz#<top-level directory>`. `top` is the position of the top-level
/// directory in `rest` (after `owner/repo/<ref>` for raw URLs, `owner/repo/blob/<ref>` else).
fn github_record(rest: &str, top: usize) -> String {
    let parts: Vec<&str> = rest.split('/').collect();
    let repo = parts.iter().take(2).copied().collect::<Vec<_>>().join("/");
    match parts.get(top) {
        Some(dir) if repo == "proteowizard/pwiz" && !dir.is_empty() => {
            format!("github:{repo}#{dir}")
        }
        _ => format!("github:{repo}"),
    }
}

/// File names a vendor gives every acquisition of a directory data set: a Bruker `.d` folder's
/// `analysis.tdf`, an Agilent MassHunter folder's `MSScan.bin`, a ChemStation `.D` folder's
/// `Report.TXT`, a MIRAX slide's `Slidedat.ini`, an Olympus VSI stack's `frame_t.ets`. A held-out
/// input or companion unpacked from a bundle can carry one, and format notes cite them, so they
/// identify nothing.
const VENDOR_FIXED_NAMES: &[&str] = &[
    "analysis.tdf",
    "analysis.tdf_bin",
    "analysis.tsf",
    "analysis.baf",
    "MSScan.bin",
    "MSProfile.bin",
    "MSPeak.bin",
    "_HEADER.TXT",
    "_extern.inf",
    "Report.TXT",
    "RESULTS.CSV",
    "Slidedat.ini",
    "frame_t.ets",
];

/// A name in `VENDOR_FIXED_NAMES`, or a MIRAX slide's numbered data file (`Data0000.dat`).
fn vendor_fixed_name(base: &str) -> bool {
    let numbered_mirax = base.len() == 12
        && base[..4].eq_ignore_ascii_case("data")
        && base[4..8].bytes().all(|b| b.is_ascii_digit())
        && base[8..].eq_ignore_ascii_case(".dat");
    numbered_mirax
        || VENDOR_FIXED_NAMES
            .iter()
            .any(|n| n.eq_ignore_ascii_case(base))
}

/// Strings that identify a held-out entry when they appear in a document.
fn needles(e: &Entry) -> Vec<String> {
    let mut out = vec![e.id.clone()];
    if !e.url.is_empty() {
        out.push(e.url.clone());
    }
    let base = e
        .filename
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default();
    // Generic member names (`fid`, `1`, `acqus`, `data.ms`) would match anything; only
    // distinctive file names count. The files of a directory data set (`role = "part"`) carry the
    // vendor's fixed names (`Index.idx.xml`, `MeasurementData.mlf`, `r01c01f01p01-ch1sk1fk1fl1.tiff`)
    // that every format note cites; their ids and URLs still count, and so does the folder's id.
    if e.role != "part" && base.len() >= 10 && base.contains('.') && !vendor_fixed_name(base) {
        out.push(base.to_string());
    }
    out
}

/// How a source record is written in a document: the token to look for, and words one of which
/// must be on the same line when the token is a bare number (a Zenodo, figshare, EMPIAR or DANDI
/// record is only a number, and numbers turn up everywhere). Records without a number or an
/// accession (a GitHub repository, an OME sample directory) are cited by name in prose and are
/// not looked for.
fn record_token(key: &str) -> Option<(String, &'static [&'static str])> {
    let (kind, id) = key.split_once(':')?;
    let bare = |min: usize, words: &'static [&'static str]| {
        (id.len() >= min && id.bytes().all(|b| b.is_ascii_digit())).then(|| (id.to_string(), words))
    };
    match kind {
        "zenodo" => bare(5, &["zenodo"]),
        "figshare" => bare(6, &["figshare"]),
        "dandi" => bare(6, &["dandi"]),
        "empiar" => bare(5, &["empiar"]),
        "pride" | "metabolights" | "massive" | "bioimage-archive" | "biostudies"
        | "flowrepository" => Some((id.to_string(), &[])),
        _ => None,
    }
}

/// Citations of a held-out record's number that were already in the notes when the check learned
/// to look for them (2026-10-06), as `(document, record)`. The check lists them and does not
/// fail on them. A provenance log is history, so the owner decides what to do about them, for
/// example whether the record's entry should carry `exposed`. A new citation fails the check, and
/// so does an entry here that no longer matches.
const KNOWN_CITATIONS: &[(&str, &str)] = &[
    ("docs/formats/bench-instruments-survey.md", "zenodo:6754439"),
    ("docs/formats/czi.md", "zenodo:19047136"),
    ("docs/provenance/czi.md", "zenodo:19047136"),
];

/// A line that says a record is held out or reserved. Naming a reserved record in order to say
/// that no development file comes from it, or that a development record is its sibling, is how
/// the logs record rule 3, so such a line is not a citation.
fn says_held_out(line: &str) -> bool {
    let line = line.to_ascii_lowercase();
    ["held out", "held-out", "heldout", "reserve"]
        .iter()
        .any(|w| line.contains(w))
}

/// True when `line` holds `token` as a whole word (not inside a longer number or name), and, if
/// `words` is not empty, one of `words`. Both are compared in lower case.
fn line_cites(line: &str, token: &str, words: &[&str]) -> bool {
    let line = line.to_ascii_lowercase();
    let token = token.to_ascii_lowercase();
    if !words.is_empty() && !words.iter().any(|w| line.contains(w)) {
        return false;
    }
    let mut from = 0;
    while let Some(i) = line[from..].find(&token) {
        let start = from + i;
        let end = start + token.len();
        let before = line[..start].chars().next_back();
        let after = line[end..].chars().next();
        if !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric) {
            return true;
        }
        from = start + 1;
    }
    false
}

fn files_under(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for e in rd.filter_map(std::result::Result::ok) {
        let p = e.path();
        if p.is_dir() {
            files_under(&p, ext, out);
        } else if p.extension().is_some_and(|x| x == ext) {
            out.push(p);
        }
    }
}

/// Documents that must never cite a held-out file.
fn guarded_documents(root: &Path) -> Vec<PathBuf> {
    let mut docs = Vec::new();
    files_under(&root.join("docs/provenance"), "md", &mut docs);
    files_under(&root.join("docs/formats"), "md", &mut docs);
    if let Ok(rd) = fs::read_dir(root.join("crates")) {
        for c in rd.filter_map(std::result::Result::ok) {
            files_under(&c.path().join("src"), "rs", &mut docs);
        }
    }
    docs.sort();
    docs
}

/// All problems found (empty when the held-out set is clean).
fn problems(root: &Path, manifest_text: &str) -> Result<Vec<String>> {
    let m: Manifest = toml::from_str(manifest_text).context("parse corpus/manifest.toml")?;
    let held: Vec<&Entry> = m.file.iter().filter(|e| e.tier == TIER).collect();
    let mut out = Vec::new();
    // 3. consistency
    for e in m.file.iter().filter(|e| e.role == ROLE && e.tier != TIER) {
        out.push(format!(
            "{}: role = \"{ROLE}\" but tier = {:?} (held-out inputs are on the held-out tier)",
            e.id, e.tier
        ));
    }
    for e in &held {
        if !matches!(
            e.role.as_str(),
            "heldout" | "oracle-export" | "bundle" | "companion" | "part"
        ) {
            out.push(format!(
                "{}: held-out entry with role {:?} (expected heldout, oracle-export, bundle, companion or part)",
                e.id, e.role
            ));
        }
    }
    for e in m.file.iter().filter(|e| e.exposed.is_some()) {
        if let Some(why) = exposure_problem(e) {
            out.push(format!("{}: {why}", e.id));
        }
    }
    let held_ids: BTreeSet<&str> = held.iter().map(|e| e.id.as_str()).collect();
    for e in m.file.iter().filter(|e| e.tier != TIER) {
        if held_ids.contains(e.id.as_str()) {
            out.push(format!(
                "{}: the id is used both in the held-out set and in the development corpus",
                e.id
            ));
        }
    }
    // 2. disjoint sources
    let mut held_records: BTreeMap<String, &str> = BTreeMap::new();
    for e in &held {
        for k in keys_of(e) {
            held_records.entry(k).or_insert(e.id.as_str());
        }
    }
    for e in m.file.iter().filter(|e| e.tier != TIER) {
        for k in keys_of(e) {
            if let Some(h) = held_records.get(&k) {
                out.push(format!(
                    "{}: comes from {k}, the source of held-out file {h}; a record is either held out entirely or not at all",
                    e.id
                ));
            }
        }
    }
    // 1. no citations
    let needles: Vec<(String, &str)> = held
        .iter()
        .flat_map(|e| needles(e).into_iter().map(move |n| (n, e.id.as_str())))
        .collect();
    for doc in guarded_documents(root) {
        let text = fs::read_to_string(&doc).with_context(|| format!("read {}", doc.display()))?;
        let rel = doc.strip_prefix(root).unwrap_or(&doc).display().to_string();
        for (needle, id) in &needles {
            if let Some(line) = text.lines().position(|l| l.contains(needle.as_str())) {
                out.push(format!(
                    "{rel}:{}: cites held-out file {id} ({needle:?}); held-out files must never be used to develop or debug a reader",
                    line + 1
                ));
            }
        }
    }
    // 1b. no citation of a reserved source record by its number. Records of exposed entries were
    // developed on before the draw, so citing them is expected. A sibling record has another
    // number and is not found.
    let exposed_records: BTreeSet<String> = held
        .iter()
        .filter(|e| e.exposed.is_some())
        .flat_map(|e| keys_of(e))
        .collect();
    let mut records: BTreeMap<String, (&str, String, &'static [&'static str])> = BTreeMap::new();
    for e in &held {
        for k in keys_of(e) {
            if exposed_records.contains(&k) {
                continue;
            }
            if let Some((token, words)) = record_token(&k) {
                records.entry(k).or_insert((e.id.as_str(), token, words));
            }
        }
    }
    let mut seen_known: BTreeSet<(&str, &str)> = BTreeSet::new();
    for doc in guarded_documents(root) {
        let text = fs::read_to_string(&doc).with_context(|| format!("read {}", doc.display()))?;
        let rel = doc.strip_prefix(root).unwrap_or(&doc).display().to_string();
        for (key, (id, token, words)) in &records {
            let cited = text
                .lines()
                .position(|l| line_cites(l, token, words) && !says_held_out(l));
            if let Some(line) = cited {
                if let Some(k) = KNOWN_CITATIONS
                    .iter()
                    .find(|(path, k)| *path == rel && k == key)
                {
                    seen_known.insert(*k);
                    continue;
                }
                out.push(format!(
                    "{rel}:{}: cites {key}, the source record of held-out file {id}; a held-out record is not used for development (another record by the same lab is fine, rule 3)",
                    line + 1
                ));
            }
        }
    }
    for (path, key) in KNOWN_CITATIONS {
        if records.contains_key(*key)
            && !seen_known.contains(&(*path, *key))
            && root.join(path).exists()
        {
            out.push(format!(
                "{path}: no longer cites {key}; remove it from KNOWN_CITATIONS in xtask/src/heldout.rs"
            ));
        }
    }
    Ok(out)
}

/// Why an `exposed` value is not acceptable, if it is not: it belongs on held-out inputs only and
/// reads `<YYYY-MM-DD>: <statement>`.
fn exposure_problem(e: &Entry) -> Option<String> {
    let text = e.exposed.as_deref()?;
    if e.tier != TIER || e.role != ROLE {
        return Some(format!(
            "`exposed` is only for held-out inputs (tier and role \"{ROLE}\"), not tier {:?} role {:?}",
            e.tier, e.role
        ));
    }
    let (date, rest) = text.split_once(':').unwrap_or(("", ""));
    let dated = date.len() == 10
        && date.bytes().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        });
    if !dated || rest.trim().len() < 20 {
        return Some(
            "`exposed` must read \"YYYY-MM-DD: who developed on which files of this record, and what was inferred from them\""
                .into(),
        );
    }
    None
}

/// Held-out inputs with a recorded development exposure: (id, statement).
fn exposed_entries(manifest_text: &str) -> Result<Vec<(String, String)>> {
    let m: Manifest = toml::from_str(manifest_text).context("parse corpus/manifest.toml")?;
    Ok(m.file
        .into_iter()
        .filter_map(|e| e.exposed.map(|x| (e.id, x)))
        .collect())
}

pub fn check(root: &Path) -> Result<()> {
    let text = fs::read_to_string(root.join("corpus/manifest.toml"))
        .context("read corpus/manifest.toml")?;
    let found = problems(root, &text)?;
    let n = toml::from_str::<Manifest>(&text)?
        .file
        .iter()
        .filter(|e| e.tier == TIER)
        .count();
    let exposed = exposed_entries(&text)?;
    if found.is_empty() {
        println!(
            "held-out set: {n} entries; no provenance log, format note or reader cites one, and no development file shares a source record"
        );
        for (path, key) in KNOWN_CITATIONS {
            println!("known citation, for the owner to decide: {path} names {key}");
        }
        if !exposed.is_empty() {
            println!(
                "{} held-out inputs are exposed (developed on before the draw reserved their record; kept, but not counted toward generalization):",
                exposed.len()
            );
            for (id, why) in &exposed {
                println!("    {id}: {why}");
            }
        }
        return Ok(());
    }
    for p in &found {
        println!("    {p}");
    }
    bail!(
        "{} held-out violations (see docs/benchmark/heldout.md: held-out files are for measuring, never for developing)",
        found.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"
[[file]]
id = "dev-a"
tier = "smoke"
role = "input"
filename = "dev-a.czi"
url = "https://zenodo.org/api/records/111/files/a.czi/content"

[[file]]
id = "ho-zenodo222-b"
tier = "heldout"
role = "heldout"
filename = "heldout/ho-zenodo222-b/sample_b_image.czi"
url = "https://zenodo.org/api/records/222/files/sample_b_image.czi/content"
"#;

    fn tmp_root(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("xtask-heldout-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("docs/provenance")).unwrap();
        fs::create_dir_all(d.join("docs/formats")).unwrap();
        d
    }

    #[test]
    fn record_keys() {
        assert_eq!(
            record_key("https://zenodo.org/api/records/7015307/files/x.czi/content").as_deref(),
            Some("zenodo:7015307")
        );
        assert_eq!(
            record_key("https://zenodo.org/records/123/files/x").as_deref(),
            Some("zenodo:123")
        );
        assert_eq!(
            record_key("https://ftp.ebi.ac.uk/biostudies/fire/S-BSST/202/S-BSST1202/Files/a.zvi")
                .as_deref(),
            Some("biostudies:S-BSST1202")
        );
        assert_eq!(
            record_key("https://ftp.ebi.ac.uk/pub/databases/metabolights/studies/public/MTBLS20/FILES/a.RAW")
                .as_deref(),
            Some("metabolights:MTBLS20")
        );
        assert_eq!(
            record_key("https://ftp.pride.ebi.ac.uk/pride/data/archive/2012/03/PXD000001/x.mzML")
                .as_deref(),
            Some("pride:PXD000001")
        );
        assert_eq!(
            record_key("https://downloads.openmicroscopy.org/images/ND2/aryeh/b.nd2").as_deref(),
            Some("ome-sample:nd2/aryeh")
        );
        assert_eq!(
            record_key("https://raw.githubusercontent.com/swharden/pyABF/abc/data/a.abf")
                .as_deref(),
            Some("github:swharden/pyabf")
        );
        assert_eq!(
            record_key("https://cellpainting-gallery.s3.amazonaws.com/cpg0016-jump/source_4/images/b/p/x.tiff")
                .as_deref(),
            Some("cell-painting-gallery:cpg0016-jump/source_4")
        );
        assert_eq!(
            record_key("https://ftp.ebi.ac.uk/biostudies/fire/S-BIAD/152/S-BIAD2152/Files/a/b.TIF")
                .as_deref(),
            Some("bioimage-archive:S-BIAD2152")
        );
        assert_eq!(
            record_key("https://raw.githubusercontent.com/ProteoWizard/pwiz/83836d2/pwiz_tools/Skyline/a.mzMLb")
                .as_deref(),
            Some("github:proteowizard/pwiz#pwiz_tools")
        );
        assert_eq!(
            record_key("https://raw.githubusercontent.com/ProteoWizard/pwiz/699dd48/pwiz/data/vendor_readers/Bruker/a.tar.bz2")
                .as_deref(),
            Some("github:proteowizard/pwiz#pwiz")
        );
        assert_eq!(record_key("https://example.org/x"), None);
    }

    #[test]
    fn source_texts_name_records() {
        assert_eq!(
            source_keys("Figshare 10.6084/m9.figshare.29908870.v1 'Additional file 5'"),
            vec!["figshare:29908870"]
        );
        assert_eq!(
            source_keys("Cardiff University Research Portal (Figshare) 29988259 'Raw data'"),
            vec!["figshare:29988259"]
        );
        assert_eq!(
            source_keys("DANDI 000293 (version 0.220708.1652): UHN recordings"),
            vec!["dandi:000293"]
        );
        assert_eq!(
            source_keys(
                "nmrXiv study S275 'nmrxiv-pt-succrose' (project P52), https://nmrxiv.org/sample/S275"
            ),
            vec!["nmrxiv:P52"]
        );
        assert_eq!(
            source_keys("Harvard Dataverse doi:10.7910/DVN/QE5CEQ 'Role of Purinergic Receptors'"),
            vec!["dataverse:10.7910/dvn/qe5ceq"]
        );
        assert_eq!(
            source_keys("Zenodo record 14316687 (doi:10.5281/zenodo.14316687, concept 10407761)"),
            vec!["zenodo:14316687"]
        );
        assert_eq!(
            source_keys("PRIDE PXD074950, B1_S2-E1_1_3363.d.zip; MassIVE MSV000085661"),
            vec!["massive:MSV000085661", "pride:PXD074950"]
        );
        assert!(source_keys("tlambert03/nd2 test data").is_empty());
        assert_eq!(
            record_key("https://gin.g-node.org/NeuralEnsemble/ephy_testing_data/raw/bf392aa/x.ncs")
                .as_deref(),
            Some("gin:neuralensemble/ephy_testing_data")
        );
    }

    #[test]
    fn a_development_file_naming_a_heldout_record_in_its_source_fails() {
        let root = tmp_root("source");
        let m = format!(
            "{MANIFEST}\n[[file]]\nid = \"ho-figshare777777-x\"\ntier = \"heldout\"\nrole = \"heldout\"\nfilename = \"heldout/ho-figshare777777-x/x.fcs\"\nurl = \"https://ndownloader.figshare.com/files/1\"\nsource = \"Figshare 777777 'held out'\"\n\n[[file]]\nid = \"dev-d\"\ntier = \"standard\"\nrole = \"input\"\nfilename = \"d.fcs\"\nurl = \"https://ndownloader.figshare.com/files/2\"\nsource = \"Figshare 777777 'held out', another file\"\n"
        );
        let p = problems(&root, &m).unwrap();
        assert!(p.iter().any(|x| x.contains("figshare:777777")), "{p:?}");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn clean_set_passes() {
        let root = tmp_root("clean");
        fs::write(root.join("docs/provenance/czi.md"), "used dev-a.czi only\n").unwrap();
        assert!(problems(&root, MANIFEST).unwrap().is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_provenance_log_citing_a_heldout_file_fails() {
        let root = tmp_root("cite");
        fs::write(
            root.join("docs/provenance/czi.md"),
            "2026-10-01: fixed tiles, corpus ho-zenodo222-b\n",
        )
        .unwrap();
        let p = problems(&root, MANIFEST).unwrap();
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].starts_with("docs/provenance/czi.md:1:"), "{p:?}");
        fs::write(
            root.join("docs/provenance/czi.md"),
            "looked at sample_b_image.czi\n",
        )
        .unwrap();
        assert_eq!(problems(&root, MANIFEST).unwrap().len(), 1);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn vendor_file_names_of_a_heldout_folder_are_not_needles() {
        let root = tmp_root("parts");
        let m = format!(
            "{MANIFEST}\n[[file]]\nid = \"ho-plate-folder\"\ntier = \"heldout\"\nrole = \"heldout\"\nfilename = \"heldout/ho-plate-folder/P1\"\nurl = \"\"\n\n[[file]]\nid = \"ho-plate-folder-index\"\ntier = \"heldout\"\nrole = \"part\"\nfilename = \"heldout/ho-plate-folder/P1/MeasurementData.mlf\"\nurl = \"https://example.org/plates/P1/MeasurementData.mlf\"\n"
        );
        fs::write(
            root.join("docs/formats/cellvoyager.md"),
            "the plate index is MeasurementData.mlf\n",
        )
        .unwrap();
        assert!(problems(&root, &m).unwrap().is_empty());
        fs::write(
            root.join("docs/formats/cellvoyager.md"),
            "debugged on ho-plate-folder\n",
        )
        .unwrap();
        assert_eq!(problems(&root, &m).unwrap().len(), 1);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn vendor_fixed_names_identify_nothing() {
        assert!(vendor_fixed_name("analysis.tdf"));
        assert!(vendor_fixed_name("REPORT.TXT"));
        assert!(vendor_fixed_name("Data0012.dat"));
        assert!(!vendor_fixed_name("Data0012a.dat"));
        assert!(!vendor_fixed_name("B_Blank_Whatman_neg.d"));
    }

    #[test]
    fn a_development_file_from_a_heldout_record_fails() {
        let root = tmp_root("record");
        let m = format!(
            "{MANIFEST}\n[[file]]\nid = \"dev-c\"\ntier = \"standard\"\nrole = \"input\"\nfilename = \"c.czi\"\nurl = \"https://zenodo.org/records/222/files/c.czi\"\n"
        );
        let p = problems(&root, &m).unwrap();
        assert!(p.iter().any(|x| x.contains("zenodo:222")), "{p:?}");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn exposure_is_a_dated_statement_on_a_heldout_input() {
        let root = tmp_root("exposed");
        let ok = MANIFEST.replace(
            "role = \"heldout\"\n",
            "role = \"heldout\"\nexposed = \"2026-09-26: development work used a sibling file; nothing inferred\"\n",
        );
        assert!(problems(&root, &ok).unwrap().is_empty());
        assert_eq!(exposed_entries(&ok).unwrap().len(), 1);
        let undated = MANIFEST.replace(
            "role = \"heldout\"\n",
            "role = \"heldout\"\nexposed = \"yes\"\n",
        );
        assert_eq!(problems(&root, &undated).unwrap().len(), 1);
        let on_dev = MANIFEST.replace(
            "role = \"input\"\n",
            "role = \"input\"\nexposed = \"2026-09-26: development work used this file; nothing inferred\"\n",
        );
        let p = problems(&root, &on_dev).unwrap();
        assert!(p.iter().any(|x| x.starts_with("dev-a:")), "{p:?}");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_provenance_log_citing_a_heldout_record_number_fails() {
        let root = tmp_root("record-number");
        let m = format!(
            "{MANIFEST}\n[[file]]\nid = \"ho-mtbls-x\"\ntier = \"heldout\"\nrole = \"heldout\"\nfilename = \"heldout/ho-mtbls-x/x.raw\"\nurl = \"https://ftp.ebi.ac.uk/pub/databases/metabolights/studies/public/MTBLS12457/FILES/x.raw\"\n\n[[file]]\nid = \"ho-fig-y\"\ntier = \"heldout\"\nrole = \"heldout\"\nfilename = \"heldout/ho-fig-y/y.fcs\"\nurl = \"https://ndownloader.figshare.com/files/9\"\nsource = \"Figshare 31095109 'held out'\"\n"
        );
        let doc = root.join("docs/provenance/x.md");
        // a sibling record, an unrelated number and a longer number are fine
        fs::write(
            &doc,
            "figshare 31095110 (sibling)\nrun 31095109 of the batch\nMTBLS124570\nzenodo 22222\n",
        )
        .unwrap();
        assert!(problems(&root, &m).unwrap().is_empty());
        for (text, key) in [
            ("see MTBLS12457 for the layout\n", "metabolights:MTBLS12457"),
            ("a file of mtbls12457.\n", "metabolights:MTBLS12457"),
            ("from figshare 31095109 (v2)\n", "figshare:31095109"),
            ("doi 10.6084/m9.figshare.31095109.v1\n", "figshare:31095109"),
        ] {
            fs::write(&doc, text).unwrap();
            let p = problems(&root, &m).unwrap();
            assert_eq!(p.len(), 1, "{text}: {p:?}");
            assert!(p[0].starts_with("docs/provenance/x.md:1:"), "{p:?}");
            assert!(p[0].contains(key), "{p:?}");
        }
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_line_saying_the_record_is_held_out_is_not_a_citation() {
        let root = tmp_root("disclosure");
        let m = format!(
            "{MANIFEST}\n[[file]]\nid = \"ho-zenodo666666-x\"\ntier = \"heldout\"\nrole = \"heldout\"\nfilename = \"heldout/ho-zenodo666666-x/x.czi\"\nurl = \"https://zenodo.org/records/666666/files/x.czi\"\n"
        );
        let doc = root.join("docs/provenance/czi.md");
        fs::write(
            &doc,
            "Held out (never opened while developing): Zenodo 666666\nsibling of Zenodo 666666, which a draw reserves\n",
        )
        .unwrap();
        assert!(problems(&root, &m).unwrap().is_empty());
        fs::write(
            &doc,
            "Held out: other files.\nused Zenodo 666666 for tiles\n",
        )
        .unwrap();
        let p = problems(&root, &m).unwrap();
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].starts_with("docs/provenance/czi.md:2:"), "{p:?}");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_bare_number_needs_its_repository_on_the_line() {
        assert!(line_cites("Zenodo record 4563053", "4563053", &["zenodo"]));
        assert!(line_cites("10.5281/zenodo.4563053", "4563053", &["zenodo"]));
        assert!(!line_cites("offset 4563053", "4563053", &["zenodo"]));
        assert!(!line_cites("zenodo 45630531", "4563053", &["zenodo"]));
        assert!(line_cites("S-BIAD1129", "s-biad1129", &[]));
        assert!(!line_cites("S-BIAD11290", "s-biad1129", &[]));
    }

    #[test]
    fn the_record_of_an_exposed_entry_may_be_cited() {
        let root = tmp_root("exposed-record");
        let m = format!(
            "{MANIFEST}\n[[file]]\nid = \"ho-zenodo444444-x\"\ntier = \"heldout\"\nrole = \"heldout\"\nfilename = \"heldout/ho-zenodo444444-x/x.czi\"\nurl = \"https://zenodo.org/records/444444/files/x.czi\"\nexposed = \"2026-09-26: development work used a sibling file; nothing inferred\"\n\n[[file]]\nid = \"ho-zenodo555555-x\"\ntier = \"heldout\"\nrole = \"heldout\"\nfilename = \"heldout/ho-zenodo555555-x/x.czi\"\nurl = \"https://zenodo.org/records/555555/files/x.czi\"\n"
        );
        let doc = root.join("docs/provenance/czi.md");
        fs::write(&doc, "Zenodo record 444444 was read for the tile layout\n").unwrap();
        assert!(problems(&root, &m).unwrap().is_empty());
        fs::write(&doc, "Zenodo record 555555 was read for the tile layout\n").unwrap();
        assert_eq!(problems(&root, &m).unwrap().len(), 1);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_repository_is_clean() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let text = fs::read_to_string(root.join("corpus/manifest.toml")).unwrap();
        let p = problems(&root, &text).unwrap();
        assert!(p.is_empty(), "held-out violations:\n{}", p.join("\n"));
    }
}
