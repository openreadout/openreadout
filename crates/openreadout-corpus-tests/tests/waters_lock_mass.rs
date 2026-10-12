//! Waters lock-mass correction (`Dataset::set_lock_mass`, `docs/provenance/waters-raw.md`,
//! 2026-10-12) against MassLynx's own correction of the same acquisition: the ProteoWizard test
//! data hold one SYNAPT G2-Si run as acquired (profile, uncorrected) and as MassLynx centroided it
//! with lock-mass correction at m/z 684.3469 and at 785.8426. Our corrected profile peaks must lie
//! where MassLynx put its corrected centroids, to the few ppm by which two centroiding methods
//! differ.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test waters_lock_mass -- --nocapture`
#![cfg(feature = "corpus")]
#![allow(clippy::many_single_char_names)] // m/z helpers and scan triples (a: ours, r: raw, c: MassLynx)

use std::path::{Path, PathBuf};

use openreadout_core::FormatReader;
use openreadout_waters::WatersRawReader;

fn corpus_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR").map_or_else(
        || {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../corpus/files")
                .canonicalize()
                .unwrap()
        },
        PathBuf::from,
    )
}

/// Intensity-weighted centroid of the contiguous points above half the apex, the apex being
/// the most intense point within `half` of `around`.
fn centroid(mz: &[f64], it: &[f32], around: f64, half: f64) -> Option<f64> {
    let lo = mz.partition_point(|&m| m < around - half);
    let hi = mz.partition_point(|&m| m <= around + half);
    if hi < lo + 3 {
        return None;
    }
    let k = (lo..hi).max_by(|&a, &b| it[a].total_cmp(&it[b]))?;
    let (mut a, mut b) = (k, k);
    while a > lo && it[a - 1] > it[k] / 2.0 {
        a -= 1;
    }
    while b + 1 < hi && it[b + 1] > it[k] / 2.0 {
        b += 1;
    }
    let (n, d) = (a..=b).fold((0.0, 0.0), |(n, d), j| {
        (n + mz[j] * f64::from(it[j]), d + f64::from(it[j]))
    });
    Some(n / d)
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

#[test]
fn profile_corrected_like_masslynx() {
    let dir = corpus_dir().join("pwiz-waters");
    let profile = dir.join("ATEHLSTLSEK_profile.raw");
    if !profile.exists() {
        eprintln!("skip: {} missing", profile.display());
        return;
    }
    for (reference, corrected) in [
        (684.3469, "ATEHLSTLSEK_LM_684.3469.raw"),
        (785.8426, "ATEHLSTLSEK_LM_785.8426.raw"),
    ] {
        let mut ours = WatersRawReader.open(&profile).unwrap();
        let mut raw = WatersRawReader.open(&profile).unwrap();
        let mut theirs = WatersRawReader.open(&dir.join(corrected)).unwrap();
        assert!(ours.set_lock_mass(Some(reference)).unwrap());
        let n = ours.info().unwrap().spectra[0].scan_count;
        let (mut after, mut before) = (Vec::new(), Vec::new());
        for i in 0..n {
            let a = ours.read_spectrum(0, i).unwrap();
            if a.extra.contains_key("lock_mass_reference") {
                continue;
            }
            let r = raw.read_spectrum(0, i).unwrap();
            let c = theirs.read_spectrum(0, i).unwrap();
            assert!(a.extra.contains_key("lock_mass_correction"));
            assert!(
                !r.extra.contains_key("lock_mass_correction"),
                "raw by default"
            );
            let mut idx: Vec<usize> = (0..c.mz.len()).collect();
            idx.sort_by(|&x, &y| c.intensity[y].total_cmp(&c.intensity[x]));
            for &j in idx.iter().take(12) {
                let m = c.mz[j];
                if let Some(x) = centroid(&a.mz, &a.intensity, m, 0.1) {
                    after.push((x / m - 1.0) * 1e6);
                }
                if let Some(x) = centroid(&r.mz, &r.intensity, m, 0.1) {
                    before.push((x / m - 1.0) * 1e6);
                }
            }
        }
        assert!(
            after.len() >= 50,
            "{corrected}: {} peaks compared",
            after.len()
        );
        let (m_after, m_before) = (median(after.clone()), median(before));
        let abs_after = median(after.iter().map(|x| x.abs()).collect());
        println!(
            "{corrected}: {} peaks; median offset from MassLynx {m_before:.2} ppm uncorrected, {m_after:.2} ppm corrected (median |ppm| {abs_after:.2})",
            after.len()
        );
        assert!(m_after.abs() < 2.5 && abs_after < 3.0, "{corrected}");
    }
}

/// A file that records its Lock Mass in `_extern.inf` corrects with `auto` (no m/z given).
#[test]
fn auto_takes_the_files_lock_mass() {
    let path = corpus_dir().join("mtbls13432-xevog2xs-b008-lpos");
    if !path.exists() {
        eprintln!("skip: {} missing", path.display());
        return;
    }
    let mut ds = WatersRawReader.open(&path).unwrap();
    assert!(ds.set_lock_mass(None).unwrap());
    let sp = ds.read_spectrum(0, 1).unwrap();
    let c = &sp.extra["lock_mass_correction"];
    assert_eq!(c["reference_mz"], 556.2771);
    assert!(c["ppm"].as_f64().unwrap().abs() < 200.0, "{c}");
}
