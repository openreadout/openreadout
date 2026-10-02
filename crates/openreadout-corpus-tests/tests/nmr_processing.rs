//! NMR processing, peak picking and integration against vendor-processed spectra and nmrglue
//! (`oracle/signal/nmr.py` → `corpus/oracle/nmr-processing/*.json`):
//!
//! - Bruker: the FID processed here with the parameters TopSpin stored (LB, SI, PHC0/PHC1, SF)
//!   against TopSpin's own `pdata/1/1r` (read through the Bruker reader, itself validated against
//!   nmrglue): correlation over the signal points, peak positions and relative heights of the
//!   25 tallest peaks nmrglue picks on `1r`, integrals over TopSpin's stored regions against
//!   `integrals.txt`, recall of TopSpin's stored `peaklist.xml`; automatic phasing against the
//!   same `1r`; peaks of an nmrglue processing pipeline of the same FID;
//! - JEOL: the FID processed here (automatic phasing) against the MestReNova-processed spectrum
//!   of the same acquisition (JCAMP-DX export), peak positions;
//! - Magritek Spinsolve: every FID row processed here with the stored phase against the
//!   Spinsolve software's own processed spectrum of that row (`spectrum.pt1`, `*-Spectra.pt1`,
//!   read at the offsets `oracle/spinsolve.py` found): the ppm axis point by point, the
//!   correlation of the real parts, the largest difference after a least-squares scale; automatic
//!   phasing against the same spectra.
//!
//! Spectra whose TopSpin S/N is below 100 (noise-only 13C test acquisitions) and data TopSpin
//! processed with steps this crate does not do (linear prediction `ME_mod` ≠ 0, a time-domain
//! offset `TDoff` ≠ 0, analog-filter data `DSPFVS` < 0) are reported, not asserted. Processing
//! parity is asserted on what isolates the processing: the spectrum correlation with TopSpin's
//! `1r`, peak positions, and peak heights and integrals measured the same way on both spectra
//! (the same baseline correction, peak picker and integration); TopSpin's own stored peak lists
//! and integrals are reported beside them (a stored list older than the current referencing is
//! recognised and reported, not asserted).
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test nmr_processing --
//! --nocapture` (`OPENREADOUT_CORPUS_DIR` overrides `corpus/files`).
#![cfg(feature = "corpus")]
#![allow(clippy::many_single_char_names, clippy::neg_cmp_op_on_partial_ord)]

use std::path::{Path, PathBuf};

use openreadout_core::FormatReader;
use openreadout_signal::nmr::{
    self, BaselineMode, NmrRequest, PeakOptions, PhaseMode, PpmAxis, ProcessOptions,
    SpectrumSource, pick_peaks,
};
use serde::Deserialize;

/// Experiments where automatic phasing is known to land in another minimum of the objective
/// than TopSpin's stored phases (reported, not asserted; docs/formats/nmr-processing.md).
const AUTOPHASE_KNOWN_MISSES: &[&str] = &["nmrxiv-s1132-2"];

#[derive(Deserialize)]
struct OraclePeak {
    ppm: f64,
    height: f64,
}
#[derive(Deserialize)]
struct StoredPeak {
    ppm: f64,
    intensity: f64,
}
#[derive(Deserialize)]
struct StoredIntegral {
    from_ppm: f64,
    to_ppm: f64,
    value: f64,
}
#[derive(Deserialize)]
struct NmrglueFid {
    correlation_with_topspin: f64,
    peaks: Vec<OraclePeak>,
}
#[derive(Deserialize)]
struct Oracle {
    id: String,
    path: String,
    #[serde(default)]
    topspin_snr: Option<f64>,
    #[serde(default)]
    topspin_noise_sd: Option<f64>,
    #[serde(default)]
    topspin_peaks: Vec<OraclePeak>,
    #[serde(default)]
    topspin_peaklist: Vec<StoredPeak>,
    #[serde(default)]
    topspin_integrals: Vec<StoredIntegral>,
    #[serde(default)]
    nmrglue_fid: Option<NmrglueFid>,
    #[serde(default)]
    reference_peaks: Vec<OraclePeak>,
    #[serde(default)]
    procs: serde_json::Value,
    #[serde(default)]
    acqus: serde_json::Value,
    /// Spinsolve: where the software's processed spectra sit in its plot files.
    #[serde(default)]
    vendor_spectra: Vec<VendorSpectra>,
    /// Spinsolve: `Phase(p0, p1)` of `processing.script`.
    #[serde(default)]
    script_phase_deg: Option<(f64, f64)>,
}
#[derive(Deserialize)]
struct VendorSpectra {
    file: String,
    points: usize,
    axis_offsets: Vec<u64>,
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn files_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root().join("corpus/files"), PathBuf::from)
}

