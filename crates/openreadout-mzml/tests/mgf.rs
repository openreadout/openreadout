//! MGF reading on a hand-written file (`tests/fixtures/mgf-synthetic.mgf`): file-level charge,
//! retention-time ranges, chimeric precursors, polarity, peak charges, case-insensitive keys,
//! and truncation.

use std::path::Path;

use openreadout_core::FormatReader;
use openreadout_core::source::Input;
use openreadout_mzml::MgfReader;

fn fixture() -> &'static Path {
    Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/mgf-synthetic.mgf"
    ))
}

#[test]
fn reads_every_block() {
    let head = std::fs::read(fixture()).unwrap();
    let d = MgfReader.sniff(&head, fixture()).unwrap();
    assert_eq!(d.format_id, "mgf");
    let mut ds = MgfReader.open(fixture()).unwrap();
    let info = ds.info().unwrap();
    let run = &info.spectra[0];
    assert_eq!(run.scan_count, 3);
    assert_eq!(run.ms_levels, vec![2]);
    assert_eq!(run.rt_range_s, Some([95.5, 101.5]));
    assert_eq!(run.extra["total_points"], 7);
    assert_eq!(run.extra["chimeric_spectra"], 1);
    assert_eq!(run.extra["writer"], "ProteoWizard");

    let a = ds.read_spectrum(0, 0).unwrap();
    assert_eq!(a.scan_number, 17);
    assert_eq!(a.precursor_mz, Some(600.25));
    assert_eq!(a.precursor_intensity, Some(15000.0));
    assert_eq!(a.precursor_charge, Some(2));
    assert_eq!(a.rt_s, Some(95.5));
    assert_eq!(a.extra["rt_range_s"], serde_json::json!([95.5, 97.25]));
    assert_eq!(
        a.native_id.as_deref(),
        Some("controllerType=0 controllerNumber=1 scan=17")
    );
    assert_eq!(a.mz, vec![110.07, 175.119, 800.4]);
    assert_eq!(a.intensity, vec![120.5, 300.25, 55.0]);
    assert_eq!(a.extra["peak_charges"], serde_json::json!([null, 1, null]));

    let b = ds.read_spectrum(0, 1).unwrap();
    assert_eq!(b.scan_number, 2);
    assert_eq!(b.polarity, "negative");
    assert_eq!(b.precursor_mz, Some(450.5));
    assert_eq!(b.extra["precursors"], serde_json::json!([450.5, 451.75]));
    // the file-level CHARGE names two charges
    assert_eq!(b.precursor_charge, None);
    assert_eq!(b.extra["charges"], serde_json::json!([2, 3]));

    let c = ds.read_spectrum(0, 2).unwrap();
    assert_eq!(c.extra["title"], "lower case keys, file-level charge");
    assert_eq!(c.intensity, vec![1000.0, 250.0]);
    assert!(ds.check().unwrap().findings.is_empty());
    assert_eq!(ds.find_spectrum(0, 17).unwrap(), Some(0));
}

#[test]
fn truncated_file_keeps_complete_blocks() {
    let text = std::fs::read_to_string(fixture()).unwrap();
    let cut = text.rfind("end ions").unwrap();
    let input = Input::from_bytes("cut.mgf", text.as_bytes()[..cut].to_vec());
    let mut ds = MgfReader.open_input(&input).unwrap();
    assert_eq!(ds.info().unwrap().spectra[0].scan_count, 2);
    let rep = ds.check().unwrap();
    assert!(rep.findings.iter().any(|f| f.code == "truncated"));
}

#[test]
fn bad_peak_is_an_error_not_a_panic() {
    let input = Input::from_bytes(
        "bad.mgf",
        b"BEGIN IONS\nPEPMASS=500\n100.5\nEND IONS\n".to_vec(),
    );
    let mut ds = MgfReader.open_input(&input).unwrap();
    assert!(ds.read_spectrum(0, 0).is_err());
    assert!(!ds.check().unwrap().findings.is_empty());
}
