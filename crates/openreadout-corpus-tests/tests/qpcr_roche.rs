//! Roche LightCycler files against ground truth read without our code (`oracle/qpcr_roche.py` →
//! `corpus/oracle/qpcr-roche/<id>.json`):
//!
//! - LightCycler 96 (`.lc96p`): the depositors' table of the LightCycler 96 software's results.
//!   Every exported well's Cq is ours for that sample and gene, no other reaction has a Cq (the
//!   RDML `cq` of wells the software calls Negative is not a Cq), and the replicate Cq mean and
//!   error are the software's (within the table's two decimals).
//! - LightCycler 480 (`.ixo`): a standard-library decode of the file's own statements (sample
//!   names, the analysis' calls and crossing points, readings per program and channel), also
//!   checked against the vendor's calls: every position called positive rises in the analysed
//!   channel, every one called negative stays flat. Two independent oracles besides: the QC
//!   runs' depositor's own reader (`corpus/oracle/qpcr-roche-qcreader/`) and the LightCycler 480
//!   software's Cp table and raw-data export of six plates (`corpus/oracle/qpcr-roche-export/`).
//!   With `QPCR_ROCHE_RESULTS=<file>` those two comparisons append a results line per file for
//!   `cargo xtask assurance-audit refresh --results <file>`.
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

/// A results line for `cargo xtask assurance-audit refresh` (`QPCR_ROCHE_RESULTS=<file>`):
/// `compared` names the outputs the comparison checked, `fields` the tracked normalized fields.
fn results_line(id: &str, problems: &[String], compared: &[&str], fields: &[&str]) -> String {
    let status = if problems.is_empty() { "pass" } else { "FAIL" };
    let mut s = serde_json::json!({
        "id": id,
        "format": "roche-lightcycler-ixo",
        "status": status,
        "independent": true,
        "compared": compared,
        "fields": fields,
    })
    .to_string();
    s.push('\n');
    s
}

/// Append results lines to the file `QPCR_ROCHE_RESULTS` names (both LightCycler 480 tests
/// write to it, so it is appended to, not replaced).
fn write_results(lines: &str) {
    if let Ok(p) = std::env::var("QPCR_ROCHE_RESULTS") {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .unwrap();
        f.write_all(lines.as_bytes()).unwrap();
    }
}

/// The normalized experiment as `info --json` reports it under `/experiment`.
fn experiment_json(ds: &QpcrDataset, info: &openreadout_core::FileInfo) -> Value {
    serde_json::to_value(openreadout_core::experiment::of_dataset(ds, info)).unwrap()
}

fn oracle_in(dir: &str, id: &str) -> Value {
    let p = root().join(format!("corpus/oracle/{dir}/{id}.json"));
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap()
}

