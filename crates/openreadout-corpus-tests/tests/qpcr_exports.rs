//! StepOne/7500-layout `.eds` runs against the vendor's Results export of the same run
//! (`oracle/qpcr_exports.py` → `corpus/oracle/qpcr-exports/<id>.json`, read with xlrd): per
//! well × target the Ct (or "Undetermined"), Ct Mean and SD, the threshold, the baseline window
//! the vendor used (automatic baselines are recovered from Rn − ΔRn, or left empty — never the
//! setting), and the Tm list. Our own Cq (`analyze qpcr --cq`) is compared with the vendor's Ct.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test qpcr_exports -- --nocapture`
#![cfg(feature = "corpus")]
#![allow(clippy::float_cmp)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_qpcr::{QpcrDataset, QpcrReportRequest, qpcr_report};
use serde_json::Value;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn corpus_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root().join("corpus/files"), PathBuf::from)
}

const RUNS: [&str; 8] = [
    "eds-stepone-taec-mu-11-3-6h",
    "eds-stepone-taec-4-7-24h",
    "eds-stepone-taec-72h-13-6",
    "eds-stepone-taec-mu-11-3-6h-b",
    "eds-stepone-taec-mu-12-3-24h",
    "eds-stepone-taec-24h-0819",
    "eds-stepone-caco2-dss-eps-il6",
    "eds-stepone-caco2-dss-eps-ccl2",
];

