//! Synthetic Axon Text Files (ATF 1.0) written from the layout in docs/formats/abf.md.
#![allow(clippy::float_cmp)]

use openreadout_abf::{ATF_FORMAT_ID, AtfReader};
use openreadout_core::model::Severity;
use openreadout_core::reader::FormatReader;
use openreadout_core::{Dataset, Error};

/// Two signals (`IN 1` first) × 2 sweeps, 4 rows at 1 kHz, time in ms.
const ATF: &str = "ATF\t1.0\n3\t5\n\"AcquisitionMode=Episodic Stimulation\"\n\"Comment=synthetic\"\n\"Signals=\"\t\"IN 1\"\t\"IN 0\"\t\"IN 1\"\t\"IN 0\"\n\"Time (ms)\"\t\"Trace #1 (mV)\"\t\"Trace #1 (pA)\"\t\"Trace #2 (mV)\"\t\"Trace #2 (pA)\"\n0\t1.5\t-10\t2.5\t-20\n1\t1.6\t-11\t2.6\t-21\n2\t1.7\t-12\t2.7\t-22\n3\t1.8\t-13\t2.8\t-23\n";

fn open(text: &str) -> (tempfile::TempDir, Result<Box<dyn Dataset>, Error>) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("x.atf");
    std::fs::write(&p, text).unwrap();
    assert_eq!(
        AtfReader.sniff(text.as_bytes(), &p).map(|d| d.format_id),
        Some(ATF_FORMAT_ID)
    );
    let ds = AtfReader.open(&p);
    (dir, ds)
}

#[test]
fn signals_become_channels_and_repeats_become_sweeps() {
    let (_d, ds) = open(ATF);
    let mut ds = ds.unwrap();
    let info = ds.info().unwrap();
    let t = &info.traces[0];
    assert_eq!((t.sweep_count, t.sample_count), (2, 4));
    assert_eq!(t.channels[0].name, "IN 1");
    assert_eq!(t.channels[0].unit.as_deref(), Some("mV"));
    assert_eq!(t.channels[1].unit.as_deref(), Some("pA"));
    assert_eq!(t.sample_rate_hz, 1000.0);
    assert_eq!(t.extra["comment"], "synthetic");
    let s1 = ds.read_trace(0, 1, 1, 10).unwrap();
    assert_eq!(
        s1.channels,
        [vec![2.6, 2.7, 2.8], vec![-21.0, -22.0, -23.0]]
    );
    assert!(ds.check().unwrap().ok);
}

#[test]
fn a_cut_file_fails_check() {
    // cut in the middle of the last row
    let cut = &ATF[..ATF.len() - 6];
    let (_d, ds) = open(cut);
    let r = ds.unwrap().check().unwrap();
    assert!(!r.ok);
    assert!(
        r.findings
            .iter()
            .any(|f| f.severity == Severity::Error && f.code == "truncated")
    );
}

#[test]
fn non_time_tables_and_garbage_are_refused() {
    let genepix = "ATF\t1.0\n0\t2\n\"Block\"\t\"Column\"\n1\t2\n";
    let (_d, ds) = open(genepix);
    assert!(matches!(ds, Err(Error::Unsupported { .. })));
    let (_d2, ds) = open("ATF\t1.0\nnot counts\n");
    assert!(matches!(ds, Err(Error::Corrupt { .. })));
}

#[test]
fn atf_from_bytes_without_a_local_file() {
    let input = openreadout_core::source::Input::from_bytes("memory.atf", ATF);
    let mut ds = AtfReader.open_input(&input).unwrap();
    assert_eq!(
        ds.read_trace(0, 1, 1, 10).unwrap().channels,
        [vec![2.6, 2.7, 2.8], vec![-21.0, -22.0, -23.0]]
    );
    assert!(ds.check().unwrap().ok);
}