/// The four QC runs against the depositor's own offline reader (`oracle/lc480_qc_reader.mjs`,
/// `corpus/oracle/qpcr-roche-qcreader/`): instrument model, software version and run date, every
/// stored crossing point, and the amplification readings of cycles 2-45 per channel (count, sum
/// and first well). Its melt data are the software's resampled curves, which we do not return, so
/// melt is not compared.
#[test]
fn lightcycler480_matches_the_depositors_reader() {
    let mut results = String::new();
    let mut failed = Vec::new();
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
        let o = oracle_in("qpcr-roche-qcreader", id);
        let mut ds = QpcrDataset::open(&path).unwrap();
        let info = ds.info().unwrap();
        let e = experiment_json(&ds, &info);
        let mut problems = Vec::new();
        if e["instrument"]["model"] != o["instrument"] {
            problems.push(format!(
                "model {} vs {}",
                e["instrument"]["model"], o["instrument"]
            ));
        }
        if e["instrument"]["software_version"] != o["software"] {
            problems.push(format!(
                "software {} vs {}",
                e["instrument"]["software_version"], o["software"]
            ));
        }
        // the reader gives the run's day only
        let day = e["acquisition"]["started_at"].as_str().unwrap_or_default();
        if !day.starts_with(o["date"].as_str().unwrap()) {
            problems.push(format!("run date {day} vs {}", o["date"]));
        }
        // crossing points: every stored one is our Cq, and we have no other
        let rep = qpcr_report(&ds, &QpcrReportRequest::default()).unwrap();
        let ours: BTreeMap<u32, f64> = rep
            .records
            .iter()
            .filter(|r| r.run == "Run")
            .filter_map(|r| Some(((r.row - 1) * 12 + r.col - 1, r.cq?)))
            .collect();
        let want: BTreeMap<u32, f64> = o["crossing_points"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.parse().unwrap(), v.as_f64().unwrap()))
            .collect();
        if ours.keys().ne(want.keys()) {
            problems.push(format!(
                "{} positions with a Cq, the reader has {}",
                ours.len(),
                want.len()
            ));
        }
        for (pos, cp) in &want {
            if ours
                .get(pos)
                .is_none_or(|c| (c - cp).abs() > 1e-9 * cp.abs())
            {
                problems.push(format!("position {pos}: Cq {:?} vs {cp}", ours.get(pos)));
            }
        }
        // amplification readings per channel and cycle (the reader's cycle 1 holds melt
        // readings, so it starts at cycle 2)
        let channels = ds.vendor_metadata().unwrap()["channels"].clone();
        let mut cycles = 0usize;
        for (ch, per_cycle) in o["amplification"].as_object().unwrap() {
            let name = channels[ch.parse::<usize>().unwrap()]["name"]
                .as_str()
                .unwrap()
                .to_string();
            let Some(t) = info
                .traces
                .iter()
                .find(|t| t.name.as_deref() == Some(format!("Run: amplification {name}").as_str()))
            else {
                problems.push(format!("no amplification trace for channel {name}"));
                continue;
            };
            let tr = ds.read_trace(t.index, 0, 0, t.sample_count).unwrap();
            for c in per_cycle.as_array().unwrap() {
                let k = c["cycle"].as_u64().unwrap() as usize - 1;
                let vals: Vec<f64> = tr
                    .channels
                    .iter()
                    .filter_map(|w| w.get(k).copied())
                    .collect();
                let sum: f64 = vals.iter().sum();
                let (n, s, w0) = (
                    c["n"].as_u64().unwrap() as usize,
                    c["sum"].as_f64().unwrap(),
                    c["well0"].as_f64().unwrap(),
                );
                if vals.len() != n || (sum - s).abs() > 1e-9 * s.abs() || vals.first() != Some(&w0)
                {
                    problems.push(format!(
                        "{name} cycle {}: {} readings summing to {sum} (first {:?}) vs {n}, {s}, {w0}",
                        k + 1,
                        vals.len(),
                        vals.first()
                    ));
                }
                cycles += 1;
            }
        }
        eprintln!(
            "{id}: {} crossing points and {cycles} channel-cycles compared with the depositor's reader; {} problems",
            want.len(),
            problems.len()
        );
        results.push_str(&results_line(
            id,
            &problems,
            &["metadata", "tables", "traces"],
            &["experiment.instrument.model"],
        ));
        if !problems.is_empty() {
            failed.push(format!(
                "{id}: {}",
                problems[..problems.len().min(5)].join("; ")
            ));
        }
    }
    write_results(&results);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// The six Mendeley plates against the LightCycler 480 software's own exports
