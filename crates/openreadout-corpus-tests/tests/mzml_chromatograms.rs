//! mzML export of runs that hold chromatograms and no spectra: a ProteoWizard mzML of an
//! Agilent MRM run (TIC and SRM chromatograms) and a Waters Xevo TQ-XS MRM `.raw` (one table of
//! transitions). The export writes a chromatogramList and no spectrumList, verifies itself by
//! reading the file back, and the mzML reader must find the source's chromatograms in it with
//! the same values.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test mzml_chromatograms`
#![cfg(feature = "corpus")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::{Dataset, FormatReader};
use openreadout_mzml::MzmlReader;
use openreadout_mzml_writer::{MzmlExportOptions, export_mzml};
use openreadout_waters::WatersRawReader;

fn corpus_file(rel: &str) -> Option<PathBuf> {
    let dir = std::env::var_os("OPENREADOUT_CORPUS_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/files"),
        PathBuf::from,
    );
    let path = dir.join(rel);
    if path.exists() {
        Some(path)
    } else {
        eprintln!(
            "skip: {} missing (cargo xtask corpus fetch)",
            path.display()
        );
        None
    }
}

/// Export `ds` to mzML in `dir`; returns the file and its text.
fn export(ds: &mut dyn Dataset, input: &Path, dir: &Path) -> (PathBuf, String) {
    let out = dir.join("out.mzML");
    let report = export_mzml(ds, input, &out, &MzmlExportOptions::default()).unwrap();
    assert!(report.verified);
    assert_eq!(report.spectra_written, 0);
    assert!(report.chromatograms_written > 0);
    let text = std::fs::read_to_string(&out).unwrap();
    assert!(!text.contains("<spectrumList"), "no spectrumList");
    assert!(
        !text.contains("<index name=\"spectrum\">"),
        "no spectrum index"
    );
    assert!(text.contains("<indexList count=\"1\">"));
    (out, text)
}

/// Chromatograms of an mzML file by id: (time s, intensity, precursor, product).
type Chroms = BTreeMap<String, (Vec<f64>, Vec<f64>, Option<f64>, Option<f64>)>;

fn chromatograms(path: &Path) -> Chroms {
    let mut ds = MzmlReader.open(path).unwrap();
    let info = ds.info().unwrap();
    assert!(info.spectra.iter().all(|r| r.scan_count == 0));
    let mut out = BTreeMap::new();
    for t in &info.traces {
        let tr = ds.read_trace(t.index, 0, 0, t.sample_count).unwrap();
        let f = |k: &str| t.extra.get(k).and_then(serde_json::Value::as_f64);
        out.insert(
            t.name.clone().unwrap(),
            (
                tr.channels[0].clone(),
                tr.channels[1].clone(),
                f("precursor_mz"),
                f("product_mz"),
            ),
        );
    }
    out
}

#[test]
fn chromatogram_only_mzml_round_trips() {
    let Some(input) = corpus_file("Reader_Agilent_Test.data/MRM Neg C5.mzML") else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let mut ds = MzmlReader.open(&input).unwrap();
    let (out, text) = export(ds.as_mut(), &input, tmp.path());
    assert!(
        text.contains(
            "accession=\"MS:1001473\" name=\"selected reaction monitoring chromatogram\""
        )
    );
    let (a, b) = (chromatograms(&input), chromatograms(&out));
    assert_eq!(a.len(), 5, "TIC and four SRM chromatograms");
    assert_eq!(a, b);
}

#[test]
fn waters_mrm_tables_become_srm_chromatograms() {
    let Some(input) =
        corpus_file("mtbls3555-bv-ix-alpha-raw-zip/BV IX alpha QM - Lizzy SS-06-17-2021.raw")
    else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let mut ds = WatersRawReader.open(&input).unwrap();
    let info = ds.info().unwrap();
    assert!(info.spectra.is_empty());
    let t = &info.tables[0];
    let table = ds.read_table(t.index, 0, t.row_count).unwrap();
    let (_, text) = export(ds.as_mut(), &input, tmp.path());
    // the run id is a valid xs:ID although the file name has spaces
    assert!(text.contains("<run id=\"BV_IX_alpha_QM_-_Lizzy_SS-06-17-2021\""));
    let got = chromatograms(&tmp.path().join("out.mzML"));
    let time_s: Vec<f64> = table.columns[0].iter().map(|m| m * 60.0).collect();
    assert_eq!(got.len(), t.columns.len() - 1, "TIC and one per transition");
    let tic = &got["TIC FUNC001"];
    assert_eq!((&tic.0, &tic.1), (&time_s, &table.columns[1]));
    for (k, col) in t.columns.iter().enumerate().skip(2) {
        let c = &got[&format!("SRM FUNC001 {}", col.name)];
        assert_eq!((&c.0, &c.1), (&time_s, &table.columns[k]), "{}", col.name);
        let (q1, q3) = col.name.split_once(" > ").unwrap();
        assert_eq!(c.2, Some(q1.parse().unwrap()));
        assert_eq!(c.3, Some(q3.parse().unwrap()));
    }
}
