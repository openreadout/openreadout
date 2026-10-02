//! Quantitation against independent ground truth (`corpus/oracle/quant/*.json`, written by
//! `oracle/quant.py`; method and numbers in `book/src/guides/quantitation.md`):
//!
//! - `vendor_integrations`: our automatic peaks and our integration with the vendor's own
//!   boundaries and baseline, against the peak tables the vendor data systems computed for the
//!   same injections (Agilent ChemStation, Shimadzu LabSolutions, MSD ChemStation), and our
//!   integration arithmetic against pyOpenMS `PeakIntegrator` on the same boundaries.
//! - `xic_against_depositor_mzml`: TIC, BPC and ±10 ppm XICs extracted from vendor raw files
//!   against the same chromatograms computed with pyteomics from the depositors' mzML, point by
//!   point; and our XIC peaks against pyOpenMS `PeakPickerChromatogram`.
//! - `srm_against_depositor_chromatograms`: `--transition Q1>Q3` chromatograms from raw SRM/MRM
//!   files against the depositors' mzML SRM chromatograms (the `chromatograms` of the existing
//!   corpus oracles).
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test quant -- --nocapture`
//! Env: `OPENREADOUT_CORPUS_DIR` overrides `corpus/files`; `CORPUS_ONLY=<substring>` filters ids.
#![cfg(feature = "corpus")]
#![allow(clippy::many_single_char_names, clippy::float_cmp)]

use std::path::{Path, PathBuf};

use openreadout_core::parallel::ReadContext;
use openreadout_core::{Dataset, Registry};
use openreadout_corpus_tests::oracle_json;
use openreadout_quant::extract::{ChromRequest, Chromatogram, Target, Tolerance, extract};
use openreadout_quant::peaks::{AreaTimeUnit, Peak, PeakParams, find_peaks, integrate_range};
use serde_json::Value;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn corpus_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root().join("corpus/files"), PathBuf::from)
}

fn only(id: &str) -> bool {
    std::env::var("CORPUS_ONLY").map_or(true, |o| id.contains(&o))
}

fn registry() -> Registry {
    Registry::new()
        .with(Box::new(openreadout_thermo::ThermoRawReader))
        .with(Box::new(openreadout_agilent_ms::AgilentMsReader))
        .with(Box::new(openreadout_sciex::SciexWiffReader))
        .with(Box::new(openreadout_chrom::ChemStationReader))
        .with(Box::new(openreadout_chrom::AndiReader))
        .with(Box::new(openreadout_waters::WatersRawReader))
        .with(Box::new(openreadout_mzml::MzmlReader))
        .with(Box::new(openreadout_mzml::MzxmlReader))
}

fn oracles(suffix_xic: bool) -> Vec<(String, Value)> {
    let dir = root().join("corpus/oracle/quant");
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return out;
    };
    let mut paths: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for p in paths {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let is_json = p
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"));
        if name.ends_with(".xic.json") != suffix_xic || !is_json {
            continue;
        }
        let id = name
            .trim_end_matches(".json")
            .trim_end_matches(".xic")
            .to_string();
        if !only(&id) {
            continue;
        }
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        out.push((id, v));
    }
    out
}

fn open(reg: &Registry, path: &Path) -> Option<Box<dyn Dataset>> {
    if !path.exists() {
        eprintln!("skip (missing): {}", path.display());
        return None;
    }
    Some(
        reg.open(path)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
            .1,
    )
}

fn chromatogram(
    ds: &mut dyn Dataset,
    targets: Vec<Target>,
    tol: Option<Tolerance>,
) -> Vec<Chromatogram> {
    chromatogram_view(ds, targets, tol, false)
}

fn chromatogram_view(
    ds: &mut dyn Dataset,
    targets: Vec<Target>,
    tol: Option<Tolerance>,
    profile: bool,
) -> Vec<Chromatogram> {
    let info = ds.info().unwrap();
    let mut req = ChromRequest::default();
    req.targets = targets;
    req.profile = profile;
    if let Some(t) = tol {
        req.tolerance = t;
    }
    extract(ds, &info, &req, &ReadContext::default())
        .unwrap()
        .chromatograms
}

fn f(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64)
}

/// Median, 95th percentile and maximum of `v` (absolute values).
fn stats(v: &[f64]) -> (f64, f64, f64) {
    if v.is_empty() {
        return (f64::NAN, f64::NAN, f64::NAN);
    }
    let mut a: Vec<f64> = v.iter().map(|x| x.abs()).collect();
    a.sort_by(f64::total_cmp);
    let q = |p: f64| a[((a.len() - 1) as f64 * p).round() as usize];
    (q(0.5), q(0.95), a[a.len() - 1])
}

#[derive(Default)]
struct Tally {
    vendor_peaks: usize,
    matched: usize,
    rt_diff_min: Vec<f64>,
    area_rel: Vec<f64>,
    area_rel_valley: Vec<f64>,
    area_pct_diff: Vec<f64>,
    height_rel: Vec<f64>,
    forced_area_rel: Vec<f64>,
    pyopenms_forced_rel: Vec<f64>,
    pyopenms_width50_rel: Vec<f64>,
    pyopenms_pick_rt: Vec<f64>,
    pyopenms_pick_area_rel: Vec<f64>,
    worst: Vec<(f64, String)>,
    /// per vendor data set: (automatic default, automatic valley, vendor limits+baseline) area ratios − 1
    family: std::collections::BTreeMap<String, [Vec<f64>; 3]>,
}

/// The vendor data set a corpus id belongs to.
fn family(id: &str) -> String {
    if id.starts_with("mtbls1892-") {
        "Shimadzu LabSolutions (ANDI, MTBLS1892)".into()
    } else if id.starts_with("cheminfo-") {
        "Agilent ChemStation (ANDI, HPLC-DAD)".into()
    } else if id.starts_with("chromhandler-001") {
        "Agilent ChemStation (Report.TXT, GC FID/TCD)".into()
    } else if id.starts_with("gc2asm-") {
        "Agilent ChemStation (Result.xml, GC FID/TCD)".into()
    } else {
        "MSD ChemStation (RESULTS.CSV, GC-MS TIC)".into()
    }
}

/// Our peaks whose apex lies inside `[start, end]` (or within `tol` of `rt` without bounds).
fn matched<'a>(ours: &'a [Peak], vp: &Value) -> Vec<&'a Peak> {
    let rt = f(vp, "rt_min").unwrap();
    if let (Some(a), Some(b)) = (f(vp, "start_min"), f(vp, "end_min")) {
        let inside: Vec<&Peak> = ours
            .iter()
            .filter(|p| p.rt_min >= a && p.rt_min <= b)
            .collect();
        if inside.is_empty() {
            return ours
                .iter()
                .filter(|p| (p.rt_min - rt).abs() <= 0.02)
                .collect();
        }
        return inside;
    }
    let w = f(vp, "width_min").unwrap_or(0.05).max(0.02);
    ours.iter()
        .filter(|p| (p.rt_min - rt).abs() <= w)
        .min_by(|a, b| (a.rt_min - rt).abs().total_cmp(&(b.rt_min - rt).abs()))
        .into_iter()
        .collect()
}

fn compare_signal(
    tally: &mut Tally,
    id: &str,
    c: &Chromatogram,
    vendor_peaks: &[Value],
    area_to_signal_s: Option<f64>,
    height_scale: f64,
) -> Vec<Peak> {
    let mut p = PeakParams::default();
    p.area_time_unit = AreaTimeUnit::S;
    let table = find_peaks(&c.rt_min, &c.intensity, &p).unwrap();
    let mut pv = p.clone();
    pv.baseline = openreadout_quant::peaks::BaselineMode::Valley;
    let valley = find_peaks(&c.rt_min, &c.intensity, &pv).unwrap();
    let total_vendor: f64 = vendor_peaks.iter().filter_map(|v| f(v, "area")).sum();
    let mut ours_matched_total = 0.0;
    let mut pairs: Vec<(f64, f64)> = Vec::new();
    for vp in vendor_peaks {
        tally.vendor_peaks += 1;
        let m = matched(&table.peaks, vp);
        if m.is_empty() {
            tally.worst.push((
                1.0,
                format!(
                    "{id} {}: vendor peak at {:.3} min not found",
                    c.label,
                    f(vp, "rt_min").unwrap()
                ),
            ));
            continue;
        }
        tally.matched += 1;
        let top = m
            .iter()
            .max_by(|a, b| a.height.total_cmp(&b.height))
            .unwrap();
        let area: f64 = m.iter().map(|q| q.area).sum();
        let rt_d = top.rt_min - f(vp, "rt_min").unwrap();
        tally.rt_diff_min.push(rt_d);
        let va = f(vp, "area").unwrap();
        ours_matched_total += area;
        pairs.push((area, va));
        if let Some(k) = area_to_signal_s {
            let mv = matched(&valley.peaks, vp);
            let rv =
                (!mv.is_empty()).then(|| mv.iter().map(|q| q.area).sum::<f64>() / (va * k) - 1.0);
            let rel = area / (va * k) - 1.0;
            if let Some(rv) = rv {
                tally.area_rel_valley.push(rv);
            }
            let fam = tally.family.entry(family(id)).or_default();
            fam[0].push(rel);
            fam[1].extend(rv);
            tally.area_rel.push(rel);
            tally.worst.push((
                rel.abs(),
                format!(
                    "{id} {} {:.3} min: area {area:.6} vs vendor {:.6} ({:+.2} %)",
                    c.label,
                    f(vp, "rt_min").unwrap(),
                    va * k,
                    100.0 * rel
                ),
            ));
        }
        if let Some(vh) = f(vp, "height") {
            let h = m.iter().map(|q| q.height).fold(f64::NEG_INFINITY, f64::max);
            tally.height_rel.push(h / (vh / height_scale) - 1.0);
        }
        // forced: the vendor's own boundaries and baseline values, no smoothing
        if let (Some(a), Some(b), Some(k)) =
            (f(vp, "start_min"), f(vp, "end_min"), area_to_signal_s)
        {
            let mut fp = PeakParams::default();
            fp.smooth = Some(0);
            fp.area_time_unit = AreaTimeUnit::S;
            let base = match (f(vp, "baseline_start"), f(vp, "baseline_end")) {
                (Some(x), Some(y)) => Some((x, y)),
                _ => None,
            };
            if let Some(base) = base
                && let Ok(pk) = integrate_range(&c.rt_min, &c.intensity, a, b, Some(base), &fp)
            {
                let rel = pk.area / (va * k) - 1.0;
                tally.forced_area_rel.push(rel);
                tally.family.entry(family(id)).or_default()[2].push(rel);
                if std::env::var_os("QUANT_DEBUG").is_some() && rel.abs() > 0.02 {
                    println!(
                        "    forced {id} {} {:.3}-{:.3}: ours {:.4} vendor {:.4} ({:+.2} %)",
                        c.label,
                        a,
                        b,
                        pk.area,
                        va * k,
                        100.0 * rel
                    );
                }
            }
        }
    }
    // unit-free comparison: area % among the vendor's peaks
    if ours_matched_total > 0.0 && total_vendor > 0.0 {
        for (ours, v) in pairs {
            tally
                .area_pct_diff
                .push(100.0 * (ours / ours_matched_total - v / total_vendor));
        }
    }
    table.peaks
}

#[test]
fn vendor_integrations() {
    let reg = registry();
    let files = corpus_dir();
    let mut t = Tally::default();
    let mut n_files = 0;
    for (id, o) in oracles(false) {
        let input = files.join(o["input"].as_str().unwrap());
        let Some(mut ds) = open(&reg, &input) else {
            continue;
        };
        n_files += 1;
        let k = o["vendor"]["area_to_signal_s"].as_f64();
        let scale = o["vendor"]["height_scale"].as_f64().unwrap_or(1.0);
        if let Some(peaks) = o.get("peaks").and_then(Value::as_array) {
            let target = if o["signal"].get("tic").is_some() {
                Target::Tic
            } else {
                Target::Trace {
                    trace: o["signal"]["trace"].as_u64().unwrap_or(0) as u32,
                    channel: None,
                }
            };
            let c = chromatogram(ds.as_mut(), vec![target], None).remove(0);
            let ours = compare_signal(&mut t, &id, &c, peaks, k, scale);
            // pyOpenMS PeakIntegrator on the same samples and boundaries: identical arithmetic
            if let Some(forced) = o.get("pyopenms_forced").and_then(Value::as_array) {
                for fo in forced.iter().filter(|x| !x.is_null()) {
                    let (a, b) = (
                        f(fo, "left_s").unwrap() / 60.0,
                        f(fo, "right_s").unwrap() / 60.0,
                    );
                    let mut fp = PeakParams::default();
                    fp.smooth = Some(0);
                    fp.area_time_unit = AreaTimeUnit::S;
                    let pk = integrate_range(&c.rt_min, &c.intensity, a, b, None, &fp).unwrap();
                    let theirs = f(fo, "area").unwrap();
                    t.pyopenms_forced_rel.push(pk.area / theirs - 1.0);
                    // pyOpenMS reports widths between samples (no interpolation): compare in
                    // sampling intervals
                    let dt_s = 60.0 * (c.rt_min[c.rt_min.len() - 1] - c.rt_min[0])
                        / (c.rt_min.len() - 1) as f64;
                    if let (Some(w), Some(tw)) = (pk.width_half_min, f(fo, "width_at_50_s")) {
                        t.pyopenms_width50_rel.push((w * 60.0 - tw) / dt_s);
                    }
                }
            }
            if let Some(picked) = o.get("pyopenms_picked").and_then(Value::as_array) {
                for pp in picked {
                    let (a, b) = (
                        f(pp, "left_s").unwrap() / 60.0,
                        f(pp, "right_s").unwrap() / 60.0,
                    );
                    let apex = f(pp, "apex_s").unwrap() / 60.0;
                    let Some(tr) = pp.get("trapezoid") else {
                        continue;
                    };
                    let Some(theirs) = f(tr, "area") else {
                        continue;
                    };
                    // only well-defined peaks: at least 1 % of the largest pyOpenMS area
                    let max_area = picked
                        .iter()
                        .filter_map(|x| x.get("trapezoid").and_then(|t| f(t, "area")))
                        .fold(0.0, f64::max);
                    if theirs < 0.01 * max_area {
                        continue;
                    }
                    let inside: Vec<&Peak> = ours
                        .iter()
                        .filter(|q| q.rt_min >= a && q.rt_min <= b)
                        .collect();
                    if let Some(q) = inside
                        .iter()
                        .min_by(|x, y| (x.rt_min - apex).abs().total_cmp(&(y.rt_min - apex).abs()))
                    {
                        t.pyopenms_pick_rt.push(q.rt_min - apex);
                        let area: f64 = inside.iter().map(|x| x.area).sum();
                        t.pyopenms_pick_area_rel.push(area / theirs - 1.0);
                    }
                }
            }
        }
        if let Some(signals) = o.get("signals").and_then(Value::as_array) {
            let info = ds.info().unwrap();
            for s in signals {
                let name = s["trace_name"].as_str().unwrap();
                let Some(tr) = info.traces.iter().find(|x| {
                    x.name
                        .as_deref()
                        .is_some_and(|n| n.split([',', ' ']).next() == Some(name))
                }) else {
                    panic!("{id}: no trace named {name}");
                };
                let c = chromatogram(
                    ds.as_mut(),
                    vec![Target::Trace {
                        trace: tr.index,
                        channel: None,
                    }],
                    None,
                )
                .remove(0);
                compare_signal(&mut t, &id, &c, s["peaks"].as_array().unwrap(), k, 1.0);
            }
        }
    }
    if n_files == 0 {
        eprintln!("no vendor-integration files present; skipped");
        return;
    }
    let pr = |name: &str, v: &[f64], scale: f64, unit: &str| {
        let (m, p95, mx) = stats(v);
        println!(
            "  {name:<44} n={:<4} median {:.4}{unit}  p95 {:.4}{unit}  max {:.4}{unit}",
            v.len(),
            m * scale,
            p95 * scale,
            mx * scale
        );
    };
    println!(
        "vendor integrations: {n_files} files, {} vendor peaks, {} matched by our automatic integration",
        t.vendor_peaks, t.matched
    );
    pr(
        "retention time |ours − vendor|",
        &t.rt_diff_min,
        1.0,
        " min",
    );
    pr(
        "area |ours/vendor − 1| (automatic, default auto)",
        &t.area_rel,
        100.0,
        " %",
    );
    pr(
        "area |ours/vendor − 1| (automatic, valley)",
        &t.area_rel_valley,
        100.0,
        " %",
    );
    pr(
        "area % |ours − vendor| (unit-free)",
        &t.area_pct_diff,
        1.0,
        " pts",
    );
    pr("height |ours/vendor − 1|", &t.height_rel, 100.0, " %");
    pr(
        "area |ours/vendor − 1| (vendor boundaries+baseline)",
        &t.forced_area_rel,
        100.0,
        " %",
    );
    pr(
        "area |ours/pyOpenMS − 1| (same boundaries)",
        &t.pyopenms_forced_rel,
        100.0,
        " %",
    );
    pr(
        "FWHM |ours − pyOpenMS| (same boundaries)",
        &t.pyopenms_width50_rel,
        1.0,
        " samples",
    );
    pr(
        "apex |ours − pyOpenMS picker|",
        &t.pyopenms_pick_rt,
        1.0,
        " min",
    );
    pr(
        "area |ours/pyOpenMS picker − 1|",
        &t.pyopenms_pick_area_rel,
        100.0,
        " %",
    );
    println!(
        "  area |ours/vendor − 1| per vendor data set (median / p95), automatic (default) · automatic valley · vendor limits+baseline:"
    );
    for (fam, [d, v, f]) in &t.family {
        let s = |x: &[f64]| {
            let (m, p, _) = stats(x);
            format!("{:>5.1} / {:>5.1} % (n={})", 100.0 * m, 100.0 * p, x.len())
        };
        println!("    {fam:<46} {}  ·  {}  ·  {}", s(d), s(v), s(f));
    }
    t.worst.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (_, w) in t.worst.iter().take(20) {
        println!("    worst: {w}");
    }
    // Tolerances (book/src/guides/quantitation.md → Validation). Integration arithmetic is exact; the
    // automatic integration is compared with the vendor's on peaks a chromatographer would
    // report, where the differences come from where each method puts the baseline.
    let (_, _, mx) = stats(&t.pyopenms_forced_rel);
    assert!(
        mx < 1e-4,
        "trapezoid area differs from pyOpenMS PeakIntegrator by {mx}"
    );
    let (m, _, _) = stats(&t.pyopenms_width50_rel);
    assert!(
        m < 2.0,
        "widths at half height differ from pyOpenMS by {m} samples (median)"
    );
    let (m, _, _) = stats(&t.pyopenms_pick_rt);
    assert!(
        m < 0.01,
        "apexes differ from pyOpenMS PeakPickerChromatogram by {m} min (median)"
    );
    let (m, _, _) = stats(&t.rt_diff_min);
    assert!(m < 0.005, "median retention-time difference {m} min");
    assert!(
        t.matched * 100 >= t.vendor_peaks * 97,
        "only {}/{} vendor peaks found",
        t.matched,
        t.vendor_peaks
    );
    let (m, _, _) = stats(&t.area_rel);
    assert!(m < 0.05, "automatic areas: median {m}");
    let (_, p95, mx) = stats(&t.area_pct_diff);
    assert!(
        p95 < 3.0 && mx < 5.0,
        "area % differs by p95 {p95}, max {mx} points"
    );
    let (m, p95, _) = stats(&t.forced_area_rel);
    assert!(
        m < 0.02 && p95 < 0.08,
        "areas with the vendor's limits and baseline: median {m}, p95 {p95}"
    );
}

#[test]
fn xic_against_depositor_mzml() {
    let reg = registry();
    let files = corpus_dir();
    let mut any = false;
    let mut failures: Vec<String> = Vec::new();
    for (id, o) in oracles(true) {
        let input = files.join(o["input"].as_str().unwrap());
        let Some(mut ds) = open(&reg, &input) else {
            continue;
        };
        // An export made by a converter that left flagged Thermo peaks out is compared with
        // them left out (the corpus oracle of the same id records the converter).
        let software: Vec<Vec<String>> =
            oracle_json::read_to_string(&root().join(format!("corpus/oracle/{id}.json")))
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .and_then(|v| serde_json::from_value(v["export_software"].clone()).ok())
                .unwrap_or_default();
        if openreadout_corpus_tests::export_excludes_flagged_peaks(&software)
            && ds
                .info()
                .is_ok_and(|i| i.format.id == openreadout_thermo::FORMAT_ID)
        {
            let mut t = openreadout_thermo::dataset::ThermoDataset::open(&input).unwrap();
            t.set_exclude_flagged_peaks(true);
            ds = Box::new(t);
        }
        any = true;
        let rt: Vec<f64> = o["rt_min"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let mut targets = vec![Target::Tic, Target::Bpc];
        let xics = o["xics"].as_array().unwrap();
        for x in xics {
            targets.push(Target::Xic {
                mz: x["mz"].as_f64().unwrap(),
            });
        }
        let t0 = std::time::Instant::now();
        // the same representation as the export: its profiles, or the stored centroids
        let profile = o["centroided"].as_bool() == Some(false);
        let ours = chromatogram_view(ds.as_mut(), targets, Some(Tolerance::Ppm(10.0)), profile);
        let dt = t0.elapsed().as_secs_f64();
        let oracle_series: Vec<Vec<f64>> = std::iter::once(o["tic"].clone())
            .chain(std::iter::once(o["bpc"].clone()))
            .chain(xics.iter().map(|x| x["intensity"].clone()))
            .map(|a| {
                a.as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_f64().unwrap_or(f64::NAN))
                    .collect()
            })
            .collect();
        let mut line = format!("{id}: {} MS1 scans ({:.2} s)", rt.len(), dt);
        let mut exact_all = true;
        for (k, (c, want)) in ours.iter().zip(&oracle_series).enumerate() {
            // align by retention time (the mzML stores times rounded differently)
            let mut rel = Vec::new();
            let mut missing = 0;
            let mut j = 0;
            for (i, &r) in rt.iter().enumerate() {
                while j + 1 < c.rt_min.len()
                    && (c.rt_min[j + 1] - r).abs() < (c.rt_min[j] - r).abs()
                {
                    j += 1;
                }
                if c.rt_min.is_empty() || (c.rt_min[j] - r).abs() > 1e-4 {
                    missing += 1;
                    continue;
                }
                let (a, b) = (c.intensity[j], want[i]);
                if b.is_nan() {
                    continue;
                }
                let scale = b
                    .abs()
                    .max(1e-9 * want.iter().fold(0.0f64, |m, v| m.max(v.abs())));
                rel.push(if scale > 0.0 { (a - b) / scale } else { a - b });
            }
            // XICs sum the same points: 1e-6. TIC/BPC come from each file's recorded per-scan values
            // (the export may recompute them from float32 arrays): 1e-3.
            // BPC: the file's recorded base peak (Waters: from the scan statistics) against the
            // export's value: 1e-2.
            let tol = match k {
                0 => 1e-3,
                1 => 1e-2,
                _ => 1e-6,
            };
            let within = rel.iter().filter(|v| v.abs() <= tol).count();
            let frac = within as f64 / rel.len().max(1) as f64;
            let (med, p95, mx) = stats(&rel);
            let name = match k {
                0 => "TIC".to_string(),
                1 => "BPC".to_string(),
                _ => format!("XIC {}", xics[k - 2]["mz"]),
            };
            line.push_str(&format!(
                "\n    {name:<14} points {}/{} (ours {}), {:.2} % within {tol:.0e} rel, median {med:.1e} p95 {p95:.1e} max {mx:.1e}",
                rel.len(),
                rt.len(),
                c.rt_min.len(),
                100.0 * frac
            ));
            if missing > 0 {
                failures.push(format!(
                    "{id} {name}: {missing} oracle scans without a matching point"
                ));
            }
            if frac < 0.999 {
                exact_all = false;
            }
            // a centroided (vendor peak-picked) export must match our stored centroids exactly;
            // profile exports within float rounding
            let limit = if o["centroided"].as_bool() == Some(true) {
                0.999
            } else {
                0.99
            };
            // a profile export's base-peak value is the tallest profile point, ours the recorded
            // (centroid) base peak: reported, not compared
            if frac < limit && !(k == 1 && profile) {
                failures.push(format!("{id} {name}: only {frac:.3} of the points agree"));
            }
        }
        // peaks on the XICs against pyOpenMS PeakPickerChromatogram
        let mut rt_d = Vec::new();
        let mut area_rel = Vec::new();
        for (x, c) in xics.iter().zip(ours.iter().skip(2)) {
            let table = find_peaks(&c.rt_min, &c.intensity, &PeakParams::default()).unwrap();
            let picked = x["pyopenms_picked"].as_array().unwrap();
            let max_area = picked
                .iter()
                .filter_map(|p| p.get("trapezoid").and_then(|t| f(t, "area")))
                .fold(0.0, f64::max);
            for p in picked {
                let Some(theirs) = p.get("trapezoid").and_then(|t| f(t, "area")) else {
                    continue;
                };
                if theirs < 0.05 * max_area {
                    continue;
                }
                let (a, b, apex) = (
                    f(p, "left_s").unwrap() / 60.0,
                    f(p, "right_s").unwrap() / 60.0,
                    f(p, "apex_s").unwrap() / 60.0,
                );
                let inside: Vec<&Peak> = table
                    .peaks
                    .iter()
                    .filter(|q| q.rt_min >= a && q.rt_min <= b)
                    .collect();
                if let Some(q) = inside.iter().max_by(|x, y| x.height.total_cmp(&y.height)) {
                    rt_d.push(q.rt_min - apex);
                    area_rel.push(inside.iter().map(|x| x.area).sum::<f64>() * 60.0 / theirs - 1.0);
                }
            }
        }
        let (m1, p1, _) = stats(&rt_d);
        let (m2, p2, _) = stats(&area_rel);
        line.push_str(&format!(
            "\n    XIC peaks vs pyOpenMS picker: {} matched, apex median {m1:.4} p95 {p1:.4} min, area median {:.1} % p95 {:.1} %",
            rt_d.len(),
            100.0 * m2,
            100.0 * p2
        ));
        println!("{line}");
        let _ = exact_all;
    }
    if !any {
        eprintln!("no XIC oracle inputs present; skipped");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn srm_against_depositor_chromatograms() {
    let reg = registry();
    let files = corpus_dir();
    let manifest: toml::Value =
        toml::from_str(&std::fs::read_to_string(root().join("corpus/manifest.toml")).unwrap())
            .unwrap();
    let entries = manifest["file"].as_array().unwrap();
    let mut checked = 0;
    for id in [
        "mtbls1822-tsq-74",
        "mtbls449-13047CHQ_0001_A1",
        "mtbls6084-sl-st-blank2",
    ] {
        if !only(id) {
            continue;
        }
        let oracle: Value =
            match oracle_json::read_to_string(&root().join(format!("corpus/oracle/{id}.json"))) {
                Ok(s) => serde_json::from_str(&s).unwrap(),
                Err(_) => continue,
            };
        let Some(input) = entries
            .iter()
            .find(|e| e["id"].as_str() == Some(id) && e["role"].as_str() == Some("input"))
        else {
            continue;
        };
        let path = files.join(input["filename"].as_str().unwrap());
        let Some(mut ds) = open(&reg, &path) else {
            continue;
        };
        let chroms = oracle["chromatograms"].as_array().unwrap();
        // transitions that occur once (scheduled windows of a repeated Q1/Q3 are separate
        // traces in the export but one transition here)
        let srm: Vec<&Value> = chroms.iter().filter(|c| c["kind"] == "srm").collect();
        let unique: Vec<&Value> = srm
            .iter()
            .copied()
            .filter(|c| {
                srm.iter()
                    .filter(|d| {
                        d["precursor_mz"] == c["precursor_mz"] && d["product_mz"] == c["product_mz"]
                    })
                    .count()
                    == 1
            })
            .collect();
        let targets: Vec<Target> = unique
            .iter()
            .map(|c| Target::Srm {
                q1: c["precursor_mz"].as_f64().unwrap(),
                q3: c["product_mz"].as_f64().unwrap(),
            })
            .collect();
        let t0 = std::time::Instant::now();
        let ours = chromatogram(ds.as_mut(), targets, None);
        let dt = t0.elapsed().as_secs_f64();
        let (mut exact, mut sums) = (0, Vec::new());
        for (c, o) in ours.iter().zip(&unique) {
            let nz: Vec<f64> = c.intensity.iter().copied().filter(|v| *v != 0.0).collect();
            let sum: f64 = nz.iter().sum();
            let want = o["sum_intensity"].as_f64().unwrap();
            let rel = if want == 0.0 { sum } else { sum / want - 1.0 };
            sums.push(rel);
            if nz.len() as u64 == o["n_nonzero"].as_u64().unwrap() && rel.abs() < 1e-6 {
                exact += 1;
            }
        }
        let (m, _, mx) = stats(&sums);
        println!(
            "{id}: {}/{} SRM transitions identical to the depositor's chromatograms (non-zero points and intensity sum; median |Δsum| {m:.1e}, max {mx:.1e}; {:.2} s)",
            exact,
            unique.len(),
            dt
        );
        assert!(
            exact * 100 >= unique.len() * 95,
            "{id}: {exact}/{} transitions match",
            unique.len()
        );
        checked += 1;
    }
    if checked == 0 {
        eprintln!("no SRM inputs present; skipped");
    }
}