/// (`oracle/lc480_export_oracle.py`, `corpus/oracle/qpcr-roche-export/`): every well's sample
/// name; the Cp where the file stores the analysis (`cp1` only; the other files store none and
/// we return none); and every exported reading, which is the stored reading times the
/// acquisition's export scale (`vendor.export_scale`), within the export's rounding.
#[test]
fn lightcycler480_matches_the_vendor_exports() {
    let mut results = String::new();
    let mut failed = Vec::new();
    for id in [
        "ixo-mendeley-diras2-cp1",
        "ixo-mendeley-diras2-cp2",
        "ixo-mendeley-diras2-cp3",
        "ixo-mendeley-diras2-cp4",
        "ixo-mendeley-diras2-cp6",
        "ixo-mendeley-diras2-cp7",
    ] {
        let path = corpus_dir().join(format!("{id}.ixo"));
        if !path.exists() {
            eprintln!("skip {id}: not downloaded");
            continue;
        }
        let o = oracle_in("qpcr-roche-export", id);
        let mut ds = QpcrDataset::open(&path).unwrap();
        let info = ds.info().unwrap();
        let vendor = ds.vendor_metadata().unwrap();
        let e = experiment_json(&ds, &info);
        let mut problems = Vec::new();
        if e["instrument"]["software_version"] != o["software"] {
            problems.push(format!(
                "software {} vs {}",
                e["instrument"]["software_version"], o["software"]
            ));
        }
        let rep = qpcr_report(&ds, &QpcrReportRequest::default()).unwrap();
        let tol_cp = o["tol_cp"].as_f64().unwrap() + 1e-9;
        let tol_raw = o["tol_raw"].as_f64().unwrap() + 1e-9;
        let analysed = rep.records.iter().any(|r| r.cq_status != "no result");
        let (mut cps, mut readings) = (0usize, 0usize);
        for export_ch in o["channels"].as_array().unwrap() {
            // the export names a channel by its wavelengths (`465-510`); the file may name it
            // otherwise (`FAM`)
            let export_ch = export_ch.as_str().unwrap();
            let Some(ch) = vendor["channels"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|c| {
                    let nm = |k: &str| c[k].as_f64().unwrap_or(f64::NAN);
                    c["name"] == export_ch
                        || format!("{}-{}", nm("excitation_nm"), nm("emission_nm")) == export_ch
                })
                .and_then(|c| c["name"].as_str())
            else {
                problems.push(format!("no channel {export_ch}"));
                continue;
            };
            // trace names carry the run's name only when the file has several runs
            let wanted = format!("amplification {ch}");
            let Some(t) = info.traces.iter().find(|t| {
                t.name
                    .as_deref()
                    .is_some_and(|n| n == wanted || n.ends_with(&format!(": {wanted}")))
            }) else {
                problems.push(format!(
                    "no amplification trace for channel {ch} (traces: {:?})",
                    info.traces
                        .iter()
                        .map(|t| t.name.clone())
                        .collect::<Vec<_>>()
                ));
                continue;
            };
            let run = match t.name.as_deref().unwrap().split_once(": ") {
                Some((run, _)) => run,
                None => rep.records.first().map_or("", |r| r.run.as_str()),
            };
            let tr = ds.read_trace(t.index, 0, 0, t.sample_count).unwrap();
            let scale: Vec<Option<f64>> = vendor["export_scale"][ch]
                .as_array()
                .map(|a| a.iter().map(Value::as_f64).collect())
                .unwrap_or_default();
            for w in o["wells"].as_array().unwrap() {
                let pos = w["pos"].as_str().unwrap();
                let Some(r) = rep
                    .records
                    .iter()
                    .find(|r| r.run == run && r.well == pos && r.dye.as_deref() == Some(ch))
                else {
                    problems.push(format!("{pos}: no record"));
                    continue;
                };
                if r.sample.as_deref() != w["name"].as_str() {
                    problems.push(format!("{pos}: sample {:?} vs {}", r.sample, w["name"]));
                }
                if analysed {
                    let ok = match (r.cq, w["cp"].as_f64()) {
                        (Some(a), Some(b)) => (a - b).abs() <= tol_cp,
                        (None, None) => true,
                        _ => false,
                    };
                    if !ok {
                        problems.push(format!("{pos}: Cq {:?} vs Cp {}", r.cq, w["cp"]));
                    }
                    cps += 1;
                } else if r.cq.is_some() {
                    problems.push(format!("{pos}: a Cq in a file without an analysis"));
                }
                let Some(k) = t
                    .channels
                    .iter()
                    .position(|c| c.extra.get("well").and_then(Value::as_str) == Some(pos))
                else {
                    problems.push(format!("{pos}: no amplification curve"));
                    continue;
                };
                let want = w["raw"][export_ch].as_array().unwrap();
                if want.len() != tr.channels[k].len() || want.len() != scale.len() {
                    problems.push(format!(
                        "{pos}: {} cycles, {} scales, the export has {}",
                        tr.channels[k].len(),
                        scale.len(),
                        want.len()
                    ));
                    continue;
                }
                for (i, v) in want.iter().enumerate() {
                    let v = v.as_f64().unwrap();
                    let ours = tr.channels[k][i] * scale[i].unwrap_or(f64::NAN);
                    if (ours - v).abs() > tol_raw || ours.is_nan() {
                        problems.push(format!("{pos} cycle {}: {ours} vs export {v}", i + 1));
                    }
                    readings += 1;
                }
            }
        }
        eprintln!(
            "{id}: {} wells, {cps} Cps and {readings} exported readings compared; {} problems",
            o["wells"].as_array().unwrap().len(),
            problems.len()
        );
        results.push_str(&results_line(
            id,
            &problems,
            &["metadata", "tables", "traces"],
            &[],
        ));
        if !problems.is_empty() {
            failed.push(format!(
                "{id}: {}",
                problems[..problems.len().min(5)].join("; ")
            ));
        }
    }
    write_results(&results);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}
