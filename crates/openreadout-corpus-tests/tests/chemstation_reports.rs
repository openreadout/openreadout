//! ChemStation `.D` directories against the vendor's own reports and exports, read here
//! independently of the reader (whitespace- and comma-split, not the reader's column extents):
//!
//! - `Report.TXT` (LC/GC ChemStation): every peak of every signal is a row of `vendor_peaks`
//!   with the report's retention time, area, height and area %, and `check` finds each peak on
//!   our decoded signal (retention time on a local maximum, height within 10 %);
//! - `RESULTS.CSV` (MSD ChemStation TIC integration): the same for the TIC, apex scans included;
//! - `tic_front.csv` (a TIC export of the same MSD runs): every point equals our TIC.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test chemstation_reports -- --nocapture`
#![cfg(feature = "corpus")]
// report values are compared exactly as printed
#![allow(clippy::float_cmp)]

use std::path::{Path, PathBuf};

use openreadout_core::Dataset;

fn corpus_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/files"),
        PathBuf::from,
    )
}

fn utf16(b: &[u8]) -> String {
    let units: Vec<u16> = b[2..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    String::from_utf16_lossy(&units)
}

/// (signal, [rt, area, height, area %]) of every peak row of a `Report.TXT`.
fn report_rows(text: &str) -> Vec<(String, [f64; 4])> {
    let mut out = Vec::new();
    let mut signal = String::new();
    for l in text.lines() {
        let l = l.trim_end_matches('\r');
        if let Some(rest) = l.strip_prefix("Signal ") {
            signal = rest
                .split_once(':')
                .unwrap()
                .1
                .split(',')
                .next()
                .unwrap()
                .replace(' ', "");
            continue;
        }
        let w: Vec<&str> = l.split_whitespace().collect();
        // `1 2.824 BB S 0.0881 4.57137e4 7718.08594 97.91525` (the type may hold a space)
        if w.len() >= 7 && w[0].parse::<u32>().is_ok() && w[1].contains('.') {
            let n = w.len();
            let f = |s: &str| s.parse::<f64>().unwrap();
            out.push((
                signal.clone(),
                [f(w[1]), f(w[n - 3]), f(w[n - 2]), f(w[n - 1])],
            ));
        }
    }
    out
}

fn table_rows(ds: &mut dyn Dataset) -> (Vec<String>, Vec<Vec<f64>>) {
    let info = ds.info().unwrap();
    let t = info
        .tables
        .iter()
        .find(|t| t.name.as_deref() == Some("vendor_peaks"))
        .expect("a vendor_peaks table");
    let cats: Vec<String> = t.columns[0].extra["categories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    let tab = ds.read_table(t.index, 0, t.row_count).unwrap();
    let rows = (0..tab.columns[0].len())
        .map(|i| tab.columns.iter().map(|c| c[i]).collect())
        .collect();
    (cats, rows)
}

fn no_mismatch(ds: &mut dyn Dataset, what: &str) {
    let rep = ds.check().unwrap();
    let bad: Vec<&str> = rep
        .findings
        .iter()
        .filter(|f| f.code.starts_with("vendor_"))
        .map(|f| f.message.as_str())
        .collect();
    assert!(bad.is_empty(), "{what}: {bad:?}");
}

#[test]
fn report_txt_peaks_are_read_and_found_on_our_signals() {
    let mut peaks = 0;
    for run in ["0101", "0102", "0103", "0104"] {
        let dir = corpus_dir().join(format!("chromhandler-001F{run}.D"));
        if !dir.exists() {
            eprintln!("skip {}", dir.display());
            continue;
        }
        let want = report_rows(&utf16(&std::fs::read(dir.join("Report.TXT")).unwrap()));
        let mut ds =
            openreadout_chrom::chemstation_dataset::ChemStationDataset::open(&dir).unwrap();
        let (cats, rows) = table_rows(&mut ds);
        assert_eq!(rows.len(), want.len(), "{run}: peaks");
        for ((sig, w), r) in want.iter().zip(&rows) {
            assert_eq!(&cats[r[0] as usize], sig, "{run}: signal");
            // rt, area, height, area % as printed
            for (k, c) in [(0, 2), (1, 5), (2, 6), (3, 7)] {
                assert!(
                    (r[c] - w[k]).abs() <= 1e-9 * w[k].abs().max(1.0),
                    "{run} {sig}: column {c}: {} vs {}",
                    r[c],
                    w[k]
                );
            }
        }
        no_mismatch(&mut ds, run);
        peaks += rows.len();
    }
    eprintln!("{peaks} Report.TXT peaks read and found on our signals");
}

#[test]
fn msd_results_and_tic_exports_match() {
    let (mut peaks, mut points, mut runs) = (0, 0, 0);
    for i in 1..=12 {
        let dir = corpus_dir().join(format!("chromhandler-RAU-R505-{i:02}.D"));
        if !dir.join("data.ms").exists() {
            eprintln!("skip {}", dir.display());
            continue;
        }
        let mut ds =
            openreadout_chrom::chemstation_dataset::ChemStationDataset::open(&dir).unwrap();
        // RESULTS.CSV rows, comma-split
        let text = std::fs::read_to_string(dir.join("RESULTS.CSV")).unwrap();
        let want: Vec<Vec<f64>> = text
            .lines()
            .filter_map(|l| l.split_once("=,"))
            .filter(|(k, _)| k.trim().parse::<u32>().is_ok())
            .filter(|(_, v)| v.contains('"'))
            .map(|(_, v)| {
                v.split(',')
                    .map(|s| {
                        s.trim()
                            .trim_matches('"')
                            .trim()
                            .parse::<f64>()
                            .unwrap_or(f64::NAN)
                    })
                    .collect()
            })
            .collect();
        let (cats, rows) = table_rows(&mut ds);
        assert_eq!(cats, vec!["TIC data.ms".to_string()], "{i}");
        assert_eq!(rows.len(), want.len(), "{i}: peaks");
        for (w, r) in want.iter().zip(&rows) {
            // Peak, R.T., First, Max, Last, PK TY, Height, Area, Pct Max, Pct Total
            for (k, c) in [
                (0, 1),
                (1, 2),
                (2, 10),
                (3, 11),
                (4, 12),
                (6, 6),
                (7, 5),
                (9, 7),
            ] {
                assert_eq!(r[c], w[k], "{i}: column {c}");
            }
        }
        no_mismatch(&mut ds, &format!("RAU-R505-{i:02}"));
        peaks += rows.len();
        // tic_front.csv against our TIC
        let tic = dir.join("tic_front.csv");
        if tic.exists() {
            // (minutes as printed, counts): the export prints six significant digits
            let export: Vec<(String, f64)> = std::fs::read_to_string(&tic)
                .unwrap()
                .lines()
                .skip_while(|l| !l.starts_with("Start of data points"))
                .skip(1)
                .filter_map(|l| l.trim().split_once(','))
                .map(|(a, b)| (a.to_string(), b.parse().unwrap()))
                .collect();
            let mut ours = Vec::new();
            ds.visit_scan_headers(0, 0, &mut |h| {
                ours.push((h.rt_s.unwrap() / 60.0, h.total_ion_current.unwrap()));
                true
            })
            .unwrap();
            assert_eq!(ours.len(), export.len(), "{i}: TIC points");
            for ((t, v), (es, ev)) in ours.iter().zip(&export) {
                let et: f64 = es.parse().unwrap();
                let decimals = es.split_once('.').map_or(0, |(_, d)| d.len());
                let half = 0.5 * 10f64.powi(-i32::try_from(decimals).unwrap());
                assert!((t - et).abs() <= half + 1e-9, "{i}: time {t} vs {es}");
                assert_eq!(v, ev, "{i}: TIC at {et}");
            }
            points += export.len();
        }
        runs += 1;
    }
    eprintln!(
        "{runs} MSD runs: {peaks} RESULTS.CSV peaks read and found on our TIC, {points} TIC points equal to the export"
    );
}

/// `Result.xml` (ChemStation's XML export): every `IntegrationResults` element is a row, the
/// compound results are joined to it, and `check` finds the peaks on our signals: areas between
/// the vendor's own limits and above its baseline (not only heights).
#[test]
fn result_xml_peaks_are_read_and_found_on_our_signals() {
    let mut rows_total = 0;
    for run in ["gc2asm-V181.D", "gc2asm-three-channels.D"] {
        let dir = corpus_dir().join(run);
        if !dir.join("Result.xml").exists() {
            eprintln!("skip {run}");
            continue;
        }
        let text = utf16(&std::fs::read(dir.join("Result.xml")).unwrap());
        let want = text.matches("<IntegrationResults>").count();
        let mut ds =
            openreadout_chrom::chemstation_dataset::ChemStationDataset::open(&dir).unwrap();
        let info = ds.info().unwrap();
        let t = info
            .tables
            .iter()
            .find(|t| t.name.as_deref() == Some("vendor_peaks"))
            .unwrap();
        assert_eq!(t.row_count as usize, want, "{run}: rows");
        let names: Vec<String> = t.columns[9].extra["categories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let tab = ds.read_table(t.index, 0, t.row_count).unwrap();
        if run == "gc2asm-three-channels.D" {
            // a compound result as the XML states it
            let k = names.iter().position(|n| n == "Methane").unwrap();
            let i = (0..tab.columns[9].len())
                .find(|&i| tab.columns[9][i] == k as f64)
                .unwrap();
            assert_eq!(tab.columns[8][i], 0.904_667_315_8);
            assert_eq!(t.columns[8].unit.as_deref(), Some("% v/v"));
        }
        let rep = ds.check().unwrap();
        assert!(
            rep.findings
                .iter()
                .all(|f| f.code != "vendor_peak_mismatch"),
            "{run}: {:?}",
            rep.findings
        );
        rows_total += want;
    }
    eprintln!("{rows_total} Result.xml peaks read; areas and heights found on our signals");
}
