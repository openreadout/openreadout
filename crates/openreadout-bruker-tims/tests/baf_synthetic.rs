//! The synthetic BAF fixture (`oracle/make_baf_fixtures.py`): metadata, the packed profile, the
//! centroids, precursor fields, m/z from the recalibration and from the acquisition
//! calibration, the record walk when the trailer index is gone, and refusals of damaged copies.

use std::path::{Path, PathBuf};

use openreadout_bruker_tims::BrukerBafReader;
use openreadout_core::{FormatReader, SpectrumView};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/baf-synthetic.d")
}

/// The generator's calibration: delay 3000 ns, time base 0.5 ns, t0 2000 ns, Q from the
/// reference 0.01, the residual 1e-3 + 1e-6 m inside [50, 1500].
fn mz(index: f64, k: f64, q: f64) -> f64 {
    let t = 3000.0 + 0.5 * index;
    let k1 = 1e6 / k.sqrt();
    let x = (-k1 + (k1 * k1 - 4.0 * q * (2000.0 - t)).sqrt()) / (2.0 * q);
    let m = x * x;
    let c = 1e-3 + 1e-6 * m.clamp(50.0, 1500.0);
    let d = if m < 50.0 {
        50.0 - m
    } else if m > 1500.0 {
        m - 1500.0
    } else {
        0.0
    };
    m - c * (-d * d).exp()
}

/// Drift factor of the generator's settings (reference 30/28 °C, gradients 4 and 2 ppm/°C).
fn drift(t1: f64, t2: f64) -> f64 {
    1.0 - 1e-6 * (4.0 * (t1 - 30.0) + 2.0 * (t2 - 28.0))
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * b.abs().max(1.0)
}

#[test]
fn metadata() {
    let ds = BrukerBafReader.open(&fixture()).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("OPENREADOUT-SYNTH"));
    let run = &info.spectra[0];
    assert_eq!(run.scan_count, 2);
    assert_eq!(run.ms_levels, [1, 2]);
    assert_eq!(run.rt_range_s, Some([1.5, 2.25]));
    let i = run.instrument.as_ref().unwrap();
    assert_eq!(i.model.as_deref(), Some("synthetic TOF"));
    assert_eq!(i.software_version.as_deref(), Some("1.2.3"));
    let e = &run.extra;
    assert_eq!(e["acquired_utc"], "2026-10-12T09:15:30.250Z");
    assert_eq!(e["utc_offset_minutes"], 120);
    assert_eq!(e["sample_name"], "Synthetic sample");
    assert_eq!(e["method"], "synthetic.m");
    assert_eq!(e["mz_calibration_source"], "Calibrator.ami");
    assert_eq!(e["profile_coding"], "packed");
    assert_eq!(e["centroid_element_code"], 3);
    assert_eq!(e["index_source"], "trailer index");
}

#[test]
fn spectra_and_recalibrated_mz() {
    let mut ds = BrukerBafReader.open(&fixture()).unwrap();
    let (k_ami, q_ref) = (1.00002e8, 0.01);
    // Spectrum 1: the packed profile, m/z of the recalibration at its temperatures.
    let f1 = drift(30.5, 28.0);
    let p = ds.read_spectrum_view(0, 0, SpectrumView::Primary).unwrap();
    assert!(!p.centroided);
    let want: Vec<f32> = [0, 0, 0, 0, 5, 30, 120, 260, 120, 30, 5, 0, 0, 0, 0, 0, 3000, 70000, 2000, 0]
        .iter()
        .chain([0i32; 20].iter())
        .map(|&v| v as f32)
        .collect();
    assert_eq!(p.intensity, want);
    for (i, &m) in p.mz.iter().enumerate() {
        assert!(close(m, mz(i as f64, k_ami * f1, q_ref / f1)), "profile point {i}: {m}");
    }
    let c = ds.read_spectrum_view(0, 0, SpectrumView::Centroid).unwrap();
    assert!(c.centroided);
    assert_eq!(c.intensity, [150.0, 260.0, 70000.0]);
    for (&m, idx) in c.mz.iter().zip([7.0, 17.0, 23.5]) {
        assert!(close(m, mz(idx, k_ami * f1, q_ref / f1)));
    }
    assert_eq!(c.total_ion_current, Some(70410.0));
    assert_eq!(c.scan_window_mz, Some([50.0, 1500.0]));
    // Spectrum 2: MS/MS with its precursor fields.
    let f2 = drift(31.0, 28.5);
    let s = ds.read_spectrum(0, 1).unwrap();
    assert_eq!((s.scan_number, s.ms_level, s.rt_s), (2, 2, Some(2.25)));
    assert_eq!(s.precursor_mz, Some(300.25));
    assert_eq!(s.isolation_window_mz, Some([298.75, 301.75]));
    assert_eq!(s.precursor_charge, Some(2));
    assert_eq!(s.activation.as_deref(), Some("CID"));
    assert_eq!(s.collision_energy, Some(20.0));
    assert_eq!(s.native_id.as_deref(), Some("scan=2"));
    assert!(close(s.mz[0], mz(11.25, k_ami * f2, q_ref / f2)));
}

