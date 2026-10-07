//! Our three Cq methods against the vendor's Cq, on every development qPCR file and on the
//! LightCycler 480 software's Cp tables (`corpus/oracle/qpcr-roche-export/`). Prints the
//! agreement per format and method that `docs/formats/qpcr.md` → "Analysis" records, and
//! checks the claims made there. Files that are not downloaded are skipped.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test qpcr_cq -- --nocapture`
#![cfg(feature = "corpus")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_qpcr::{
    CqMethod, QpcrAssayRecord, QpcrDataset, QpcrReport, QpcrReportRequest, qpcr_report,
};
use serde::Deserialize;

const FORMATS: [&str; 5] = [
    "rdml",
    "roche-lightcycler-ixo",
    "applied-biosystems-eds",
    "rotor-gene-rex",
    "qpcr-results-export",
];
const METHODS: [CqMethod; 3] = [
    CqMethod::Threshold,
    CqMethod::StoredThreshold,
    CqMethod::SecondDerivative,
];
/// The four LightCycler 480 runs the second-derivative windows and gate were chosen on.
const LC480_QC_RUNS: [&str; 4] = [
    "ixo-lc480-qc-2017a",
    "ixo-lc480-qc-2023b",
    "ixo-lc480-qc-2025b",
    "ixo-lc480-qc-2026a",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn corpus_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root().join("corpus/files"), PathBuf::from)
}

#[derive(Deserialize)]
struct Manifest {
    file: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    id: String,
    filename: String,
    format: String,
    #[serde(default)]
    tier: String,
    #[serde(default)]
    role: String,
}

/// Downloaded development inputs of the qPCR formats (no held-out file).
fn development_inputs() -> Vec<Entry> {
    let text = std::fs::read_to_string(root().join("corpus/manifest.toml")).unwrap();
    let manifest: Manifest = toml::from_str(&text).unwrap();
    manifest
        .file
        .into_iter()
        .filter(|e| {
            e.tier != "heldout"
                && (e.role.is_empty() || e.role == "input")
                && FORMATS.contains(&e.format.as_str())
                && corpus_dir().join(&e.filename).is_file()
        })
        .collect()
}

fn report(ds: &QpcrDataset, method: Option<CqMethod>) -> QpcrReport {
    let mut req = QpcrReportRequest::default();
    req.compute_cq = true;
    req.cq_method = method;
    qpcr_report(ds, &req).unwrap()
}

/// Agreement of our Cq with the vendor's over the results where the vendor gives a Cq or says
/// there is none.
#[derive(Default, Debug)]
struct Agreement {
    files: usize,
    results: usize,
    /// Results whose curve the method could not analyse (no computed Cq or call).
    unanalysed: usize,
    /// Both give a Cq.
    pairs: usize,
    /// One gives a Cq and the other none.
    calls_differ: usize,
    /// Pairs more than 0.05 cycles apart.
    beyond_0_05: usize,
    sum: f64,
    max: f64,
}

impl Agreement {
    /// `ours`: our record of the same well and target.
    fn add(&mut self, vendor: Option<f64>, ours: Option<&QpcrAssayRecord>) {
        self.results += 1;
        let Some(ours) = ours.filter(|r| r.computed_cq_status.is_some()) else {
            self.unanalysed += 1;
            return;
        };
        match (vendor, ours.computed_cq) {
            (Some(v), Some(o)) => {
                let d = (v - o).abs();
                assert!(d.is_finite());
                self.pairs += 1;
                self.beyond_0_05 += usize::from(d > 0.05);
                self.sum += d;
                self.max = self.max.max(d);
            }
            (None, None) => {}
            _ => self.calls_differ += 1,
        }
    }

    fn mean(&self) -> f64 {
        self.sum / self.pairs as f64
    }

    fn print(&self, label: &str) {
        let numbers = if self.pairs == 0 {
            "-".to_string()
        } else {
            format!("max {:.3} mean {:.3}", self.max, self.mean())
        };
        println!(
            "{label}: files {} results {} unanalysed {} pairs {} ({numbers}, {} beyond 0.05) calls differ {}",
            self.files,
            self.results,
            self.unanalysed,
            self.pairs,
            self.beyond_0_05,
            self.calls_differ
        );
    }
}