fn f(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

#[test]
fn stepone_runs_match_their_results_exports() {
    let (mut wells, mut undetermined, mut windows, mut refused, mut tms) = (0, 0, 0, 0, 0);
    let mut diffs: Vec<f64> = Vec::new();
    for id in RUNS {
        let path = corpus_dir().join(format!("{id}.eds"));
        if !path.exists() {
            eprintln!("skip {id}: not downloaded");
            continue;
        }
        let o: Value = serde_json::from_str(
            &std::fs::read_to_string(root().join(format!("corpus/oracle/qpcr-exports/{id}.json")))
                .unwrap(),
        )
        .unwrap();
        let ds = QpcrDataset::open(&path).unwrap();
        let mut req = QpcrReportRequest::default();
        req.compute_cq = true;
        let rep = qpcr_report(&ds, &req).unwrap();
        let ours: BTreeMap<(String, String), &openreadout_qpcr::QpcrAssayRecord> = rep
            .records
            .iter()
            .map(|r| ((r.well.clone(), r.target.clone().unwrap_or_default()), r))
            .collect();
        for w in o["records"].as_array().unwrap() {
            let key = (
                w["well"].as_str().unwrap().to_string(),
                w["target"].as_str().unwrap().to_string(),
            );
            let at = format!("{id} {} {}", key.0, key.1);
            let r = ours.get(&key).unwrap_or_else(|| panic!("{at}: no record"));
            // (eds-stepone-taec-24h-0819 was saved without sample names; its export has them)
            if r.sample.is_some() {
                assert_eq!(r.sample.as_deref(), w["sample"].as_str(), "{at}: sample");
            }
            match &w["ct"] {
                Value::String(s) if s == "Undetermined" => {
                    assert_eq!(r.cq_status, "undetermined", "{at}");
                    assert!(r.cq.is_none(), "{at}");
                    undetermined += 1;
                }
                v => {
                    let ct = v.as_f64().unwrap_or_else(|| panic!("{at}: Ct {v}"));
                    // the export writes float32; analysis_result.txt six decimals
                    let cq = r.cq.unwrap_or_else(|| panic!("{at}: no Cq (export {ct})"));
                    assert!((cq - ct).abs() <= 1e-5, "{at}: Cq {cq} vs {ct}");
                    if let Some(c) = r.computed_cq {
                        diffs.push(c - ct);
                    }
                }
            }
            for (name, ours, want) in [
                ("Ct Mean", r.cq_mean, f(&w["ct_mean"])),
                ("Ct SD", r.cq_sd, f(&w["ct_sd"])),
                ("threshold", r.threshold, f(&w["threshold"])),
            ] {
                if let Some(want) = want {
                    let got = ours.unwrap_or_else(|| panic!("{at}: no {name} (export {want})"));
                    assert!(
                        (got - want).abs() <= 1e-5 * want.abs().max(1e-3),
                        "{at}: {name} {got} vs {want}"
                    );
                }
            }
            // the baseline window the vendor used, or nothing
            let want = (f(&w["baseline_start"]), f(&w["baseline_end"]));
            match (r.baseline_start, r.baseline_end) {
                (Some(s), Some(e)) => {
                    assert_eq!(
                        (Some(f64::from(s)), Some(f64::from(e))),
                        want,
                        "{at}: baseline window"
                    );
                    windows += 1;
                }
                (None, None) => refused += 1,
                other => panic!("{at}: half a baseline window {other:?}"),
            }
            let tm: Vec<f64> = w["tm"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_f64)
                .collect();
            assert_eq!(r.tm.len(), tm.len(), "{at}: Tm count");
            for (a, b) in r.tm.iter().zip(&tm) {
                assert!((a - b).abs() <= 1e-4, "{at}: Tm {a} vs {b}");
            }
            tms += tm.len();
            wells += 1;
        }
    }
    let mut abs: Vec<f64> = diffs.iter().map(|d| d.abs()).collect();
    abs.sort_by(f64::total_cmp);
    let median = abs.get(abs.len() / 2).copied().unwrap_or(0.0);
    eprintln!(
        "{wells} wells equal to the vendor exports ({undetermined} Undetermined, {tms} Tm values); baseline window recovered for {windows}, left empty for {refused}; qpcr --cq vs vendor Ct over {} wells: median |d| {median:.4}, 95th percentile {:.4}",
        abs.len(),
        abs.get(abs.len() * 95 / 100).copied().unwrap_or(0.0)
    );
    assert!(
        windows >= 9 * (windows + refused) / 10,
        "most automatic windows are recovered"
    );
    assert!(
        median <= 0.1,
        "our Cq with the vendor's baseline line and threshold"
    );
}

/// Genotyping calls (`eds-qs7-snv-genotyping`, no vendor export): the table `genotypes` against
/// 1000 Genomes genotypes of four reference samples (Ensembl REST, `docs/provenance/qpcr.md`
/// 2026-09-26) and the NTC wells.
#[test]
fn genotyping_calls_match_reference_genotypes() {
    use openreadout_core::Dataset;
    let path = corpus_dir().join("eds-qs7-snv-genotyping.eds");
    if !path.exists() {
        eprintln!("skip eds-qs7-snv-genotyping: not downloaded");
        return;
    }
    let mut ds = QpcrDataset::open(&path).unwrap();
    let info = ds.info().unwrap();
    let t = info
        .tables
        .iter()
        .find(|t| t.name.as_deref() == Some("genotypes"))
        .expect("a genotypes table");
    let cats = |c: usize| -> Vec<String> {
        t.columns[c].extra["categories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    };
    let (samples, markers, tasks, genotypes) = (cats(3), cats(4), cats(5), cats(7));
    let tab = ds.read_table(t.index, 0, t.row_count).unwrap();
    let mut seen: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let mut ntc = 0;
    for i in 0..tab.columns[0].len() {
        let task = &tasks[tab.columns[5][i] as usize];
        if task == "ntc" {
            assert_eq!(tab.columns[8][i], 0.0, "NTC row {i}");
            ntc += 1;
            continue;
        }
        let s = &samples[tab.columns[3][i] as usize];
        let m = &markers[tab.columns[4][i] as usize];
        let g = tab.columns[7][i];
        if g.is_finite() {
            seen.entry((s.clone(), m.clone()))
                .or_default()
                .push(genotypes[g as usize].clone());
        }
    }
    // 1000 Genomes: rs1061235 (A/T, the assay's strand) and rs10484555 (reference T/C; the
    // assay's G/A is the other strand: G = C, A = T)
    for (sample, marker, want) in [
        ("HG00096", "rs1061235", "A/A"),
        ("HG00463", "rs1061235", "A/A"),
        ("HG01190", "rs1061235", "A/A"),
        ("HG01791", "rs1061235", "A/A"),
        ("HG00096", "rs10484555", "A/A"),
        ("HG01190", "rs10484555", "A/A"),
        ("HG01791", "rs10484555", "A/A"),
    ] {
        let got = &seen[&(sample.to_string(), marker.to_string())];
        assert!(
            got.iter().all(|g| g == want),
            "{sample} {marker}: {got:?} vs {want}"
        );
    }
    // HG00463 is C|T at rs10484555: one replicate is called heterozygous (G/A)
    assert!(seen[&("HG00463".to_string(), "rs10484555".to_string())].contains(&"G/A".to_string()));
    assert_eq!(ntc, 18);
    eprintln!(
        "{} genotype calls; 7 reference genotypes, one heterozygote and 18 NTC wells as expected",
        tab.columns[0].len()
    );
}
