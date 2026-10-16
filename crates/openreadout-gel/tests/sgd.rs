//! A synthetic Syngene GeneSys `.sgd` written by `fuzz/seeds.py` (`mini_sgd`, from the layout in
//! `docs/formats/syngene-sgd.md`): capture, settings, pixels and experiment facts; truncations
//! and flipped bytes open or fail cleanly.

use openreadout_core::source::Input;
use openreadout_core::{Dataset, FormatReader, PlaneIndex};
use openreadout_gel::SyngeneReader;

fn fixture() -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sgd-synthetic.sgd"),
    )
    .unwrap()
}

fn open(bytes: Vec<u8>) -> openreadout_core::Result<Box<dyn Dataset>> {
    SyngeneReader.open_input(&Input::from_bytes("blot.sgd", bytes))
}

#[test]
fn reads_capture() {
    let bytes = fixture();
    let d = SyngeneReader.sniff_input(&bytes[..512], &Input::from_bytes("x.bin", bytes.clone()));
    assert_eq!(d.unwrap().format_id, openreadout_gel::SYNGENE_FORMAT_ID);
    let mut ds = open(bytes).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("GeneSys 1.6.1.0"));
    let im = &info.images[0];
    assert_eq!((im.size_x, im.size_y), (4, 3));
    assert_eq!(im.channels[0].name.as_deref(), Some("ECL"));
    assert_eq!(im.channels[0].exposure_ms, Some(9500.0));
    assert_eq!(
        im.instrument.as_ref().unwrap().model.as_deref(),
        Some("G:Box mini 6")
    );
    let p = ds.read_plane(0, PlaneIndex::default()).unwrap();
    let values: Vec<u16> = p
        .data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    assert_eq!(values, (100..1300).step_by(100).collect::<Vec<u16>>());
    let e = ds.experiment().unwrap();
    assert_eq!(e.method.unwrap().technique.unwrap().id, "OBI:0000854");
    assert!(ds.check().unwrap().findings.is_empty());
}

#[test]
fn damaged_files_fail_cleanly() {
    let full = fixture();
    for n in (0..full.len()).step_by(7) {
        if let Ok(mut ds) = open(full[..n].to_vec()) {
            let _ = ds.info();
            let _ = ds.read_plane(0, PlaneIndex::default());
            let _ = ds.check();
        }
    }
    for i in (0..full.len()).step_by(3) {
        let mut b = full.clone();
        b[i] ^= 0xA5;
        if let Ok(mut ds) = open(b) {
            let _ = ds.info();
            let _ = ds.experiment();
            let _ = ds.read_plane(0, PlaneIndex::default());
            let _ = ds.check();
        }
    }
}
