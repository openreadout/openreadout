//! Gel and blot imager and Incucyte TIFFs (`docs/formats/tiff.md` § Gel and blot imagers,
//! § Incucyte): synthetic files written by `fuzz/seeds.py` (`imager_seeds`). Their settings are
//! read, stored values are left alone, and every truncation opens or fails cleanly.

use openreadout_core::source::Input;
use openreadout_core::{Dataset, FormatReader, PlaneIndex};
use openreadout_tiff::TiffReader;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/imagers")
            .join(name),
    )
    .unwrap()
}

fn open(name: &str, bytes: Vec<u8>) -> openreadout_core::Result<Box<dyn Dataset>> {
    TiffReader.open_input(&Input::from_bytes(name, bytes))
}

#[test]
fn amersham_settings_and_stored_values() {
    let mut ds = open("a.tif", fixture("imager-amersham.tif")).unwrap();
    let info = ds.info().unwrap();
    assert!(
        info.notes
            .iter()
            .any(|n| n == "TIFF sub-format: cytiva-imager")
    );
    assert!(info.notes.iter().any(|n| n.contains("WhiteIsZero")));
    assert!(info.notes.iter().any(|n| n.contains("dark offset of 1000")));
    let im = &info.images[0];
    assert_eq!(im.channels[0].name.as_deref(), Some("Chemiluminescence"));
    assert_eq!(im.channels[0].exposure_ms, Some(5000.0));
    assert_eq!(im.acquired_at.as_deref(), Some("2023-10-31T07:47:31"));
    let inst = im.instrument.as_ref().unwrap();
    assert_eq!(inst.model.as_deref(), Some("Amersham Imager 680"));
    assert_eq!(im.extra["imager"]["ccd_temperature_c"], -25.0);
    let e = ds.experiment().unwrap();
    assert_eq!(e.instrument.unwrap().serial.as_deref(), Some("1"));
    assert_eq!(e.method.unwrap().technique.unwrap().id, "OBI:0000854");
    // values as stored (not inverted, not offset-corrected): 1000, 1100, ...
    let p = ds.read_plane(0, PlaneIndex::default()).unwrap();
    assert_eq!(&p.data[..4], &[0xE8, 0x03, 0x4C, 0x04]);
}

#[test]
fn glp_block_and_incucyte_page() {
    let info = open("g.tif", fixture("imager-glp.tif"))
        .unwrap()
        .info()
        .unwrap();
    assert!(
        info.notes
            .iter()
            .any(|n| n == "TIFF sub-format: uvitec-vilber")
    );
    let im = &info.images[0];
    assert_eq!(im.channels[0].exposure_ms, Some(11_200.0));
    assert_eq!(
        im.instrument.as_ref().unwrap().manufacturer.as_deref(),
        Some("Uvitec")
    );
    let info = open("i.tif", fixture("incucyte.tif"))
        .unwrap()
        .info()
        .unwrap();
    assert_eq!(info.images.len(), 1);
    let im = &info.images[0];
    assert_eq!(im.pyramid_levels, 1);
    assert_eq!(im.physical_size.x, Some(1.244));
    assert_eq!(im.acquired_at.as_deref(), Some("2020-07-22T13:33:44"));
    assert_eq!(im.channels[0].name.as_deref(), Some("HD Phase Image"));
}

#[test]
fn truncated_files_fail_cleanly() {
    for name in ["imager-amersham.tif", "imager-glp.tif", "incucyte.tif"] {
        let full = fixture(name);
        for n in 0..full.len() {
            if let Ok(mut ds) = open(name, full[..n].to_vec()) {
                let _ = ds.info();
                let _ = ds.experiment();
                let _ = ds.read_plane(0, PlaneIndex::default());
                let _ = ds.check();
            }
        }
    }
}
