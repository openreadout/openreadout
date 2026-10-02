//! Roche LightCycler files against ground truth read without our code (`oracle/qpcr_roche.py` →
//! `corpus/oracle/qpcr-roche/<id>.json`):
//!
//! - LightCycler 96 (`.lc96p`): the depositors' table of the LightCycler 96 software's results.
//!   Every exported well's Cq is ours for that sample and gene, no other reaction has a Cq (the
//!   RDML `cq` of wells the software calls Negative is not a Cq), and the replicate Cq mean and
//!   error are the software's (within the table's two decimals).
//! - LightCycler 480 (`.ixo`): a standard-library decode of the file's own statements (sample
//!   names, the analysis' calls and crossing points, readings per program and channel). No
//!   vendor export exists, so the decode is also checked against the vendor's calls: every
//!   position called positive rises in the analysed channel, every one called negative stays flat.
//!
//! Both are exported to RDML and read back. `analyze qpcr --cq` agreement with the vendor Cq is
//! printed (a method difference: the file does not say how the vendor computed its Cq).
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test qpcr_roche -- --nocapture`
#![cfg(feature = "corpus")]
// o/r/k/p/n/c: oracle, record, key, position, count, channel; keyed groups of tuples
#![allow(clippy::many_single_char_names, clippy::type_complexity)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::Dataset;
use openreadout_qpcr::{QpcrDataset, QpcrReportRequest, qpcr_report};
use serde_json::Value;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn corpus_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root().join("corpus/files"), PathBuf::from)
}

fn oracle(id: &str) -> Value {
    let p = root().join(format!("corpus/oracle/qpcr-roche/{id}.json"));
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap()
}

/// Export to RDML, read back, and compare the Cq records.
fn roundtrip(id: &str, ds: &QpcrDataset) {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join(format!("{id}.rdml"));
    let rep = openreadout_qpcr::export_rdml(ds, &out, true).unwrap();
    assert!(rep.verified, "{id}: RDML export not verified");
    let back = QpcrDataset::open(&out).unwrap();
    let cqs = |d: &QpcrDataset| {
        let mut v: Vec<(String, u32, u32, Option<String>, Option<u64>)> =
            qpcr_report(d, &QpcrReportRequest::default())
                .unwrap()
                .records
                .into_iter()
                .map(|x| (x.run, x.row, x.col, x.dye, x.cq.map(f64::to_bits)))
                .collect();
        v.sort();
        v
    };
    assert_eq!(
        cqs(ds),
        cqs(&back),
        "{id}: RDML round trip changed the Cq records"
    );
}

fn print_cq_agreement(id: &str, ds: &QpcrDataset) {
    let mut req = QpcrReportRequest::default();
    req.compute_cq = true;
    let rep = qpcr_report(ds, &req).unwrap();
    if let Some(c) = rep.cq_comparison {
        eprintln!(
            "{id}: qpcr --cq vs vendor Cq: {} both, {} both undetermined, {} only vendor, {} only ours, median |d| {:?}, mean d {:?}, max |d| {:?}",
            c.both_cq,
            c.both_undetermined,
            c.only_vendor,
            c.only_ours,
            c.median_abs_difference,
            c.mean_difference,
            c.max_abs_difference
        );
    }
}

fn gene_of(target: &str) -> String {
    target
        .rsplit_once('@')
        .map_or(target, |(_, g)| g)
        .trim()
        .to_lowercase()
}

#[test]
fn lightcycler96_results_match_the_vendor_table() {
    let mut total = 0usize;
    for id in [
        "lc96p-pendo-hmuy-hm-dip",
        "lc96p-pendo-hmuy-24h48h-r12",
        "lc96p-pendo-hmuy-24h48h-r34",
    ] {
        let path = corpus_dir().join(format!("{id}.lc96p"));
        if !path.exists() {
            eprintln!("skip {id}: not downloaded");
            continue;
        }
        let o = oracle(id);
        let ds = QpcrDataset::open(&path).unwrap();
        let rep = qpcr_report(&ds, &QpcrReportRequest::default()).unwrap();
        // ours: determined records by (sample, gene)
        let mut ours: BTreeMap<(String, String), Vec<(f64, Option<f64>, Option<f64>)>> =
            BTreeMap::new();
        for r in &rep.records {
            if let Some(cq) = r.cq {
                ours.entry((
                    r.sample.clone().unwrap_or_default(),
                    gene_of(r.target.as_deref().unwrap_or_default()),
                ))
                .or_default()
                .push((cq, r.cq_mean, r.cq_sd));
            } else if r.cq_stored.is_some() {
                assert!(
                    r.flags
                        .iter()
                        .any(|f| f.starts_with("lc96_") || f == "cq_at_cycle_count"),
                    "{id} {}: a stored cq without a LightCycler 96 call or cycle-count flag",
                    r.well
                );
            }
        }
        let mut want: BTreeMap<(String, String), Vec<(f64, f64, f64)>> = BTreeMap::new();
        for r in o["records"].as_array().unwrap() {
            let n = |k: &str| r[k].as_str().unwrap().trim().parse::<f64>().unwrap();
            want.entry((
                r["sample"].as_str().unwrap().trim().to_string(),
                r["gene"].as_str().unwrap().trim().to_lowercase(),
            ))
            .or_default()
            .push((n("cq"), n("cq_mean"), n("cq_error")));
        }
        assert_eq!(
            ours.keys().collect::<Vec<_>>(),
            want.keys().collect::<Vec<_>>(),
            "{id}: sample x gene groups with a Cq"
        );
        for (k, w) in &want {
            let mut a: Vec<f64> = ours[k].iter().map(|x| x.0).collect();
            let mut b: Vec<f64> = w.iter().map(|x| x.0).collect();
            a.sort_by(f64::total_cmp);
            b.sort_by(f64::total_cmp);
            assert_eq!(a.len(), b.len(), "{id} {k:?}: replicates");
            for (x, y) in a.iter().zip(&b) {
                assert!(
                    (x - y).abs() <= 0.005 + 1e-9,
                    "{id} {k:?}: Cq {x} vs table {y}"
                );
            }
            for (_, mean, sd) in &ours[k] {
                let (m, e) = (w[0].1, w[0].2);
                assert!(
                    mean.is_some_and(|v| (v - m).abs() <= 0.005 + 1e-9),
                    "{id} {k:?}: Cq mean {mean:?} vs table {m}"
                );
                assert!(
                    sd.is_some_and(|v| (v - e).abs() <= 0.005 + 1e-9),
                    "{id} {k:?}: Cq error {sd:?} vs table {e}"
                );
            }
            total += b.len();
        }
        roundtrip(id, &ds);
        print_cq_agreement(id, &ds);
    }
    eprintln!("{total} LightCycler 96 Cqs, means and errors equal the vendor table");
}

#[test]
fn lightcycler480_decode_matches_an_independent_reading() {
    let mut files = 0usize;
    for id in [
        "ixo-lc480-qc-2017a",
        "ixo-lc480-qc-2023b",
        "ixo-lc480-qc-2025b",
        "ixo-lc480-qc-2026a",
    ] {
        let path = corpus_dir().join(format!("{id}.ixo"));
        if !path.exists() {
            eprintln!("skip {id}: not downloaded");
            continue;
        }
        let o = oracle(id);
        let mut ds = QpcrDataset::open(&path).unwrap();
        let info = ds.info().unwrap();
        let ex = &info.tables[0].extra;
        assert_eq!(ex["experiment_name"], o["experiment"], "{id}");
        assert_eq!(ex["instrument"]["model"], o["instrument"], "{id}");
        assert_eq!(ex["acquired_at"], o["started_at"], "{id}");
        let rep = qpcr_report(&ds, &QpcrReportRequest::default()).unwrap();
        // the analysed channel: the one with vendor calls
        let called: Vec<_> = rep
            .records
            .iter()
            .filter(|r| r.run == "Run" && r.cq_status != "no result")
            .collect();
        let chan = called[0].dye.clone().unwrap();
        assert!(
            called
                .iter()
                .all(|r| r.dye.as_deref() == Some(chan.as_str()))
        );
        let positions = o["positions"].as_array().unwrap();
        assert_eq!(called.len(), positions.len(), "{id}: called positions");
        for p in positions {
            let pos = p["pos"].as_u64().unwrap() as u32;
            let (row, col) = (pos / 12 + 1, pos % 12 + 1);
            let r = called
                .iter()
                .find(|r| r.row == row && r.col == col)
                .unwrap_or_else(|| panic!("{id}: no record at position {pos}"));
            assert_eq!(r.sample.as_deref(), p["sample"].as_str(), "{id} {pos}");
            match p["call"].as_i64().unwrap() {
                2 => assert_eq!(r.cq, p["cp"].as_f64(), "{id} {pos}: Cp"),
                0 => assert!(r.cq.is_none() && r.cq_undetermined, "{id} {pos}"),
                c => panic!("{id} {pos}: call {c} (not in the corpus)"),
            }
            assert_eq!(r.excluded.is_none(), p["included"].as_bool().unwrap());
        }
        // readings: count and sum per program and channel (traces: run order Run, melt runs)
        let programs = o["programs"].as_array().unwrap();
        let channels: Vec<&str> = o["channels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        for (key, v) in o["readings"].as_object().unwrap() {
            let (p, c) = key.split_once('/').unwrap();
            let prog = &programs[p.parse::<usize>().unwrap()];
            let ch = channels[c.parse::<usize>().unwrap()];
            let (kind, run) = if prog["mode"] == "Quantification" {
                ("amplification", "Run".to_string())
            } else {
                ("melt", prog["name"].as_str().unwrap().to_string())
            };
            let name = format!("{run}: {kind} {ch}");
            let t = info
                .traces
                .iter()
                .find(|t| t.name.as_deref() == Some(name.as_str()))
                .unwrap_or_else(|| panic!("{id}: no trace {name}"));
            let n = v[0].as_u64().unwrap();
            assert_eq!(t.sample_count, n, "{id} {name}: readings");
            let tr = ds.read_trace(t.index, 0, 0, n).unwrap();
            let sum: f64 = tr.channels.iter().flatten().sum();
            let want = v[1].as_f64().unwrap();
            assert!(
                (sum - want).abs() <= 1e-9 * want.abs(),
                "{id} {name}: sum {sum} vs {want}"
            );
        }
        // the decode agrees with the vendor's calls: positive wells rise, negative ones stay flat
        let amp = info
            .traces
            .iter()
            .find(|t| t.name.as_deref() == Some(format!("Run: amplification {chan}").as_str()))
            .unwrap();
        let tr = ds.read_trace(amp.index, 0, 0, amp.sample_count).unwrap();
        for (k, ch) in amp.channels.iter().enumerate() {
            let well = ch.extra["well"].as_str().unwrap();
            let r = called.iter().find(|r| r.well == well).unwrap();
            let y = &tr.channels[k];
            let n = y.len();
            let rise = (y[n - 3..].iter().sum::<f64>() / 3.0) / (y[3..8].iter().sum::<f64>() / 5.0);
            if r.cq.is_some() {
                assert!(rise > 1.5, "{id} {well}: called positive, rises {rise:.2}x");
            } else {
                assert!(rise < 1.2, "{id} {well}: called negative, rises {rise:.2}x");
            }
        }
        roundtrip(id, &ds);
        print_cq_agreement(id, &ds);
        files += 1;
    }
    eprintln!("{files} LightCycler 480 files agree with the standard-library reading");
}
