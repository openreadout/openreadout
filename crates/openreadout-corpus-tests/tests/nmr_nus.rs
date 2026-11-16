//! Reconstruction of non-uniformly sampled (NUS) 2D NMR data (`nmr::nus`):
//!
//! - simulated sampling: fully sampled Bruker experiments that match TopSpin's `2rr` at ≥ 0.999
//!   (`tests/nmr_processing_2d.rs`), with 25 % and 50 % of the increments kept on a Poisson-gap
//!   schedule. The reconstructed and the zero-filled spectra are compared with the fully sampled
//!   one: signal correlation (points where |full| > 10 × its noise SD), the cross peaks
//!   `pick_cross_peaks` finds on the full spectrum (found within two points, height ratio) and
//!   false cross peaks (picked on the sparse spectrum, no full-spectrum peak within two points);
//! - measured NUS sets: every manifest-listed Bruker experiment with a `nuslist` and TopSpin's
//!   reconstructed `pdata/1/2rr`, processed with reconstruction and with zero filling, against
//!   TopSpin's spectrum; and the MQMAS pair `desperate-rbno3-mqmas-30` (NUS) against its fully
//!   sampled twin `-20`, both processed here.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test
//! nmr_nus -- --nocapture` (`OPENREADOUT_CORPUS_DIR` overrides `corpus/files`).
#![cfg(feature = "corpus")]
// `!(x >= t)` is the deliberate NaN-rejecting form of a threshold test
#![allow(clippy::many_single_char_names, clippy::neg_cmp_op_on_partial_ord)]

use std::path::{Path, PathBuf};
use std::time::Instant;

use openreadout_core::{Dataset, FormatReader};
use openreadout_signal::nmr::{
    self, CrossPeakOptions, NdOptions, NusMode, SpectrumNd, indirect, nus,
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn files_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root().join("corpus/files"), PathBuf::from)
}

/// Is the data set's top directory listed in the manifest (clean-room policy rule 1)?
fn listed(rel: &str) -> bool {
    let manifest = std::fs::read_to_string(root().join("corpus/manifest.toml")).unwrap_or_default();
    let top = rel.split('/').next().unwrap_or(rel);
    manifest.contains(&format!("filename = \"{top}"))
}

fn corr(a: &[f64], b: &[f64], mask: Option<&[bool]>) -> f64 {
    let pick = |i: usize| mask.is_none_or(|m| m[i]);
    let n = a.len().min(b.len());
    let (mut sa, mut sb, mut k) = (0.0, 0.0, 0.0);
    for i in (0..n).filter(|&i| pick(i)) {
        sa += a[i];
        sb += b[i];
        k += 1.0;
    }
    let (ma, mb) = (sa / k, sb / k);
    let (mut c, mut va, mut vb) = (0.0, 0.0, 0.0);
    for i in (0..n).filter(|&i| pick(i)) {
        c += (a[i] - ma) * (b[i] - mb);
        va += (a[i] - ma).powi(2);
        vb += (b[i] - mb).powi(2);
    }
    c / (va * vb).sqrt()
}

fn robust_sd(v: &[f64]) -> f64 {
    let mut s: Vec<f64> = v
        .iter()
        .step_by((v.len() / 200_000).max(1))
        .copied()
        .collect();
    s.sort_by(f64::total_cmp);
    let med = s[s.len() / 2];
    let mut d: Vec<f64> = s.iter().map(|x| (x - med).abs()).collect();
    d.sort_by(f64::total_cmp);
    1.4826 * d[d.len() / 2]
}

/// Correlation of `a` with `reference` over the reference's signal points.
fn signal_corr(a: &[f64], reference: &[f64]) -> f64 {
    let sd = robust_sd(reference);
    let mask: Vec<bool> = reference.iter().map(|v| v.abs() > 10.0 * sd).collect();
    if mask.iter().filter(|m| **m).count() > 10 {
        corr(a, reference, Some(&mask))
    } else {
        corr(a, reference, None)
    }
}

fn open(path: &Path) -> Box<dyn Dataset> {
    openreadout_nmr::BrukerReader.open(path).unwrap()
}

/// Cross peaks at least this many noise SDs of the fully sampled spectrum high are compared.
const STRONG_SNR: f64 = 20.0;

struct PeakMatch {
    reference: usize,
    found: usize,
    false_peaks: usize,
    /// Median and largest |height / reference height − 1| of the found peaks.
    median_height_err: f64,
    max_height_err: f64,
}