/// A copy of the fixture without `Calibrator.ami` and with `analysis.baf` replaced by `baf`.
fn copy(tag: &str, baf: &[u8], ami: bool) -> PathBuf {
    let d = std::env::temp_dir().join(format!("baf-{tag}-{}.d", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("analysis.baf"), baf).unwrap();
    if ami {
        std::fs::copy(fixture().join("Calibrator.ami"), d.join("Calibrator.ami")).unwrap();
    }
    d
}

#[test]
fn acquisition_calibration_and_record_walk() {
    let baf = std::fs::read(fixture().join("analysis.baf")).unwrap();
    // Without Calibrator.ami: the reference K and Q at the spectrum's temperatures.
    let d = copy("acq", &baf, false);
    let mut ds = BrukerBafReader.open(&d).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.spectra[0].extra["mz_calibration_source"], "acquisition");
    let f1 = drift(30.5, 28.0);
    let c = ds.read_spectrum_view(0, 0, SpectrumView::Centroid).unwrap();
    assert!(close(c.mz[0], mz(7.0, 1.0e8 * f1, 0.01 / f1)));
    // Without a pointer to the trailer index the records are walked.
    let mut no_trailer = baf.clone();
    no_trailer[28..36].copy_from_slice(&0u64.to_le_bytes());
    let d = copy("walk", &no_trailer, true);
    let ds = BrukerBafReader.open(&d).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.spectra[0].scan_count, 2);
    assert_eq!(info.spectra[0].extra["index_source"], "record walk");
}

#[test]
fn damaged_copies_are_refused() {
    let baf = std::fs::read(fixture().join("analysis.baf")).unwrap();
    for (tag, cut) in [("head", 20usize), ("mid", baf.len() / 2), ("tail", baf.len() - 30)] {
        let d = copy(tag, &baf[..cut], true);
        let res = BrukerBafReader.open(&d).and_then(|mut ds| {
            for k in 0..2 {
                ds.read_spectrum(0, k)?;
            }
            Ok(())
        });
        match res {
            Err(e) => assert!(matches!(e.exit_code(), 4 | 6), "{tag}: {e}"),
            // A cut trailer leaves the spectra readable through the record walk.
            Ok(()) => assert_eq!(tag, "tail"),
        }
    }
    // An unknown calibration kind: the spectrum's m/z are refused (exit 6), the run opens.
    let mut odd = baf.clone();
    let tag = 0xBFA0_0006u32.to_le_bytes();
    let at = odd
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == tag)
        .map(|(i, _)| i)
        .nth(1)
        .unwrap();
    odd[at + 8..at + 10].copy_from_slice(&7u16.to_le_bytes());
    let d = copy("kind", &odd, false);
    let mut ds = BrukerBafReader.open(&d).unwrap();
    assert_eq!(ds.read_spectrum(0, 0).unwrap_err().exit_code(), 6);
}
