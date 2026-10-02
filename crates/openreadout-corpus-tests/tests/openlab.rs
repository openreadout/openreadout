//! Agilent OpenLab CDS injections against the vendor's own results (`corpus/oracle/openlab/*.json`,
//! written by `oracle/openlab_cds.py vendor` from allotropy's reading of the `.rx` packages and the
//! vendor's `_1.csv` exports; numbers in `docs/formats/openlab-cds.md` → Validation):
//!
//! - `vendor_peak_tables`: our `vendor_peaks` table equals the oracle's reading of every `.rx`
//!   (every value bit for bit, the signal each peak belongs to, which checks the signal matching)
//!   and, for the Polyarc GC-FID set, the vendor CSV export to its printed precision, together
//!   with the injection metadata the CSV prints (sample, operator, vial, volume, method).
//! - `integration_with_vendor_limits`: our decoded signal integrated between the vendor's peak
//!   limits above the vendor's baseline gives the vendor's areas and heights (the decode, the
//!   time axis and the unit of the area are right).
//! - `automatic_peaks_against_vendor`: `openreadout analyze peaks`' automatic integration of the
//!   same signals against the vendor's peak tables (`drop`, `valley` and the default `auto`;
//!   detection, retention times and the `valley`/`auto` areas asserted).
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test openlab -- --nocapture`
//! Env: `OPENREADOUT_CORPUS_DIR` overrides `corpus/files`; `CORPUS_ONLY=<substring>` filters ids.
#![cfg(feature = "corpus")]
#![allow(clippy::float_cmp)]

use std::path::{Path, PathBuf};

use openreadout_core::parallel::ReadContext;
use openreadout_core::{Dataset, Registry};
use openreadout_quant::extract::{ChromRequest, Chromatogram, Target, extract};
use openreadout_quant::peaks::{
    AreaTimeUnit, BaselineMode, PeakParams, find_peaks, integrate_range,
};
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

fn registry() -> Registry {
    Registry::new().with(Box::new(openreadout_chrom::OpenLabReader))
}

/// Every vendor oracle whose input is present: (id, oracle, opened data set).
fn cases() -> Vec<(String, Value, Box<dyn Dataset>)> {
    let dir = root().join("corpus/oracle/openlab");
    let only = std::env::var("CORPUS_ONLY").ok();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    paths.sort();
    let reg = registry();
    let mut out = Vec::new();
    for p in paths {
        let o: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let id = o["id"].as_str().unwrap().to_string();
        if only.as_deref().is_some_and(|x| !id.contains(x)) {
            continue;
        }
        let input = corpus_dir().join(o["input"].as_str().unwrap());
        if !input.exists() {
            continue;
        }
        let (_, ds) = reg
            .open(&input)
            .unwrap_or_else(|e| panic!("{}: {e}", input.display()));
        out.push((id, o, ds));
    }
    if out.is_empty() {
        eprintln!("no OpenLab CDS corpus files present; skipped");
    }
    out
}

fn f(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64)
}

/// Median, 95th percentile and maximum of |v|.
fn stats(v: &[f64]) -> (f64, f64, f64) {
    if v.is_empty() {
        return (f64::NAN, f64::NAN, f64::NAN);
    }
    let mut a: Vec<f64> = v.iter().map(|x| x.abs()).collect();
    a.sort_by(f64::total_cmp);
    let q = |p: f64| a[((a.len() - 1) as f64 * p).round() as usize];
    (q(0.5), q(0.95), a[a.len() - 1])
}

/// One row of our vendor-peak table: signal name, (column, value) pairs, baseline code.
type Row = (String, Vec<(String, f64)>, String);

/// Our vendor-peak rows in table order.
fn our_rows(ds: &mut dyn Dataset) -> Vec<Row> {
    let info = ds.info().unwrap();
    let t = &info.tables[0];
    let tab = ds.read_table(0, 0, t.row_count).unwrap();
    let cats = |c: usize| -> Vec<String> {
        t.columns[c].extra["categories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_string())
            .collect()
    };
    let (signals, codes) = (cats(0), cats(13));
    (0..t.row_count as usize)
        .map(|r| {
            let cols = t
                .columns
                .iter()
                .enumerate()
                .map(|(c, col)| (col.name.clone(), tab.columns[c][r]))
                .collect();
            (
                signals[tab.columns[0][r] as usize].clone(),
                cols,
                codes[tab.columns[13][r] as usize].clone(),
            )
        })
        .collect()
}

fn col(row: &[(String, f64)], name: &str) -> f64 {
    row.iter().find(|(n, _)| n == name).unwrap().1
}

#[test]
fn vendor_peak_tables() {
    let cases = cases();
    let (mut peaks, mut values, mut csv_values, mut meta) = (0usize, 0usize, 0usize, 0usize);
    let mut problems = Vec::new();
    for (id, o, mut ds) in cases {
        let rows = our_rows(ds.as_mut());
        let mut want: Vec<(String, &Value)> = Vec::new();
        for s in o["signals"].as_array().unwrap() {
            for p in s["peaks"].as_array().unwrap() {
                want.push((s["signal"].as_str().unwrap().to_string(), p));
            }
        }
        if rows.len() != want.len() {
            problems.push(format!("{id}: {} peaks, oracle {}", rows.len(), want.len()));
            continue;
        }
        for (signal, p) in &want {
            peaks += 1;
            let rt = f(p, "rt_min").unwrap();
            let Some((sig, row, code)) = rows.iter().find(|(_, r, _)| {
                col(r, "rt_min") == rt && col(r, "area") == f(p, "area").unwrap()
            }) else {
                problems.push(format!("{id}: vendor peak at {rt} min not in our table"));
                continue;
            };
            if sig != signal {
                problems.push(format!("{id} {rt}: signal {sig} != oracle {signal}"));
            }
            if Some(code.as_str()) != p["code"].as_str() {
                problems.push(format!(
                    "{id} {rt}: baseline code {code} != oracle {}",
                    p["code"]
                ));
            }
            for (ours, theirs) in [
                ("start_min", "start_min"),
                ("end_min", "end_min"),
                ("height", "height"),
                ("area_percent", "area_percent"),
                ("height_percent", "height_percent"),
                ("width_base_min", "width_base_min"),
                ("symmetry", "symmetry"),
                ("baseline_start", "baseline_start"),
                ("baseline_end", "baseline_end"),
            ] {
                let Some(w) = f(p, theirs) else { continue };
                values += 1;
                if col(row, ours) != w {
                    problems.push(format!(
                        "{id} {rt}: {ours} {} != oracle {w}",
                        col(row, ours)
                    ));
                }
            }
        }
        // the vendor's CSV export (Polyarc): values to its printed precision, and the metadata
        let Some(csv) = o.get("csv") else { continue };
        for c in csv["peaks"].as_array().unwrap() {
            let rt = f(c, "rt_min").unwrap();
            let Some((_, row, code)) = rows
                .iter()
                .find(|(_, r, _)| (col(r, "rt_min") - rt).abs() <= 0.0005 + 1e-9)
            else {
                problems.push(format!("{id}: CSV peak at {rt} min not in our table"));
                continue;
            };
            for (ours, theirs, half) in [
                ("width_base_min", "width_min", 0.005),
                ("area", "area", 0.005),
                ("height", "height", 0.005),
                ("area_percent", "area_percent", 0.005),
            ] {
                csv_values += 1;
                let w = f(c, theirs).unwrap();
                if (col(row, ours) - w).abs() > half + 1e-9 {
                    problems.push(format!("{id} {rt}: {ours} {} vs CSV {w}", col(row, ours)));
                }
            }
            if Some(code.as_str()) != c["type"].as_str() {
                problems.push(format!("{id} {rt}: code {code} vs CSV {}", c["type"]));
            }
        }
        let info = ds.info().unwrap();
        let e = &info.traces[0].extra;
        for (ours, theirs) in [
            ("sample_name", "sample_name"),
            ("operator", "operator"),
            ("vial", "vial"),
            ("method", "method"),
        ] {
            meta += 1;
            if e.get(ours).and_then(Value::as_str) != csv[theirs].as_str() {
                problems.push(format!(
                    "{id}: {ours} {:?} vs CSV {}",
                    e.get(ours),
                    csv[theirs]
                ));
            }
        }
        meta += 2;
        if e.get("injection_volume").and_then(Value::as_f64) != f(csv, "injection_volume_ul") {
            problems.push(format!(
                "{id}: injection volume {:?} vs CSV {}",
                e.get("injection_volume"),
                csv["injection_volume_ul"]
            ));
        }
        if info.traces[0].channels[0].name != csv["signal"].as_str().unwrap() {
            problems.push(format!(
                "{id}: first trace {} vs the CSV's signal {}",
                info.traces[0].channels[0].name, csv["signal"]
            ));
        }
    }
    println!(
        "vendor peak tables: {peaks} peaks, {values} values equal to allotropy's reading of the .rx, {csv_values} values within the CSV's printed precision, {meta} metadata fields equal to the CSV"
    );
    for p in problems.iter().take(20) {
        println!("  {p}");
    }
    assert!(problems.is_empty(), "{} problems", problems.len());
}

fn trace_chromatogram(ds: &mut dyn Dataset, signal: &str) -> Option<Chromatogram> {
    let info = ds.info().unwrap();
    let t = info.traces.iter().find(|t| t.channels[0].name == signal)?;
    let mut req = ChromRequest::default();
    req.targets = vec![Target::Trace {
        trace: t.index,
        channel: None,
    }];
    extract(ds, &info, &req, &ReadContext::default())
        .ok()
        .map(|mut o| o.chromatograms.remove(0))
}

/// The vendor peaks whose limits lie inside the decoded signal (the allotropy result sets hold
/// signals cut to 4 values).
fn usable(c: &Chromatogram, p: &Value) -> bool {
    let (Some(a), Some(b)) = (f(p, "start_min"), f(p, "end_min")) else {
        return false;
    };
    c.rt_min.len() > 100 && a >= c.rt_min[0] && b <= c.rt_min[c.rt_min.len() - 1]
}

#[test]
fn integration_with_vendor_limits() {
    let mut area_rel = Vec::new();
    let mut height_rel = Vec::new();
    let mut worst: Vec<(f64, String)> = Vec::new();
    for (id, o, mut ds) in cases() {
        for s in o["signals"].as_array().unwrap() {
            let Some(c) = trace_chromatogram(ds.as_mut(), s["signal"].as_str().unwrap()) else {
                continue;
            };
            for p in s["peaks"].as_array().unwrap() {
                if !usable(&c, p) {
                    continue;
                }
                let (a, b) = (f(p, "start_min").unwrap(), f(p, "end_min").unwrap());
                let (b0, b1) = (
                    f(p, "baseline_start").unwrap(),
                    f(p, "baseline_end").unwrap(),
                );
                let mut fp = PeakParams::default();
                fp.smooth = Some(0);
                fp.area_time_unit = AreaTimeUnit::S;
                let pk =
                    integrate_range(&c.rt_min, &c.intensity, a, b, Some((b0, b1)), &fp).unwrap();
                let va = f(p, "area").unwrap();
                let rel = pk.area / va - 1.0;
                area_rel.push(rel);
                worst.push((
                    rel.abs(),
                    format!(
                        "{id} {} {:.3} min: area {:.4} vs vendor {va:.4} ({:+.3} %)",
                        s["signal"],
                        f(p, "rt_min").unwrap(),
                        pk.area,
                        100.0 * rel
                    ),
                ));
                height_rel.push(pk.height / f(p, "height").unwrap() - 1.0);
            }
        }
    }
    if area_rel.is_empty() {
        return;
    }
    let (m, p95, mx) = stats(&area_rel);
    let (hm, hp95, hmx) = stats(&height_rel);
    println!(
        "vendor limits and baseline: {} peaks; area |ours/vendor − 1| median {:.4} %, p95 {:.4} %, max {:.4} %; height median {:.3} %, p95 {:.3} %, max {:.3} %",
        area_rel.len(),
        100.0 * m,
        100.0 * p95,
        100.0 * mx,
        100.0 * hm,
        100.0 * hp95,
        100.0 * hmx
    );
    worst.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (_, w) in worst.iter().take(5) {
        println!("    worst: {w}");
    }
    assert!(
        m < 1e-3 && p95 < 5e-3,
        "areas with the vendor's limits: median {m}, p95 {p95}"
    );
    assert!(hm < 0.01, "heights with the vendor's limits: median {hm}");
}

/// Automatic integration of every usable vendor signal with one baseline mode: (vendor peaks,
/// found, retention-time differences, area ratios − 1, area % differences among the vendor's
/// peaks, missed peaks).
fn automatic(mode: BaselineMode) -> (usize, usize, Vec<f64>, Vec<f64>, Vec<f64>, Vec<String>) {
    let (mut vendor, mut found) = (0usize, 0usize);
    let (mut rt_d, mut area_rel, mut pct_d) = (Vec::new(), Vec::new(), Vec::new());
    let mut missed = Vec::new();
    for (id, o, mut ds) in cases() {
        for s in o["signals"].as_array().unwrap() {
            let Some(c) = trace_chromatogram(ds.as_mut(), s["signal"].as_str().unwrap()) else {
                continue;
            };
            let vp: Vec<&Value> = s["peaks"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|p| usable(&c, p))
                .collect();
            if vp.is_empty() {
                continue;
            }
            let mut pp = PeakParams::default();
            pp.area_time_unit = AreaTimeUnit::S;
            pp.baseline = mode;
            let table = find_peaks(&c.rt_min, &c.intensity, &pp).unwrap();
            let mut pairs = Vec::new();
            for p in vp {
                vendor += 1;
                let (a, b) = (f(p, "start_min").unwrap(), f(p, "end_min").unwrap());
                let inside: Vec<_> = table
                    .peaks
                    .iter()
                    .filter(|q| q.rt_min >= a && q.rt_min <= b)
                    .collect();
                let Some(top) = inside.iter().max_by(|x, y| x.height.total_cmp(&y.height)) else {
                    missed.push(format!("{id} {:.3}", f(p, "rt_min").unwrap()));
                    continue;
                };
                found += 1;
                rt_d.push(top.rt_min - f(p, "rt_min").unwrap());
                let ours: f64 = inside.iter().map(|q| q.area).sum();
                area_rel.push(ours / f(p, "area").unwrap() - 1.0);
                pairs.push((ours, f(p, "area").unwrap()));
            }
            let (so, sv) = pairs
                .iter()
                .fold((0.0, 0.0), |(a, b), (x, y)| (a + x, b + y));
            for (x, y) in pairs {
                pct_d.push(100.0 * (x / so - y / sv));
            }
        }
    }
    (vendor, found, rt_d, area_rel, pct_d, missed)
}

#[test]
fn automatic_peaks_against_vendor() {
    for mode in [BaselineMode::Drop, BaselineMode::Valley, BaselineMode::Auto] {
        let (vendor, found, rt_d, area_rel, pct_d, missed) = automatic(mode);
        if vendor == 0 {
            return;
        }
        let (rm, rp, rx) = stats(&rt_d);
        let (am, ap, ax) = stats(&area_rel);
        let (pm, pp, px) = stats(&pct_d);
        println!(
            "automatic peaks ({mode:?} baseline): {found}/{vendor} vendor peaks found; missed: {missed:?}"
        );
        println!("  retention time |ours − vendor| median {rm:.4} min, p95 {rp:.4}, max {rx:.4}");
        println!(
            "  area |ours/vendor − 1| median {:.2} %, p95 {:.2} %, max {:.1} %",
            100.0 * am,
            100.0 * ap,
            100.0 * ax
        );
        println!(
            "  area % among the vendor's peaks |ours − vendor| median {pm:.3} pts, p95 {pp:.3}, max {px:.3}"
        );
        match mode {
            BaselineMode::Drop => {
                assert!(
                    found * 100 >= vendor * 97,
                    "only {found}/{vendor} vendor peaks found"
                );
                assert!(rm < 0.005, "median retention-time difference {rm} min");
            }
            // OpenLab's integrator ends the peaks of these solvent-tail GC runs where they meet
            // the tail: `valley` and the default `auto` reproduce its areas
            _ => assert!(
                am < 0.01 && ap < 0.1 && pp < 2.0 && found == vendor,
                "{mode:?} baseline: {found}/{vendor} found, area median {am}, p95 {ap}, area % p95 {pp} points"
            ),
        }
    }
}

/// DAD spectra parts (`Spectra131`, f64 records) against the DAD's own channels of the same
/// injection (`DAD1A,Sig=210,4`: 210 nm, 4 nm bandwidth), recorded separately by the detector:
/// the spectra averaged over the channel's band must be a straight line of slope 1 within 3 %
/// (this fixes the unit: stored f64 × scale × scale = mAU) with r > 0.9999.
#[test]
fn spectra_agree_with_the_dad_channels() {
    let reg = registry();
    let mut compared = 0;
    for rel in [
        "openlab-cct/MeOH1.dx",
        "openlab-cct/openlab.sirslt/Norbert II-2026-05-26 16-19-41-05-00.dx",
    ] {
        let path = corpus_dir().join(rel);
        if !path.exists() {
            eprintln!("skip {rel}");
            continue;
        }
        let (_, mut ds) = reg.open(&path).unwrap();
        let info = ds.info().unwrap();
        let field = info
            .traces
            .iter()
            .find(|t| t.extra.contains_key("spectra"))
            .expect("a spectra trace");
        let cube = ds
            .read_trace(field.index, 0, 0, field.sample_count)
            .unwrap()
            .channels;
        let waves: Vec<f64> = field
            .channels
            .iter()
            .map(|c| c.extra["wavelength_nm"].as_f64().unwrap())
            .collect();
        for t in &info.traces {
            // "DAD1A,Sig=210,4  Ref=off"
            let Some(name) = t.name.as_deref() else {
                continue;
            };
            let Some(sig) = name.split("Sig=").nth(1) else {
                continue;
            };
            let mut it = sig.split([',', ' ']);
            let (Some(w), Some(bw)) = (
                it.next().and_then(|x| x.parse::<f64>().ok()),
                it.next().and_then(|x| x.parse::<f64>().ok()),
            ) else {
                continue;
            };
            let band: Vec<usize> = (0..waves.len())
                .filter(|&k| (waves[k] - w).abs() <= bw / 2.0 + 1e-9)
                .collect();
            if band.is_empty() || t.sample_count != field.sample_count {
                continue;
            }
            let ch = ds
                .read_trace(t.index, 0, 0, t.sample_count)
                .unwrap()
                .channels
                .remove(0);
            let x: Vec<f64> = (0..ch.len())
                .map(|i| band.iter().map(|&k| cube[k][i]).sum::<f64>() / band.len() as f64)
                .collect();
            let n = x.len() as f64;
            let (mx, my) = (x.iter().sum::<f64>() / n, ch.iter().sum::<f64>() / n);
            let sxx: f64 = x.iter().map(|a| (a - mx).powi(2)).sum();
            let syy: f64 = ch.iter().map(|b| (b - my).powi(2)).sum();
            let sxy: f64 = x.iter().zip(&ch).map(|(a, b)| (a - mx) * (b - my)).sum();
            let (slope, r) = (sxy / sxx, sxy / (sxx * syy).sqrt());
            eprintln!("{rel} {name}: slope {slope:.4}, r {r:.6}");
            assert!(
                (slope - 1.0).abs() < 0.03 && r > 0.9999,
                "{rel} {name}: slope {slope}, r {r}"
            );
            compared += 1;
        }
    }
    assert!(compared >= 4, "{compared} DAD channels compared");
}