/// The strong cross peaks of `reference` (`pick_cross_peaks`, at least `STRONG_SNR` noise SDs)
/// found again in `s` (its tallest point within one point of the apex has the same sign and at
/// least half the height; the error is that height against the same of `reference`), and the strong cross
/// peaks `pick_cross_peaks` finds in `s` where `reference` has nothing (below 5 noise SDs, no
/// cross peak within two points).
fn compare_peaks(reference: &SpectrumNd, s: &SpectrumNd) -> PeakMatch {
    let opts = CrossPeakOptions::default();
    let pick_ref =
        nmr::pick_cross_peaks(&reference.real, &reference.shape, &reference.axes, &opts).unwrap();
    let sd = pick_ref.noise_sd;
    let all_ref = pick_ref.peaks;
    let a: Vec<_> = all_ref
        .iter()
        .filter(|p| p.height.abs() >= STRONG_SNR * sd)
        .collect();
    let b = nmr::pick_cross_peaks(&s.real, &s.shape, &s.axes, &opts)
        .unwrap()
        .peaks;
    let near = |p: &[usize], q: &[usize]| p.iter().zip(q).all(|(x, y)| x.abs_diff(*y) <= 2);
    // found: the tallest point of `s` within one point of the apex has the apex's sign and at
    // least half its height
    let (n1, n2) = (reference.shape[0], reference.shape[1]);
    let mut errs = Vec::new();
    for p in &a {
        let (i0, j0) = (p.index[0], p.index[1]);
        let (mut best, mut best_ref) = (0.0f64, 0.0f64);
        for i in i0.saturating_sub(1)..=(i0 + 1).min(n1 - 1) {
            for j in j0.saturating_sub(1)..=(j0 + 1).min(n2 - 1) {
                let sign = p.height.signum();
                best = best.max(s.real[i * n2 + j] * sign);
                best_ref = best_ref.max(reference.real[i * n2 + j] * sign);
            }
        }
        let ratio = best / best_ref;
        if ratio >= 0.5 {
            errs.push((ratio - 1.0).abs());
        }
    }
    let false_peaks = b
        .iter()
        .filter(|q| q.height.abs() >= STRONG_SNR * sd)
        .filter(|q| !all_ref.iter().any(|p| near(&p.index, &q.index)))
        .filter(|q| {
            reference
                .real
                .get(q.index[0] * n2 + q.index[1])
                .is_none_or(|v| v.abs() < 5.0 * sd)
        })
        .count();
    errs.sort_by(f64::total_cmp);
    PeakMatch {
        reference: a.len(),
        found: errs.len(),
        false_peaks,
        median_height_err: errs.get(errs.len() / 2).copied().unwrap_or(f64::NAN),
        max_height_err: errs.last().copied().unwrap_or(f64::NAN),
    }
}

/// Fully sampled experiments of the 2D parity table (≥ 0.999, no linear prediction).
const FULL: &[&str] = &[
    "nmrxiv-s736/3",
    "nmrxiv-s736/4",
    "nmrxiv-s736/5",
    "nmrxiv-s740/2",
    "nmrxiv-s1247/13",
    "nmrxiv-s1517-bundle/16",
    "nmrxiv-s853/8",
    "nmrxiv-s572-bundle/5",
    "nmrxiv-s279/22",
];

