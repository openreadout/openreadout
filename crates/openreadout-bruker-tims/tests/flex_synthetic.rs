//! The Bruker flex reader on `tests/fixtures/flex-synthetic/` (`oracle/make_flex_fixtures.py`):
//! a linear little-endian spot, a reflector big-endian spot with ML3 != 0 and a LIFT spot.
//! Values known by construction.
#![allow(clippy::float_cmp)] // values known exactly by construction

use std::path::{Path, PathBuf};

use openreadout_bruker_tims::BrukerFlexReader;
use openreadout_core::FormatReader;
use openreadout_core::source::Input;

fn run() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/flex-synthetic")
}

fn quadratic(i: f64, ml3: f64) -> f64 {
    let tof = 2.0 * i + 1000.0;
    let b = 1000.0;
    let c = -tof;
    let s = ((b * b - 4.0 * ml3 * c).sqrt() - b) / (2.0 * ml3);
    s * s
}

#[test]
fn reads_every_spot_of_a_run() {
    assert!(BrukerFlexReader.sniff(&[], &run()).is_some());
    let mut ds = BrukerFlexReader.open(&run()).unwrap();
    let info = ds.info().unwrap();
    let s = &info.spectra[0];
    assert_eq!(s.scan_count, 3);
    assert_eq!(s.ms_levels, vec![1, 2]);
    assert_eq!(s.extra["modes"]["linear"], 1);
    assert_eq!(s.extra["modes"]["reflector"], 1);
    assert_eq!(s.extra["modes"]["LIFT"], 1);
    assert!(
        info.notes.iter().any(|n| n.contains("NTBCal")),
        "{:?}",
        info.notes
    );

    let lin = ds.read_spectrum(0, 0).unwrap();
    assert_eq!(lin.native_id.as_deref(), Some("0_A1/1/1SLin"));
    assert_eq!(lin.polarity, "negative");
    assert_eq!(lin.ms_level, 1);
    assert_eq!(lin.extra["spot"], "A1");
    assert_eq!(
        lin.intensity,
        (0..8).map(|i| 10.0 * i as f32).collect::<Vec<_>>()
    );
    for (i, mz) in lin.mz.iter().enumerate() {
        let want = ((1000.0 + 2.0 * i as f64) / 1000.0).powi(2);
        assert!((mz - want).abs() < 1e-12, "{i}: {mz} {want}");
    }

    let refl = ds.read_spectrum(0, 1).unwrap();
    assert_eq!(refl.extra["mode"], "reflector");
    assert_eq!(refl.intensity, vec![-2.0, -1.0, 0.0, 1_048_576.0]);
    for (i, mz) in refl.mz.iter().enumerate() {
        let want = quadratic(i as f64, 0.001);
        assert!((mz - want).abs() < 1e-9, "{i}: {mz} {want}");
    }

    let lift = ds.read_spectrum(0, 2).unwrap();
    assert_eq!(lift.ms_level, 2);
    assert_eq!(lift.precursor_mz, Some(1500.5));
    assert_eq!(lift.polarity, "positive");
    assert!(lift.extra.contains_key("mz_note"));

    // a spot directory, or its fid, opens that spot alone
    let one = run().join("0_B2/1/1SRef/fid");
    let ds1 = BrukerFlexReader.open(&one).unwrap();
    assert_eq!(ds1.info().unwrap().spectra[0].scan_count, 1);
    assert!(ds.check().unwrap().findings.is_empty());

    // a folder that holds other data beside the run is not a flex run
    let fixtures = run().parent().unwrap().to_path_buf();
    assert!(BrukerFlexReader.sniff(&[], &fixtures).is_none());
}

#[test]
fn damage_is_an_error_not_a_panic() {
    let spot = run().join("0_B2/1/1SRef");
    let acqus = std::fs::read(spot.join("acqus")).unwrap();
    let fid = std::fs::read(spot.join("fid")).unwrap();
    let dir = std::env::temp_dir().join(format!("openreadout-flex-damage-{}", std::process::id()));
    let s = dir.join("0_A1/1/1SLin");
    std::fs::create_dir_all(&s).unwrap();
    let mut variants: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    for cut in (0..acqus.len()).step_by(17) {
        variants.push((acqus[..cut].to_vec(), fid.clone()));
    }
    for cut in 0..fid.len() {
        variants.push((acqus.clone(), fid[..cut].to_vec()));
    }
    for i in (0..acqus.len()).step_by(3) {
        let mut a = acqus.clone();
        a[i] ^= 0x15;
        variants.push((a, fid.clone()));
    }
    for (a, f) in variants {
        std::fs::write(s.join("acqus"), a).unwrap();
        std::fs::write(s.join("fid"), f).unwrap();
        if let Ok(mut ds) = BrukerFlexReader.open_input(&Input::local(&dir)) {
            let _ = ds.info();
            let _ = ds.read_spectrum(0, 0);
            let _ = ds.check();
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
