//! Batch tables, sample-sheet joins, group summaries and `link`, against independent ground
//! truth (`oracle/batch.py` → `corpus/oracle/batch/`):
//!
//! - `analyze gate --tidy --median`: population counts and medians of the flowkit-8c-ics samples
//!   against FlowKit (counts exact; medians to 1e-9 relative);
//! - `table --tidy`: per-parameter events, median, mean, sd, min and max of FCS files against
//!   FlowIO scale values and NumPy; plate rows (one per well) against allotropy's ASM values;
//! - `stats --tidy`: per image and channel mean, min, max and median of ND2 files against nd2
//!   and NumPy; `trace --tidy`: per-sweep statistics of ABF files against pyABF;
//! - `batch summarize`: a seeded table against pandas and SciPy (Welch, Mann–Whitney), with and
//!   without replicate averaging;
//! - sample sheets and plate layouts written with openpyxl and csv, read back;
//! - `link`: vendor files and their depositor-made mzML conversions (pairs recorded in
//!   `corpus/manifest.toml`: the same id with roles `input` and `oracle-export`), staged under
//!   neutral names among unrelated files; pairwise precision and recall are printed, precision
//!   must be 1 (links are conservative).
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test batch -- --nocapture`
#![cfg(feature = "corpus")]
#![allow(clippy::many_single_char_names)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use openreadout_batch::measures::{GateMeasure, StatsMeasure, TableMeasure, TraceMeasure};
use openreadout_batch::run::BatchRequest;
use openreadout_batch::summarize::{SummarizeRequest, TestKind};
use openreadout_batch::{LinkOptions, Measure, Table, Value};
use openreadout_core::Registry;
use serde::Deserialize;
use serde_json::Value as J;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn files_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root().join("corpus/files"), PathBuf::from)
}

#[derive(Deserialize)]
struct Manifest {
    file: Vec<Entry>,
}
#[derive(Deserialize, Clone)]
struct Entry {
    id: String,
    filename: String,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    source: Option<String>,
}

fn manifest() -> Vec<Entry> {
    let m: Manifest =
        toml::from_str(&std::fs::read_to_string(root().join("corpus/manifest.toml")).unwrap())
            .unwrap();
    m.file
}

fn input_path(id: &str) -> Option<PathBuf> {
    let e = manifest()
        .into_iter()
        .find(|e| e.id == id && e.role.as_deref().unwrap_or("input") == "input")?;
    let p = files_dir().join(e.filename);
    p.exists().then_some(p)
}

fn oracle(name: &str) -> J {
    serde_json::from_str(
        &std::fs::read_to_string(root().join("corpus/oracle/batch").join(name)).unwrap(),
    )
    .unwrap()
}

fn registry() -> Registry {
    Registry::new()
        .with(Box::new(openreadout_czi::CziReader))
        .with(Box::new(openreadout_lif::LifReader))
        .with(Box::new(openreadout_nd2::Nd2Reader))
        .with(Box::new(openreadout_thermo::ThermoRawReader))
        .with(Box::new(openreadout_fcs::FcsReader))
        .with(Box::new(openreadout_mzml::ImzmlReader))
        .with(Box::new(openreadout_mzml::MzmlReader))
        .with(Box::new(openreadout_mzml::MzxmlReader))
        .with(Box::new(openreadout_bruker_tims::BrukerTimsReader))
        .with(Box::new(openreadout_abf::AbfReader))
        .with(Box::new(openreadout_agilent_ms::AgilentMsReader))
        .with(Box::new(openreadout_sciex::SciexWiffReader))
        .with(Box::new(openreadout_chrom::ChemStationReader))
        .with(Box::new(openreadout_chrom::AndiReader))
        .with(Box::new(openreadout_waters::WatersRawReader))
        .with(Box::new(openreadout_chrom::ShimadzuReader))
        .with(Box::new(openreadout_chrom::ChromeleonReader))
        .with(Box::new(openreadout_chrom::EmpowerArwReader))
        .with(Box::new(openreadout_tiff::TiffReader))
        .with(Box::new(openreadout_plate::PlateReader))
}

fn run(measure: &dyn Measure, inputs: Vec<PathBuf>) -> openreadout_batch::BatchResult {
    let mut req = BatchRequest::default();
    req.inputs = inputs;
    openreadout_batch::run(&registry(), measure, &req, None, None).unwrap()
}

fn close(ours: &Value, theirs: &J, rel: f64) -> bool {
    match (ours.as_f64(), theirs.as_f64()) {
        (Some(a), Some(b)) => (a - b).abs() <= rel * b.abs().max(1.0),
        (None, None) => true,
        _ => ours.is_null() && theirs.is_null(),
    }
}

/// Rows of `t` keyed by the text of `keys`.
fn index(t: &Table, keys: &[&str]) -> BTreeMap<Vec<String>, usize> {
    (0..t.rows.len())
        .map(|r| (keys.iter().map(|k| t.get(r, k).text()).collect(), r))
        .collect()
}

#[test]
fn gate_counts_and_medians_match_flowkit() {
    let o = oracle("gate-medians.json");
    let wsp = manifest()
        .into_iter()
        .find(|e| e.id == "flowkit-8c-ics")
        .map(|e| files_dir().join(e.filename))
        .unwrap();
    if !wsp.exists() {
        eprintln!("skip: corpus not fetched");
        return;
    }
    let params: Vec<String> = serde_json::from_value(o["parameters"].clone()).unwrap();
    let m = GateMeasure {
        gating_file: wsp,
        medians: params.clone(),
        ..Default::default()
    };
    let mut inputs = Vec::new();
    for s in o["samples"].as_array().unwrap() {
        inputs.push(input_path(s["fcs"].as_str().unwrap()).unwrap());
    }
    let res = run(&m, inputs.clone());
    assert_eq!(res.inputs.failed, 0, "{:?}", res.table.rows);
    let (mut compared, mut worst) = (0, 0.0f64);
    for (s, path) in o["samples"].as_array().unwrap().iter().zip(&inputs) {
        for p in s["populations"].as_array().unwrap() {
            let row = (0..res.table.rows.len())
                .find(|&r| {
                    res.table.get(r, "path").text() == path.display().to_string()
                        && res.table.get(r, "population").text() == p["path"].as_str().unwrap()
                })
                .unwrap_or_else(|| panic!("no row for {}", p["path"]));
            assert_eq!(
                res.table.get(row, "count").as_f64(),
                p["count"].as_f64(),
                "count {}",
                p["path"]
            );
            for name in &params {
                let ours = res.table.get(row, &format!("median:{name}"));
                let theirs = &p["medians"][name];
                if let (Some(a), Some(b)) = (ours.as_f64(), theirs.as_f64()) {
                    worst = worst.max((a - b).abs() / b.abs().max(1.0));
                }
                assert!(
                    close(ours, theirs, 1e-9),
                    "{} {name}: ours {ours:?} FlowKit {theirs}",
                    p["path"]
                );
                compared += 1;
            }
        }
    }
    println!("gate medians: {compared} values match FlowKit (max relative difference {worst:.1e})");
    assert!(compared > 50);
}

#[test]
fn fcs_table_summaries_match_flowio() {
    let o = oracle("fcs-summaries.json");
    let mut inputs = Vec::new();
    for f in o["files"].as_array().unwrap() {
        let Some(p) = input_path(f["id"].as_str().unwrap()) else {
            eprintln!("skip: corpus not fetched");
            return;
        };
        inputs.push(p);
    }
    let res = run(&TableMeasure::default(), inputs.clone());
    assert_eq!(res.inputs.failed, 0);
    let ix = index(&res.table, &["path", "parameter"]);
    let mut n = 0;
    for (f, path) in o["files"].as_array().unwrap().iter().zip(&inputs) {
        for p in f["parameters"].as_array().unwrap() {
            let key = vec![
                path.display().to_string(),
                p["parameter"].as_str().unwrap().to_string(),
            ];
            let r = *ix.get(&key).unwrap_or_else(|| panic!("no row for {key:?}"));
            for (ours, theirs) in [
                ("events", "events"),
                ("median", "median"),
                ("mean", "mean"),
                ("sd", "sd"),
                ("min", "min"),
                ("max", "max"),
            ] {
                assert!(
                    close(res.table.get(r, ours), &p[theirs], 1e-9),
                    "{key:?} {ours}: ours {:?} flowio {}",
                    res.table.get(r, ours),
                    p[theirs]
                );
                n += 1;
            }
        }
    }
    println!("FCS table summaries: {n} values match FlowIO + NumPy (1e-9)");
}

#[test]
fn plate_rows_match_allotropy() {
    let o = oracle("plate-values.json");
    let mut n = 0;
    for f in o["files"].as_array().unwrap() {
        let Some(path) = input_path(f["id"].as_str().unwrap()) else {
            eprintln!("skip: corpus not fetched");
            return;
        };
        let res = run(&TableMeasure::default(), vec![path]);
        assert_eq!(res.inputs.failed, 0);
        // every allotropy value appears for its well (a file may hold several plates/reads)
        for (well, vals) in f["wells"].as_object().unwrap() {
            let canon = openreadout_batch::well::parse(well).unwrap().name();
            let ours: Vec<f64> = (0..res.table.rows.len())
                .filter(|&r| res.table.get(r, "well").text() == canon)
                .filter_map(|r| res.table.get(r, "value").as_f64())
                .collect();
            for v in vals.as_array().unwrap() {
                let v = v.as_f64().unwrap();
                assert!(
                    ours.iter()
                        .any(|x| (x - v).abs() <= 1e-9 * v.abs().max(1.0)),
                    "{} {well}: allotropy {v}, ours {ours:?}",
                    f["id"]
                );
                n += 1;
            }
        }
    }
    println!("plate rows: {n} well values match allotropy");
}

#[test]
fn stats_rows_match_nd2() {
    let o = oracle("image-stats.json");
    let mut n = 0;
    for f in o["files"].as_array().unwrap() {
        let Some(path) = input_path(f["id"].as_str().unwrap()) else {
            eprintln!("skip: corpus not fetched");
            return;
        };
        let res = run(&StatsMeasure::default(), vec![path]);
        assert_eq!(res.inputs.failed, 0, "{}", f["id"]);
        let ix = index(&res.table, &["image", "channel"]);
        for row in f["rows"].as_array().unwrap() {
            let key = vec![row["image"].to_string(), row["channel"].to_string()];
            let r = *ix
                .get(&key)
                .unwrap_or_else(|| panic!("{} no row {key:?}", f["id"]));
            for k in ["count", "mean", "min", "max", "median"] {
                assert!(
                    close(res.table.get(r, k), &row[k], 1e-9),
                    "{} {key:?} {k}: ours {:?} nd2 {}",
                    f["id"],
                    res.table.get(r, k),
                    row[k]
                );
                n += 1;
            }
        }
    }
    println!("stats rows: {n} values match nd2 + NumPy");
}

#[test]
fn trace_rows_match_pyabf() {
    let o = oracle("trace-stats.json");
    let mut n = 0;
    for f in o["files"].as_array().unwrap() {
        let Some(path) = input_path(f["id"].as_str().unwrap()) else {
            eprintln!("skip: corpus not fetched");
            return;
        };
        let m = TraceMeasure {
            trace: Some(0),
            channels: vec![0],
            ..Default::default()
        };
        let res = run(&m, vec![path]);
        assert_eq!(res.inputs.failed, 0, "{}", f["id"]);
        let ix = index(&res.table, &["sweep"]);
        for row in f["rows"].as_array().unwrap() {
            let r = *ix
                .get(&vec![row["sweep"].to_string()])
                .unwrap_or_else(|| panic!("{} sweep {}", f["id"], row["sweep"]));
            for (ours, theirs, rel) in [
                ("samples", "samples", 0.0),
                ("mean", "mean", 1e-6),
                ("std", "std", 1e-6),
                ("min", "min", 1e-6),
                ("max", "max", 1e-6),
            ] {
                assert!(
                    close(res.table.get(r, ours), &row[theirs], rel),
                    "{} sweep {} {ours}: ours {:?} pyabf {}",
                    f["id"],
                    row["sweep"],
                    res.table.get(r, ours),
                    row[theirs]
                );
                n += 1;
            }
        }
    }
    println!("trace rows: {n} values match pyABF (1e-6 relative: pyABF scales float32 samples)");
}

#[test]
fn summaries_match_pandas_and_scipy() {
    let o = oracle("summarize.json");
    let (t, _) = openreadout_batch::write::read_table(
        &root().join("corpus/oracle/batch/summarize-input.csv"),
    )
    .unwrap();
    let mut n = 0;
    for (key, replicate) in [
        ("plain", None),
        ("replicate", Some("replicate".to_string())),
    ] {
        for (test, prefix) in [(TestKind::Welch, "welch"), (TestKind::MannWhitney, "mwu")] {
            let req = {
                let mut r = SummarizeRequest::default();
                r.by = vec!["condition".into()];
                r.values = vec!["mean".into()];
                r.replicate = replicate.clone();
                r.test = Some(test);
                r.control = Some("ctrl".into());
                r
            };
            let s = openreadout_batch::summarize(&t, "stats", &req).unwrap();
            assert_eq!(s.group_by, ["condition", "channel_name"]);
            assert_eq!(s.rows_excluded, 1, "the failed row");
            let ix = index(&s.table, &["condition", "channel_name"]);
            for e in o[key].as_array().unwrap() {
                let k = vec![
                    e["condition"].as_str().unwrap().to_string(),
                    e["channel_name"].as_str().unwrap().to_string(),
                ];
                let r = ix[&k];
                for c in [
                    "n",
                    "mean",
                    "sd",
                    "sem",
                    "median",
                    "min",
                    "max",
                    "cv_percent",
                ] {
                    assert!(
                        close(s.table.get(r, c), &e[c], 1e-9),
                        "{key} {k:?} {c}: ours {:?} pandas {}",
                        s.table.get(r, c),
                        e[c]
                    );
                    n += 1;
                }
                if e.get(format!("{prefix}_p")).is_some() {
                    let (stat, p) = match test {
                        TestKind::Welch => ("welch_t", "welch_p"),
                        TestKind::MannWhitney => ("mwu_u", "mwu_p"),
                    };
                    assert!(
                        close(s.table.get(r, "statistic"), &e[stat], 1e-9),
                        "{key} {k:?} {stat}"
                    );
                    assert!(
                        close(s.table.get(r, "p_value"), &e[p], 1e-7),
                        "{key} {k:?} {p}: ours {:?} scipy {}",
                        s.table.get(r, "p_value"),
                        e[p]
                    );
                    if test == TestKind::Welch {
                        assert!(close(s.table.get(r, "df"), &e["welch_df"], 1e-9));
                    }
                    n += 2;
                }
            }
        }
    }
    println!("summaries: {n} values match pandas and SciPy");
}

#[test]
fn sample_sheets_and_layouts_read_as_written() {
    let o = oracle("sheets.json");
    let dir = root().join("corpus/oracle/batch");
    let expect_layout: Vec<Vec<String>> =
        serde_json::from_value(o["layout"]["rows"].clone()).unwrap();
    for f in ["layout.csv", "layout.xlsx"] {
        let s = openreadout_batch::sheet::read(&dir.join(f), None).unwrap();
        assert_eq!(s.columns, ["well", "condition", "dose_uM"], "{f}");
        assert_eq!(s.plate_wells, Some(96), "{f}");
        let rows: Vec<Vec<String>> = s.rows.clone();
        assert_eq!(rows, expect_layout, "{f}");
    }
    let s = openreadout_batch::sheet::read(&dir.join("samples.xlsx"), None).unwrap();
    assert_eq!(
        s.worksheet.as_deref(),
        Some("notes"),
        "first sheet by default"
    );
    let s = openreadout_batch::sheet::read(&dir.join("samples.xlsx"), Some("samples")).unwrap();
    let cols: Vec<String> = serde_json::from_value(o["samples_xlsx"]["columns"].clone()).unwrap();
    let rows: Vec<Vec<String>> = serde_json::from_value(o["samples_xlsx"]["rows"].clone()).unwrap();
    assert_eq!(s.columns, cols);
    assert_eq!(s.rows, rows);
    println!(
        "sheets: 2 layouts ({} wells) and 1 workbook table read as written",
        expect_layout.len()
    );
}

/// A vendor file (or directory) and the depositor's mzML conversion of the same acquisition.
fn conversion_pairs() -> Vec<(Entry, Entry)> {
    let m = manifest();
    let mut out = Vec::new();
    for e in m
        .iter()
        .filter(|e| e.role.as_deref() == Some("oracle-export"))
    {
        if !matches!(e.format.as_deref(), Some("mzml" | "mzxml")) {
            continue;
        }
        let Some(v) = m.iter().find(|x| {
            x.id == e.id && x.role.as_deref().unwrap_or("input") == "input" && x.format != e.format
        }) else {
            continue;
        };
        if files_dir().join(&v.filename).exists() && files_dir().join(&e.filename).exists() {
            out.push((v.clone(), e.clone()));
        }
    }
    out.sort_by(|a, b| a.0.id.cmp(&b.0.id));
    out
}

fn extension(name: &str) -> String {
    let base = name
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(name);
    base.rfind('.')
        .map_or_else(String::new, |i| base[i..].to_ascii_lowercase())
}

#[test]
fn link_finds_conversions_under_neutral_names() {
    let pairs = conversion_pairs();
    if pairs.len() < 3 {
        eprintln!("skip: corpus not fetched ({} pairs)", pairs.len());
        return;
    }
    let m = manifest();
    // Distractors: mass-spectrometry and flow inputs, at most one per depositor source and none
    // from a source that contributed a pair, so no distractor can be another copy or export of
    // a staged acquisition (the manifest's `source` is the depositor's study or test-data set).
    // A source without its parenthetical parts (`mobiusklein/mzdata test data (test/data/small.mzML),
    // pinned to …` and `… (…/small.mzML.gz), pinned to …` are one test-data set holding one
    // acquisition twice).
    let source_key = |s: Option<&str>| -> String {
        let mut out = String::new();
        let mut depth = 0usize;
        for ch in s.unwrap_or_default().chars() {
            match ch {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                c if depth == 0 => out.push(c),
                _ => {}
            }
        }
        out.split_whitespace().collect::<Vec<_>>().join(" ")
    };
    let pair_sources: BTreeSet<String> = pairs
        .iter()
        .map(|p| source_key(p.0.source.as_deref()))
        .collect();
    let mut used_sources: BTreeSet<String> = BTreeSet::new();
    let mut candidates: Vec<&Entry> = m
        .iter()
        .filter(|e| {
            e.role.as_deref().unwrap_or("input") == "input"
                && !pair_sources.contains(&source_key(e.source.as_deref()))
                // synthetic fixtures are derived from other corpus files (their `source` says
                // which), i.e. the same acquisition: not distractors
                && e.source.as_deref().is_some_and(|s| !s.starts_with("synthetic"))
                && matches!(e.format.as_deref(), Some("thermo-raw" | "mzml" | "fcs"))
                && !e.filename.contains('/')
                && files_dir().join(&e.filename).exists()
                && std::fs::metadata(files_dir().join(&e.filename))
                    .is_ok_and(|md| md.len() < 60_000_000)
        })
        .collect();
    candidates.sort_by(|a, b| a.id.cmp(&b.id));
    let mut distract: Vec<Entry> = Vec::new();
    for e in candidates {
        if used_sources.insert(source_key(e.source.as_deref())) {
            distract.push(e.clone());
        }
    }
    distract.truncate(24);
    let dir = tempfile::tempdir().unwrap();
    let mut truth: BTreeSet<(String, String)> = BTreeSet::new();
    let mut n = 0;
    let mut ids: BTreeMap<String, String> = BTreeMap::new();
    let mut stage = |e: &Entry| -> String {
        n += 1;
        let name = format!("run_{n:03}{}", extension(&e.filename));
        let sub = ["a", "b", "c"][n % 3];
        std::fs::create_dir_all(dir.path().join(sub)).unwrap();
        let dst = dir.path().join(sub).join(&name);
        std::os::unix::fs::symlink(files_dir().join(&e.filename), &dst).unwrap();
        // a Sciex .wiff keeps its .wiff.scan companion beside it, under the same name
        let scan = files_dir().join(format!("{}.scan", e.filename));
        if scan.exists() {
            std::os::unix::fs::symlink(&scan, dir.path().join(sub).join(format!("{name}.scan")))
                .unwrap();
        }
        let s = dst.display().to_string();
        ids.insert(
            s.clone(),
            format!("{} ({})", e.id, e.format.as_deref().unwrap_or("?")),
        );
        s
    };
    for (v, c) in &pairs {
        let a = stage(v);
        let b = stage(c);
        truth.insert(if a < b { (a, b) } else { (b, a) });
    }
    for d in &distract {
        stage(d);
    }
    let mut o = LinkOptions::default();
    o.request.inputs = vec![dir.path().to_path_buf()];
    let t0 = std::time::Instant::now();
    let out = openreadout_batch::link(&registry(), &o).unwrap();
    let secs = t0.elapsed().as_secs_f64();
    let mut predicted: BTreeSet<(String, String)> = BTreeSet::new();
    for g in &out.groups {
        let ms: Vec<&String> = g.members.iter().map(|m| &m.path).collect();
        for i in 0..ms.len() {
            for j in i + 1..ms.len() {
                let (a, b) = (ms[i].clone(), ms[j].clone());
                predicted.insert(if a < b { (a, b) } else { (b, a) });
            }
        }
    }
    let tp = predicted.intersection(&truth).count();
    let fp = predicted.len() - tp;
    let fnn = truth.len() - tp;
    let precision = if predicted.is_empty() {
        1.0
    } else {
        tp as f64 / predicted.len() as f64
    };
    let recall = tp as f64 / truth.len() as f64;
    println!(
        "link: {} data sets ({} conversion pairs, {} distractors) in {secs:.1} s: {} groups; pairwise precision {precision:.3} ({tp} true, {fp} false), recall {recall:.3} ({fnn} missed)",
        out.inputs.datasets,
        pairs.len(),
        distract.len(),
        out.groups.len()
    );
    for g in &out.groups {
        let kinds: BTreeSet<&str> = g
            .links
            .iter()
            .flat_map(|l| l.evidence.iter().map(|e| e.kind.as_str()))
            .collect();
        println!(
            "  group {} ({:?}): {} members, evidence {:?}",
            g.group,
            g.confidence,
            g.members.len(),
            kinds
        );
    }
    let mut evidence: BTreeMap<(String, String), String> = BTreeMap::new();
    for l in out
        .groups
        .iter()
        .flat_map(|g| g.links.iter())
        .chain(out.weak_links.iter())
    {
        let k = if l.a < l.b {
            (l.a.clone(), l.b.clone())
        } else {
            (l.b.clone(), l.a.clone())
        };
        let e: Vec<String> = l
            .evidence
            .iter()
            .map(|e| format!("{} [{:?}] {}", e.kind, e.confidence, e.detail))
            .collect();
        evidence.insert(k, e.join("; "));
    }
    let name = |p: &str| {
        ids.get(p)
            .cloned()
            .unwrap_or_else(|| p.rsplit('/').next().unwrap_or(p).to_string())
    };
    for k in truth.difference(&predicted) {
        println!(
            "  missed: {} + {} ({})",
            name(&k.0),
            name(&k.1),
            evidence.get(k).map_or("no evidence", String::as_str)
        );
    }
    for k in predicted.difference(&truth) {
        println!(
            "  false: {} + {} ({})",
            name(&k.0),
            name(&k.1),
            evidence.get(k).map_or("via the group", String::as_str)
        );
    }
    assert_eq!(fp, 0, "link must not assert false links");
    assert!(recall >= 0.5, "recall {recall}");
}