#[test]
fn simulated_sampling_of_fully_sampled_sets() {
    let dir = files_dir();
    let mut failures = Vec::new();
    let mut checked = 0;
    println!(
        "| experiment | kept | correlation, reconstructed (zero filled) | cross peaks found | height error median / max | false peaks (zero filled) | reconstruction time |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- |");
    for rel in FULL {
        let path = dir.join(rel);
        if !path.exists() || !listed(rel) {
            continue;
        }
        if let Some(only) = std::env::var_os("OPENREADOUT_ND_ONLY")
            && !rel.contains(only.to_string_lossy().as_ref())
        {
            continue;
        }
        let mut ds = open(&path);
        let info = ds.info().unwrap();
        let fid = info
            .traces
            .iter()
            .find(|t| nmr::source::is_fid(t))
            .unwrap()
            .index;
        let p = nmr::nd_parameters(ds.as_ref(), &info, fid).unwrap();
        let rows = indirect::read_rows(ds.as_mut(), &info, fid).unwrap();
        let opts = NdOptions::default();
        let reference =
            nmr::process_nd(&rows, &p.direct, &p.direct_stored, &p.indirect, &opts).unwrap();
        let d = &p.indirect[0];
        let rpp = d.encoding.rows_per_point();
        let m = d.rows / rpp;
        for keep in [0.25, 0.5] {
            let k = (keep * m as f64).round() as usize;
            let flat = nus::poisson_gap_schedule(m, k, 2026);
            let sched: Vec<Vec<usize>> = flat.iter().map(|&i| vec![i]).collect();
            let measured: Vec<Vec<_>> = flat
                .iter()
                .flat_map(|&i| (0..rpp).map(move |q| i * rpp + q))
                .map(|r| rows[r].clone())
                .collect();
            let grid = indirect::place_on_grid(&measured, &sched, &p.indirect, &[m]).unwrap();
            let zero =
                nmr::process_nd(&grid, &p.direct, &p.direct_stored, &p.indirect, &opts).unwrap();
            let mut filled = grid;
            let t0 = Instant::now();
            let rec = nus::reconstruct(
                &mut filled,
                &sched,
                &p.indirect,
                &[m],
                nus::DEFAULT_ITERATIONS,
                nus::DEFAULT_CONVERGENCE,
            )
            .unwrap();
            let secs = t0.elapsed().as_secs_f64();
            let ist =
                nmr::process_nd(&filled, &p.direct, &p.direct_stored, &p.indirect, &opts).unwrap();
            let (c_ist, c_zero) = (
                signal_corr(&ist.real, &reference.real),
                signal_corr(&zero.real, &reference.real),
            );
            let (pi, pz) = (
                compare_peaks(&reference, &ist),
                compare_peaks(&reference, &zero),
            );
            println!(
                "| `{rel}` ({:?}, {} × {}) | {:.0} % | {c_ist:.4} ({c_zero:.4}) | {}/{} ({}) | {:.3} / {:.3} | {} ({}) | {secs:.1} s, {} of {} columns, ≤ {} iterations |",
                d.encoding,
                m,
                rows.first().map_or(0, Vec::len),
                100.0 * keep,
                pi.found,
                pi.reference,
                pz.found,
                pi.median_height_err,
                pi.max_height_err,
                pi.false_peaks,
                pz.false_peaks,
                rec.columns_reconstructed,
                rec.columns,
                rec.iterations_used,
            );
            checked += 1;
            let need = if keep < 0.4 { 0.97 } else { 0.99 };
            if !(c_ist >= need) || !(c_ist > c_zero) {
                failures.push(format!(
                    "{rel} at {keep}: correlation {c_ist:.4} (zero filled {c_zero:.4}, need {need})"
                ));
            }
            // false cross peaks are asserted at 50 % (at 25 % magnitude COSYs keep some)
            let false_limit = (pi.reference / 10).max(2);
            // strong cross peaks found: crowded NOESYs and magnitude COSYs lose weak lines at 25 %
            let recall = if keep < 0.4 { 0.6 } else { 0.85 };
            if (pi.found as f64) < recall * pi.reference as f64
                || (keep > 0.4 && pi.false_peaks > false_limit)
            {
                failures.push(format!(
                    "{rel} at {keep}: {}/{} cross peaks, {} false (zero filled {})",
                    pi.found, pi.reference, pi.false_peaks, pz.false_peaks
                ));
            }
        }
    }
    println!("{checked} simulated schedules");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// TopSpin's `pdata/1/2rr` of an experiment, row-major, with its shape.
fn stored_2rr(ds: &mut dyn Dataset) -> Option<(Vec<f64>, usize, usize)> {
    let info = ds.info().ok()?;
    let pt = info
        .traces
        .iter()
        .find(|t| t.name.as_deref() == Some("pdata/1"))?;
    let ch = pt
        .channels
        .iter()
        .position(|c| c.name == "2rr")
        .unwrap_or(0);
    let mut v = Vec::new();
    for s in 0..pt.sweep_count {
        let t = ds.read_trace(pt.index, s, 0, pt.sample_count).ok()?;
        v.extend_from_slice(&t.channels[ch]);
    }
    Some((v, pt.sweep_count as usize, pt.sample_count as usize))
}

/// `s` cut to the common grid with `n1 × n2` (both row-major).
fn common(s: &SpectrumNd, theirs: &[f64], n1s: usize, n2s: usize) -> (Vec<f64>, Vec<f64>) {
    let (n1, n2) = (s.shape[0].min(n1s), s.shape[1].min(n2s));
    let (mut a, mut b) = (Vec::new(), Vec::new());
    for i in 0..n1 {
        a.extend_from_slice(&s.real[i * s.shape[1]..i * s.shape[1] + n2]);
        b.extend_from_slice(&theirs[i * n2s..i * n2s + n2]);
    }
    (a, b)
}

const MEASURED: &[&str] = &[
    "nmrxiv-s279/23",
    "nmrxiv-s279/24",
    "nmrxiv-s753/21",
    "nmrxiv-s853/5",
    "nmrxiv-s853/6",
    "nmrxiv-s853/7",
    "nmrxiv-s1407-bundle/HSQC",
    "nmrxiv-s1407-bundle/HMBC",
    "nmrxiv-s1517-bundle/13",
    "nmrxiv-s1517-bundle/14",
    "nmrxiv-s1517-bundle/15",
];

#[test]
fn measured_nus_sets_against_topspin() {
    let dir = files_dir();
    let mut failures = Vec::new();
    println!(
        "| experiment | sampled | correlation with TopSpin, reconstructed (zero filled) | load time, reconstructed (zero filled) |"
    );
    println!("| --- | --- | --- | --- |");
    for rel in MEASURED {
        let path = dir.join(rel);
        if !path.exists() || !listed(rel) {
            continue;
        }
        let mut ds = open(&path);
        let info = ds.info().unwrap();
        let fid = info
            .traces
            .iter()
            .find(|t| nmr::source::is_fid(t))
            .unwrap()
            .index;
        let Some((theirs, n1s, n2s)) = stored_2rr(ds.as_mut()) else {
            continue;
        };
        let t0 = Instant::now();
        let ist = nmr::load_processed_nd(ds.as_mut(), &info, fid, &NdOptions::default()).unwrap();
        let t_ist = t0.elapsed().as_secs_f64();
        let zf_opts = NdOptions {
            nus: NusMode::ZeroFill,
            ..NdOptions::default()
        };
        let t0 = Instant::now();
        let zero = nmr::load_processed_nd(ds.as_mut(), &info, fid, &zf_opts).unwrap();
        let t_zero = t0.elapsed().as_secs_f64();
        let (a, b) = common(&ist, &theirs, n1s, n2s);
        let (z, _) = common(&zero, &theirs, n1s, n2s);
        let (c_ist, c_zero) = (signal_corr(&a, &b), signal_corr(&z, &b));
        let rec = ist.record.nus.clone().unwrap_or_default();
        println!(
            "| `{rel}` ({:?}) | {} of {} ({:.0} %) | {c_ist:.4} ({c_zero:.4}) | {t_ist:.1} s ({t_zero:.1} s) |",
            ist.record.dimensions[0].encoding,
            rec.sampled_points,
            rec.grid_points,
            100.0 * rec.sampled_fraction,
        );
        if rec.method != "iterative_soft_threshold" {
            failures.push(format!("{rel}: not reconstructed ({rec:?})"));
        }
        if !(c_ist > c_zero) {
            failures.push(format!(
                "{rel}: reconstructed {c_ist:.4}, zero filled {c_zero:.4}"
            ));
        }
    }
    // the MQMAS pair: the NUS acquisition against its fully sampled twin, both processed here
    let (nus_rel, full_rel) = (
        "desperate/RbNO3_MQMAS_Nov9_2021/30",
        "desperate/RbNO3_MQMAS_Nov9_2021/20",
    );
    let (np, fp) = (dir.join(nus_rel), dir.join(full_rel));
    if np.exists() && fp.exists() && listed(nus_rel) {
        let process = |p: &Path, o: &NdOptions| {
            let mut ds = open(p);
            let info = ds.info().unwrap();
            let fid = info
                .traces
                .iter()
                .find(|t| nmr::source::is_fid(t))
                .unwrap()
                .index;
            nmr::load_processed_nd(ds.as_mut(), &info, fid, o).unwrap()
        };
        let full = process(&fp, &NdOptions::default());
        let ist = process(&np, &NdOptions::default());
        let zero = process(
            &np,
            &NdOptions {
                nus: NusMode::ZeroFill,
                ..NdOptions::default()
            },
        );
        let n = full.real.len().min(ist.real.len());
        let (c_ist, c_zero) = (
            signal_corr(&ist.real[..n], &full.real[..n]),
            signal_corr(&zero.real[..n], &full.real[..n]),
        );
        let rec = ist.record.nus.clone().unwrap_or_default();
        println!(
            "MQMAS {nus_rel} ({} of {} increments) against the fully sampled {full_rel}: reconstructed {c_ist:.4}, zero filled {c_zero:.4}",
            rec.sampled_points, rec.grid_points
        );
        if !(c_ist > c_zero) {
            failures.push(format!(
                "{nus_rel}: reconstructed {c_ist:.4}, zero filled {c_zero:.4}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