/// Every vendor result of the report against our Cq.
fn add_report(total: &mut Agreement, rep: &QpcrReport) {
    total.files += 1;
    for r in &rep.records {
        if r.cq.is_some() || r.cq_undetermined {
            total.add(r.cq, Some(r));
        }
    }
}

#[test]
fn cq_methods_against_stored_results() {
    let mut totals: BTreeMap<String, Agreement> = BTreeMap::new();
    let mut qc = Agreement::default();
    for entry in development_inputs() {
        let ds = QpcrDataset::open(&corpus_dir().join(&entry.filename))
            .unwrap_or_else(|e| panic!("{}: {e}", entry.id));
        for method in METHODS {
            let rep = report(&ds, Some(method));
            assert_eq!(rep.cq_comparison.as_ref().unwrap().cq_method, method);
            let method_name = serde_json::to_value(method).unwrap();
            let key = format!("{} {}", entry.format, method_name.as_str().unwrap());
            add_report(totals.entry(key).or_default(), &rep);
            if method == CqMethod::SecondDerivative && LC480_QC_RUNS.contains(&entry.id.as_str()) {
                add_report(&mut qc, &rep);
                // the default for LightCycler 480 files
                let default = report(&ds, None);
                assert_eq!(
                    default.cq_comparison.unwrap().cq_method,
                    CqMethod::SecondDerivative
                );
            }
            if method == CqMethod::StoredThreshold && entry.id == "eds-qs7pro-tb18s-stdcurve" {
                let mut one = Agreement::default();
                add_report(&mut one, &rep);
                one.print(&entry.id);
                assert_eq!((one.pairs, one.calls_differ, one.beyond_0_05), (90, 0, 0));
            }
        }
    }
    for (key, total) in &totals {
        total.print(key);
    }
    if qc.files > 0 {
        qc.print("LightCycler 480 QC runs, second-derivative");
        assert_eq!((qc.unanalysed, qc.calls_differ, qc.beyond_0_05), (0, 0, 0));
    }
}

#[derive(Deserialize)]
struct ExportOracle {
    id: String,
    wells: Vec<ExportWell>,
}

#[derive(Deserialize)]
struct ExportWell {
    pos: String,
    cp: Option<f64>,
}

/// The second-derivative default on six LightCycler 480 plates whose windows and gate were not
/// chosen on, against the Cp table the LightCycler 480 software exported for each.
#[test]
fn second_derivative_against_lightcycler_480_exports() {
    let dir = root().join("corpus/oracle/qpcr-roche-export");
    let mut total = Agreement::default();
    for path in openreadout_corpus_tests::oracle_json::files(&dir, ".json") {
        let text = openreadout_corpus_tests::oracle_json::read_to_string(&path).unwrap();
        let oracle: ExportOracle = serde_json::from_str(&text).unwrap();
        let file = corpus_dir().join(format!("{}.ixo", oracle.id));
        if !file.is_file() {
            eprintln!("skip {}: not downloaded", oracle.id);
            continue;
        }
        let rep = report(&QpcrDataset::open(&file).unwrap(), None);
        assert_eq!(
            rep.cq_comparison.as_ref().unwrap().cq_method,
            CqMethod::SecondDerivative
        );
        let ours: BTreeMap<&str, &QpcrAssayRecord> =
            rep.records.iter().map(|r| (r.well.as_str(), r)).collect();
        let mut plate = Agreement {
            files: 1,
            ..Agreement::default()
        };
        for w in &oracle.wells {
            plate.add(w.cp, ours.get(w.pos.as_str()).copied());
        }
        plate.print(&oracle.id);
        total.files += 1;
        total.results += plate.results;
        total.unanalysed += plate.unanalysed;
        total.pairs += plate.pairs;
        total.calls_differ += plate.calls_differ;
        total.beyond_0_05 += plate.beyond_0_05;
        total.sum += plate.sum;
        total.max = total.max.max(plate.max);
    }
    if total.files == 0 {
        return;
    }
    total.print("LightCycler 480 exports, second-derivative");
    // Measured 2026-10-07 on the six plates: 2154 pairs, mean 0.033, max 0.345, 11 calls
    // differ, and 24 wells with a Cp after cycle 38 unanalysed.
    assert!(total.mean() < 0.04, "{total:?}");
    assert!(total.max < 0.4, "{total:?}");
    assert!(total.calls_differ <= 11, "{total:?}");
    assert!(total.unanalysed <= 24, "{total:?}");
}