fn oracles() -> Vec<Oracle> {
    let dir = root().join("corpus/oracle/nmr-processing");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    v.sort();
    v.iter()
        .map(|p| serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap())
        .collect()
}

fn corr(a: &[f64], b: &[f64], mask: Option<&[bool]>) -> f64 {
    let pick = |i: usize| mask.is_none_or(|m| m[i]);
    let n = a.len().min(b.len());
    let (mut sa, mut sb, mut c) = (0.0, 0.0, 0.0);
    let mut k = 0.0;
    for i in (0..n).filter(|&i| pick(i)) {
        sa += a[i];
        sb += b[i];
        k += 1.0;
    }
    let (ma, mb) = (sa / k, sb / k);
    let (mut va, mut vb) = (0.0, 0.0);
    for i in (0..n).filter(|&i| pick(i)) {
        c += (a[i] - ma) * (b[i] - mb);
        va += (a[i] - ma).powi(2);
        vb += (b[i] - mb).powi(2);
    }
    c / (va * vb).sqrt()
}

fn nearest(peaks: &[nmr::Peak], ppm: f64) -> Option<&nmr::Peak> {
    peaks
        .iter()
        .min_by(|a, b| (a.ppm - ppm).abs().total_cmp(&(b.ppm - ppm).abs()))
}

struct Compared {
    matched: usize,
    total: usize,
    max_dppm: f64,
    max_rel_height_err: f64,
}

/// Match oracle peaks to ours within `tol_ppm`; heights compared relative to each side's
/// tallest matched peak.
fn compare_peaks(ours: &[nmr::Peak], theirs: &[OraclePeak], tol_ppm: f64) -> Compared {
    let mut pairs = Vec::new();
    for t in theirs {
        if let Some(o) = nearest(ours, t.ppm)
            && (o.ppm - t.ppm).abs() <= tol_ppm
        {
            pairs.push((o.ppm - t.ppm, o.height, t.height));
        }
    }
    let max_dppm = pairs.iter().map(|p| p.0.abs()).fold(0.0, f64::max);
    let (om, tm) = pairs
        .iter()
        .fold((0.0_f64, 0.0_f64), |(a, b), p| (a.max(p.1), b.max(p.2)));
    let max_rel = pairs
        .iter()
        .map(|p| (p.1 / om - p.2 / tm).abs())
        .fold(0.0, f64::max);
    Compared {
        matched: pairs.len(),
        total: theirs.len(),
        max_dppm,
        max_rel_height_err: max_rel,
    }
}

fn open(path: &Path) -> Box<dyn openreadout_core::Dataset> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    if ext.eq_ignore_ascii_case("jdf") {
        openreadout_nmr::JeolReader.open(path).unwrap()
    } else if openreadout_nmr::resolve_spinsolve_dir_in(
        &openreadout_core::source::Fs::local(),
        path,
    )
    .is_some()
    {
        openreadout_nmr::SpinsolveReader.open(path).unwrap()
    } else {
        openreadout_nmr::BrukerReader.open(path).unwrap()
    }
}

fn process(
    ds: &mut dyn openreadout_core::Dataset,
    phase: PhaseMode,
    baseline: BaselineMode,
) -> (nmr::Spectrum, nmr::ProcessingRecord) {
    let info = ds.info().unwrap();
    let req = NmrRequest {
        source: SpectrumSource::Fid,
        process: ProcessOptions {
            phase,
            baseline,
            ..ProcessOptions::default()
        },
        ..NmrRequest::default()
    };
    let c = nmr::load(ds, &info, &req).unwrap();
    (c.spectrum, c.processing.unwrap())
}

fn peaks(y: &[f64], axis: &PpmAxis, min_snr: f64) -> Vec<nmr::Peak> {
    pick_peaks(
        y,
        axis,
        &PeakOptions {
            min_snr,
            ..PeakOptions::default()
        },
    )
    .0
}

