//! mzML export of runs that hold chromatograms and no spectra: a ProteoWizard mzML of an
//! Agilent MRM run (TIC and SRM chromatograms) and a Waters Xevo TQ-XS MRM `.raw` (one table of
//! transitions). The export writes a chromatogramList and no spectrumList, verifies itself by
//! reading the file back, and the mzML reader must find the source's chromatograms in it with
//! the same values. An SRM run with spectra (Thermo TSQ, Agilent MRM) also exports a TIC and
//! its SRM chromatograms, equal to the depositor's ProteoWizard export.
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

/// One chromatogram of an mzML file: id, times (s), values, precursor, product.
type Chrom = (String, Vec<f64>, Vec<f64>, Option<f64>, Option<f64>);

/// Every chromatogram of an mzML file, whether or not it also holds spectra.
fn all_chromatograms(path: &Path) -> Vec<Chrom> {
    let mut ds = MzmlReader.open(path).unwrap();
    let info = ds.info().unwrap();
    let mut out = Vec::new();
    for t in info
        .traces
        .iter()
        .filter(|t| t.extra.contains_key("chromatogram_type_accession") && t.channels.len() == 2)
    {
        let tr = ds.read_trace(t.index, 0, 0, t.sample_count).unwrap();
        let f = |k: &str| t.extra.get(k).and_then(serde_json::Value::as_f64);
        out.push((
            t.name.clone().unwrap_or_default(),
            tr.channels[0].clone(),
            tr.channels[1].clone(),
            f("precursor_mz"),
            f("product_mz"),
        ));
    }
    out
}

/// An SRM run exported to mzML carries a TIC and one SRM chromatogram per transition, rebuilt
/// from its spectra, as msconvert's export of the same file does: every chromatogram of the
/// depositor's (ProteoWizard) export has one of ours with the same transition, times and values.
#[test]
fn srm_runs_export_their_chromatograms() {
    let cases: [(&str, &str, Box<dyn FormatReader>); 2] = [
        (
            "mtbls1822-tsq-74.raw",
            "mtbls1822-tsq-74.mzML",
            Box::new(openreadout_thermo::ThermoRawReader),
        ),
        (
            "Reader_Agilent_Test.data/MRM Neg C5.d",
            "Reader_Agilent_Test.data/MRM Neg C5.mzML",
            Box::new(openreadout_agilent_ms::AgilentMsReader),
        ),
    ];
    for (raw, export_rel, reader) in cases {
        let (Some(input), Some(theirs)) = (corpus_file(raw), corpus_file(export_rel)) else {
            continue;
        };
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("out.mzML");
        let mut ds = reader.open(&input).unwrap();
        let report = export_mzml(ds.as_mut(), &input, &out, &MzmlExportOptions::default()).unwrap();
        assert!(report.verified && report.spectra_written > 0, "{raw}");
        let ours = all_chromatograms(&out);
        let want = all_chromatograms(&theirs);
        assert!(!want.is_empty(), "{export_rel}");
        let mut used = vec![false; ours.len()];
        for (id, t, v, q1, q3) in &want {
            let same = |o: &Chrom| {
                let transition = match (q1, q3) {
                    (Some(a), Some(b)) => {
                        o.3.is_some_and(|x| (x - a).abs() < 0.01)
                            && o.4.is_some_and(|x| (x - b).abs() < 0.01)
                    }
                    _ => o.0 == "TIC" && id == "TIC",
                };
                // ProteoWizard pads a transition with zeros at the times of another one whose
                // product lies within its window (two in mtbls1822): compare the non-zero points
                let nonzero = |times: &[f64], values: &[f64]| -> Vec<(f64, f64)> {
                    times
                        .iter()
                        .zip(values)
                        .filter(|p| *p.1 != 0.0)
                        .map(|(a, b)| (*a, *b))
                        .collect()
                };
                let (a, b) = (nonzero(&o.1, &o.2), nonzero(t, v));
                transition
                    && a.len() == b.len()
                    && a.iter()
                        .zip(&b)
                        // (the Agilent export stores its times as 32-bit minutes)
                        .all(|(x, y)| {
                            (x.0 - y.0).abs() <= 1e-6 + 2e-7 * y.0.abs()
                                && x.1.to_bits() == y.1.to_bits()
                        })
            };
            let k = (0..ours.len())
                .find(|&k| !used[k] && same(&ours[k]))
                .unwrap_or_else(|| panic!("{raw}: no exported chromatogram equals {id}"));
            used[k] = true;
        }
    }
}
