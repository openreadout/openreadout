//! `cargo xtask heldout-check`: keep the held-out corpus held out.
//!
//! Held-out files (`tier = "heldout"` in corpus/manifest.toml; their inputs carry
//! `role = "heldout"`) measure whether the readers generalize to files nobody developed against
//! (docs/benchmark/heldout.md). They must never be used to develop or debug a reader, so:
//!
//! 1. no provenance log (`docs/provenance/*.md`), format note (`docs/formats/*.md`) or reader source
//!    (`crates/*/src/**/*.rs`) may cite a held-out file: its corpus id, its download URL or its
//!    (distinctive) file name;
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
    if e.role != "part" && base.len() >= 10 && base.contains('.') {
        out.push(base.to_string());
    }
    out
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
    fn the_repository_is_clean() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let text = fs::read_to_string(root.join("corpus/manifest.toml")).unwrap();
        let p = problems(&root, &text).unwrap();
        assert!(p.is_empty(), "held-out violations:\n{}", p.join("\n"));
    }
}