#[test]
fn bruker_fid_processing_matches_topspin() {
    let dir = files_dir();
    let mut failures = Vec::new();
    let mut checked = 0;
    for o in oracles() {
        if o.reference_peaks.is_empty() && o.topspin_snr.is_none() {
            continue;
        }
        if !o.reference_peaks.is_empty() {
            continue; // JEOL, below
        }
        let path = dir.join(&o.path);
        if !path.exists() {
            continue;
        }
        checked += 1;
        let snr = o.topspin_snr.unwrap_or(0.0);
        let num = |v: &serde_json::Value| v.as_f64().unwrap_or(0.0);
        let unsupported_step = num(&o.procs["ME_mod"]) != 0.0
            || num(&o.procs["TDoff"]) != 0.0
            || num(&o.acqus["DSPFVS"]) < 0.0;
        let strict = snr >= 100.0 && !unsupported_step;
        let mut ds = open(&path);
        let info = ds.info().unwrap();
        let pdata = info
            .traces
            .iter()
            .find(|t| t.name.as_deref() == Some("pdata/1"))
            .unwrap()
            .index;
        let (topspin, taxis) = nmr::source::load_spectrum(ds.as_mut(), &info, pdata).unwrap();
        let (ours, rec) = process(ds.as_mut(), PhaseMode::Stored, BaselineMode::None);
        let sd = o.topspin_noise_sd.unwrap_or(0.0);
        let mask: Vec<bool> = topspin.iter().map(|v| v.abs() > 10.0 * sd).collect();
        let n_sig = mask.iter().filter(|m| **m).count();
        let c_all = corr(&ours.real, &topspin, None);
        let c_sig = if n_sig > 10 {
            corr(&ours.real, &topspin, Some(&mask))
        } else {
            f64::NAN
        };
        let scale: f64 = {
            let (mut num, mut den) = (0.0, 0.0);
            for (a, b) in ours.real.iter().zip(&topspin) {
                num += a * b;
                den += a * a;
            }
            num / den
        };
        let axis_ok = (ours.axis.first_ppm - taxis.first_ppm).abs() < 1e-4
            && (ours.axis.step_ppm - taxis.step_ppm).abs() < 1e-9;
        // peaks on the baseline-corrected spectrum (TopSpin's 1r has no DC offset; ours, without
        // baseline correction, keeps the constant the first point contributes)
        let (based, _) = process(ds.as_mut(), PhaseMode::Stored, BaselineMode::Default);
        let our_peaks = peaks(&based.real, &based.axis, 20.0);
        let step = ours.axis.step_ppm.abs();
        let cmp = compare_peaks(&our_peaks, &o.topspin_peaks, 3.0 * step);
        // TopSpin's 1r with the same baseline correction and the same picker: heights and
        // integrals measured identically on both spectra isolate the processing
        let mut ts_based = topspin.clone();
        nmr::baseline::correct(&mut ts_based, BaselineMode::Default);
        let poly5 = BaselineMode::Polynomial { order: 5 };
        let (ours5, _) = process(ds.as_mut(), PhaseMode::Stored, poly5);
        let mut ts5 = topspin.clone();
        nmr::baseline::correct(&mut ts5, poly5);
        let ts_picks: Vec<OraclePeak> = peaks(&ts5, &taxis, 20.0)
            .iter()
            .take(25)
            .map(|p| OraclePeak {
                ppm: p.ppm,
                height: p.height,
            })
            .collect();
        let same = compare_peaks(
            &peaks(&ours5.real, &ours5.axis, 20.0),
            &ts_picks,
            3.0 * step,
        );
        // a spectrum TopSpin never phased (PHC0 = PHC1 = 0) is not an absorption spectrum:
        // heights and automatic phasing are not compared against it
        let unphased = num(&o.procs["PHC0"]) == 0.0 && num(&o.procs["PHC1"]) == 0.0;
        // automatic phasing against TopSpin's spectrum
        let (auto, arec) = process(ds.as_mut(), PhaseMode::Auto, BaselineMode::None);
        let c_auto = if n_sig > 10 {
            corr(&auto.real, &topspin, Some(&mask))
        } else {
            f64::NAN
        };
        println!(
            "{}: S/N {snr:.0}{}; stored-phase corr {c_all:.6} (signal {c_sig:.6}, {n_sig} pts), scale {scale:.4}, axis match {axis_ok}; peaks {}/{} matched, max |dppm| {:.2e} ({:.2} pts), max rel height err {:.4} (same picker on 1r: {}/{}, {:.4}); auto phase {:.1}/{:.1} (stored {:.1}/{:.1}) signal corr {c_auto:.5}",
            o.id,
            if unsupported_step {
                " (processed with steps not reproduced: reported only)"
            } else {
                ""
            },
            cmp.matched,
            cmp.total,
            cmp.max_dppm,
            cmp.max_dppm / step,
            cmp.max_rel_height_err,
            same.matched,
            same.total,
            same.max_rel_height_err,
            arec.phase0_deg,
            arec.phase1_deg,
            rec.phase0_deg,
            rec.phase1_deg,
        );
        if let Some(ng) = &o.nmrglue_fid {
            let c = compare_peaks(&our_peaks, &ng.peaks, 3.0 * step);
            println!(
                "    nmrglue pipeline (corr with TopSpin {:.4}): peaks {}/{} matched, max |dppm| {:.2e}",
                ng.correlation_with_topspin, c.matched, c.total, c.max_dppm
            );
        }
        if !o.topspin_peaklist.is_empty() {
            // TopSpin's stored list against ours (default thresholds, stored spectrum)
            let tall = o
                .topspin_peaklist
                .iter()
                .map(|p| p.intensity)
                .fold(0.0, f64::max);
            let strong: Vec<&StoredPeak> = o
                .topspin_peaklist
                .iter()
                .filter(|p| p.intensity >= 0.05 * tall)
                .collect();
            let ours_default = peaks(&based.real, &based.axis, 10.0);
            let theirs_default = peaks(&ts_based, &taxis, 10.0);
            let tol = step.max(0.002);
            let found = |list: &[nmr::Peak]| {
                strong
                    .iter()
                    .filter(|p| nearest(list, p.ppm).is_some_and(|q| (q.ppm - p.ppm).abs() <= tol))
                    .count()
            };
            let (hit, hit_ts) = (found(&ours_default), found(&theirs_default));
            // a list TopSpin's own current 1r does not reproduce is older than its referencing
            let stale = (hit_ts as f64) < 0.9 * strong.len() as f64;
            println!(
                "    TopSpin peaklist.xml: {hit}/{} peaks of >= 5 % of the tallest found within max(0.002 ppm, 1 point) (on TopSpin's 1r: {hit_ts}{})",
                strong.len(),
                if stale {
                    ", a list older than the current referencing: not asserted"
                } else {
                    ""
                }
            );
            if strict && !stale && (hit as f64) < 0.9 * hit_ts as f64 {
                failures.push(format!("{}: peaklist recall {hit}/{}", o.id, strong.len()));
            }
        }
        if !o.topspin_integrals.is_empty() {
            let regions: Vec<(f64, f64)> = o
                .topspin_integrals
                .iter()
                .map(|r| (r.from_ppm, r.to_ppm))
                .collect();
            let refv = o.topspin_integrals[0].value;
            let fid_int = nmr::integrate(&based.real, &based.axis, &regions, Some((0, refv)));
            let ts_int = nmr::integrate(&topspin, &taxis, &regions, Some((0, refv)));
            let err = |v: &[nmr::Integral]| {
                v.iter()
                    .zip(&o.topspin_integrals)
                    .map(|(a, b)| {
                        (a.normalized.unwrap_or(f64::NAN) - b.value).abs() / b.value.abs()
                    })
                    .fold(0.0, f64::max)
            };
            let (e_fid, e_ts) = (err(&fid_int), err(&ts_int));
            // the same integration on TopSpin's 1r with the same baseline correction: parity of
            // the processing, as an absolute difference in units of the reference region. TopSpin's
            // 1r often carries its own polynomial baseline correction (`abs`, ABSG 5), a smooth
            // curve our order-1 default leaves in the difference, so a 5th-order correction is
            // applied to both spectra for this comparison.
            // in units of the largest region TopSpin integrated
            let big = o
                .topspin_integrals
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.value.abs().total_cmp(&b.1.value.abs()))
                .map_or(0, |(i, _)| i);
            let diff = |a: &[f64], ax: &PpmAxis, b: &[f64], bx: &PpmAxis| {
                let x = nmr::integrate(a, ax, &regions, Some((big, 1.0)));
                let y = nmr::integrate(b, bx, &regions, Some((big, 1.0)));
                x.iter()
                    .zip(&y)
                    .map(|(a, b)| {
                        (a.normalized.unwrap_or(f64::NAN) - b.normalized.unwrap_or(f64::NAN)).abs()
                    })
                    .fold(0.0, f64::max)
            };
            let e_default = diff(&based.real, &based.axis, &ts_based, &taxis);
            let e_same = diff(&ours5.real, &ours5.axis, &ts5, &taxis);
            println!(
                "    integrals ({} regions): against TopSpin's stored values, from FID max rel err {e_fid:.4}, on TopSpin 1r {e_ts:.4}; same integration on both spectra: max |difference| {e_default:.4} (order-1 baseline), {e_same:.4} (order 5) of the largest region",
                regions.len()
            );
            if strict && !(e_same <= 0.01) {
                failures.push(format!(
                    "{}: integrals differ from TopSpin's 1r by {e_same:.4} (same integration)",
                    o.id
                ));
            }
        }
        if strict {
            if !(c_sig >= 0.999) {
                failures.push(format!("{}: signal correlation {c_sig}", o.id));
            }
            if !axis_ok {
                failures.push(format!("{}: ppm axis differs from TopSpin's", o.id));
            }
            if cmp.matched < cmp.total * 9 / 10 || cmp.max_dppm > 1.5 * step {
                failures.push(format!(
                    "{}: peaks {}/{} within 3 points, max |dppm| {}",
                    o.id, cmp.matched, cmp.total, cmp.max_dppm
                ));
            }
            if !unphased && (same.max_rel_height_err > 0.02 || same.matched < same.total * 8 / 10) {
                failures.push(format!(
                    "{}: heights against TopSpin's 1r (same picker) {}/{} matched, error {}",
                    o.id, same.matched, same.total, same.max_rel_height_err
                ));
            }
            if !unphased && !AUTOPHASE_KNOWN_MISSES.contains(&o.id.as_str()) && !(c_auto >= 0.99) {
                failures.push(format!("{}: auto-phase signal correlation {c_auto}", o.id));
            }
        }
    }
    println!("{checked} Bruker experiments checked");
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn jeol_fid_processing_matches_mestrenova() {
    let dir = files_dir();
    let mut failures = Vec::new();
    for o in oracles()
        .into_iter()
        .filter(|o| !o.reference_peaks.is_empty())
    {
        let path = dir.join(&o.path);
        if !path.exists() {
            continue;
        }
        let mut ds = open(&path);
        let (ours, rec) = process(ds.as_mut(), PhaseMode::Default, BaselineMode::Default);
        let our_peaks = peaks(&ours.real, &ours.axis, 20.0);
        let cmp = compare_peaks(&our_peaks, &o.reference_peaks, 0.003);
        println!(
            "{}: phase {} {:.1}/{:.1}, group delay {:.3} points; MestReNova peaks {}/{} matched within 0.003 ppm, max |dppm| {:.2e}",
            o.id,
            rec.phase_mode,
            rec.phase0_deg,
            rec.phase1_deg,
            rec.group_delay_points,
            cmp.matched,
            cmp.total,
            cmp.max_dppm
        );
        if cmp.matched < cmp.total * 8 / 10 {
            failures.push(format!("{}: {}/{} peaks", o.id, cmp.matched, cmp.total));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// f32 values at `off` in `b`.
fn f32s(b: &[u8], off: usize, n: usize) -> Vec<f64> {
    b[off..off + 4 * n]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f64::from(f32::from_le_bytes(*c)))
        .collect()
}

#[test]
fn spinsolve_fid_processing_matches_the_software() {
    let dir = files_dir();
    let mut failures = Vec::new();
    let mut rows = 0;
    for o in oracles()
        .into_iter()
        .filter(|o| !o.vendor_spectra.is_empty())
    {
        let path = dir.join(&o.path);
        if !path.exists() {
            continue;
        }
        let exp = if path.is_dir() {
            path.clone()
        } else {
            path.parent().unwrap().to_path_buf()
        };
        for vs in &o.vendor_spectra {
            let bytes = std::fs::read(exp.join(&vs.file)).unwrap();
            let n = vs.points;
            let (mut worst_stored, mut worst_auto, mut worst_diff) = (1.0f64, 1.0f64, 0.0f64);
            let mut mixed_rows = 0;
            for (row, &off) in vs.axis_offsets.iter().enumerate() {
                let off = usize::try_from(off).unwrap();
                let axis = f32s(&bytes, off, n);
                let vals = f32s(&bytes, off + 4 * n, 2 * n);
                let vendor_re: Vec<f64> = vals.iter().step_by(2).copied().collect();
                let mut ds = open(&path);
                let info = ds.info().unwrap();
                let run = |ds: &mut dyn openreadout_core::Dataset, phase: PhaseMode| {
                    let req = NmrRequest {
                        source: SpectrumSource::Fid,
                        sweep: u32::try_from(row).unwrap(),
                        process: ProcessOptions {
                            phase,
                            baseline: BaselineMode::None,
                            ..ProcessOptions::default()
                        },
                        ..NmrRequest::default()
                    };
                    let c = nmr::load(ds, &info, &req).unwrap();
                    (c.spectrum, c.processing.unwrap())
                };
                // the software's phase, as `processing.script` records it (Phase(p0, p1): the
                // spectrum times e^{+i p0}; this crate's phases rotate by e^{-i phi})
                let (p0, p1) = o.script_phase_deg.unwrap_or((0.0, 0.0));
                let (ours, _) = run(
                    ds.as_mut(),
                    PhaseMode::Manual {
                        phase0_deg: -p0,
                        phase1_deg: -p1,
                    },
                );
                let (_, rec) = run(ds.as_mut(), PhaseMode::Default);
                assert_eq!(ours.real.len(), n, "{}: size", o.id);
                // ours: index 0 = high ppm; the software's: ascending; point k <-> n - k
                let mapped: Vec<f64> = (0..n).map(|k| vendor_re[(n - k) % n]).collect();
                let max_dppm = (1..n)
                    .map(|k| {
                        let ppm = ours.axis.first_ppm + ours.axis.step_ppm * k as f64;
                        (ppm - axis[n - k]).abs()
                    })
                    .fold(0.0, f64::max);
                let c_stored = corr(&ours.real, &mapped, None);
                let scale = ours
                    .real
                    .iter()
                    .zip(&mapped)
                    .map(|(a, b)| a * b)
                    .sum::<f64>()
                    / ours.real.iter().map(|a| a * a).sum::<f64>();
                let vmax = mapped.iter().map(|v| v.abs()).fold(0.0, f64::max);
                let diff = ours
                    .real
                    .iter()
                    .zip(&mapped)
                    .map(|(a, b)| (a * scale - b).abs())
                    .fold(0.0, f64::max)
                    / vmax;
                let (auto, _) = run(ds.as_mut(), PhaseMode::Auto);
                // over the signal points (the Bruker check does the same)
                let signal: Vec<bool> = mapped.iter().map(|v| v.abs() > 0.02 * vmax).collect();
                let c_auto = corr(&auto.real, &mapped, Some(&signal));
                println!(
                    "{} {} row {row}: default phase {} {:.3}; axis max |dppm| {max_dppm:.2e}; software-phase corr {c_stored:.7}, max diff {diff:.2e} of max; auto phase corr {c_auto:.5}",
                    o.id, vs.file, rec.phase_mode, rec.phase0_deg
                );
                rows += 1;
                worst_stored = worst_stored.min(c_stored);
                worst_diff = worst_diff.max(diff);
                if max_dppm > 2e-5 {
                    failures.push(format!("{} row {row}: ppm axis off by {max_dppm}", o.id));
                }
                if !(c_stored >= 0.999) {
                    failures.push(format!(
                        "{} row {row}: software-phase correlation {c_stored}",
                        o.id
                    ));
                }
                // inversion-recovery rows are negative: automatic phasing may flip the sign;
                // unphased software spectra (Phase(0,0)) are not a phasing reference, and rows
                // near the inversion null (lines of both signs) have no sign-free phase
                let phased = p0 != 0.0 || p1 != 0.0;
                let big = |v: &&f64| v.abs() > 0.05 * vmax;
                let pos: f64 = mapped.iter().filter(big).filter(|v| **v > 0.0).sum();
                let neg: f64 = -mapped.iter().filter(big).filter(|v| **v < 0.0).sum::<f64>();
                let mixed = pos.min(neg) / (pos + neg) > 0.05;
                if mixed {
                    mixed_rows += 1;
                } else if phased {
                    worst_auto = worst_auto.min(c_auto.abs());
                }
                if phased && !mixed && !(c_auto.abs() >= 0.9) {
                    failures.push(format!(
                        "{} row {row}: automatic phasing correlation {c_auto}",
                        o.id
                    ));
                }
            }
            println!(
                "{} {}: {} rows, script phase {:?}; worst software-phase corr {worst_stored:.7}, worst max diff {worst_diff:.2e}, worst |auto corr| {worst_auto:.5} (phased rows; {mixed_rows} rows near the inversion null not asserted)",
                o.id,
                vs.file,
                vs.axis_offsets.len(),
                o.script_phase_deg
            );
        }
    }
    println!("{rows} Spinsolve rows checked");
    assert!(failures.is_empty(), "{failures:#?}");
}
