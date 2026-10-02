//! Spectral bands and regions (`analyze peaks --x-range` on spectra, `openreadout_quant::bands`)
//! against `corpus/oracle/bands/*.json` (`oracle/bands.py`): each spectrum read with a
//! third-party reader (brukeropus, SpectroChemPy, renishawWiRE, specio, jcamp, nmrglue) and its
//! regions integrated with NumPy on the same samples and baseline.
//!
//! - every region, with the linear and with no baseline: sample count (exact), first and last x,
//!   area, area without baseline, maximum position and height, centroid (relative 1e-6; where a
//!   reader rounds its axis, the oracle's `tolerance` says by how much and why);
//! - the same areas from the vendor's own export of the spectrum (OMNIC CSV), to the precision
//!   the export prints;
//! - the five most prominent bands of `scipy.signal.find_peaks` found by our detector: inside one
//!   of our bands, its apex within 10 % of that band's width (or 1.5 samples);
//! - TopSpin's own integrals of a Bruker NMR spectrum (`integrals.txt`, no baseline, normalised
//!   to region 1) against our `--baseline none` region areas scaled the same way.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test bands -- --nocapture`
#![cfg(feature = "corpus")]
#![allow(clippy::many_single_char_names)] // t (tally), o (oracle), f, g, r, x: the other corpus tests' names

use std::path::{Path, PathBuf};

use openreadout_core::Registry;
use openreadout_quant::bands::{RegionBaseline, SpectrumBands, analyse, read_spectrum};
use openreadout_quant::peaks::PeakParams;
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
    Registry::new()
        .with(Box::new(openreadout_spectro::OpusReader))
        .with(Box::new(openreadout_spectro::OmnicReader))
        .with(Box::new(openreadout_spectro::WdfReader))
        .with(Box::new(openreadout_spectro::PeSpReader))
        .with(Box::new(openreadout_spectro::JwsReader))
        .with(Box::new(openreadout_spectro::CaryReader))
        .with(Box::new(openreadout_nmr::BrukerReader))
        .with(Box::new(openreadout_nmr::JcampReader))
}

/// The `filename` of an input entry of the manifest.
fn input_path(id: &str) -> Option<PathBuf> {
    let manifest = std::fs::read_to_string(root().join("corpus/manifest.toml")).ok()?;
    for block in manifest.split("[[file]]") {
        if block.lines().any(|l| l == format!("id = \"{id}\""))
            && block.lines().any(|l| l == "role = \"input\"")
        {
            let name = block
                .lines()
                .find_map(|l| l.strip_prefix("filename = "))?
                .trim_matches('"');
            return Some(corpus_dir().join(name));
        }
    }
    None
}

fn close(ours: f64, theirs: f64, rel: f64, abs: f64) -> bool {
    (ours - theirs).abs() <= abs.max(rel * theirs.abs())
}

#[derive(Default)]
struct Tally {
    checks: usize,
    failures: Vec<String>,
    /// Largest relative difference per source: NumPy on the reader's samples, the vendor's
    /// export, TopSpin.
    worst_rel: std::collections::BTreeMap<&'static str, f64>,
}

impl Tally {
    fn check(&mut self, what: String, ours: f64, theirs: f64, rel: f64, abs: f64) {
        self.checks += 1;
        if theirs != 0.0 {
            let source = if what.contains("export") {
                "vendor export"
            } else if what.contains("TopSpin") {
                "TopSpin integrals"
            } else {
                "NumPy on the reader's samples"
            };
            let w = self.worst_rel.entry(source).or_default();
            *w = w.max((ours - theirs).abs() / theirs.abs());
        }
        if !close(ours, theirs, rel, abs) {
            self.failures
                .push(format!("{what}: ours {ours} vs {theirs}"));
        }
    }
    fn fail(&mut self, what: String) {
        self.checks += 1;
        self.failures.push(what);
    }
}

fn run(path: &Path, trace: u32, ranges: &[[f64; 2]], baseline: RegionBaseline) -> SpectrumBands {
    let reg = registry();
    let (_, mut ds) = reg.open(path).unwrap();
    let info = ds.info().unwrap();
    let s = read_spectrum(ds.as_mut(), &info, trace, None, 0).unwrap();
    analyse(&s, &PeakParams::default(), ranges, baseline).unwrap()
}

#[test]
fn regions_and_bands_match_numpy_and_vendors() {
    let dir = root().join("corpus/oracle/bands");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    assert!(!files.is_empty());
    let mut t = Tally::default();
    for f in files {
        let o: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        let id = o["id"].as_str().unwrap();
        let Some(path) = input_path(id).filter(|p| p.exists()) else {
            eprintln!("skip {id}: corpus file not fetched");
            continue;
        };
        let trace = o["trace"].as_u64().unwrap() as u32;
        let ranges: Vec<[f64; 2]> = o["regions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                [
                    r["range"][0].as_f64().unwrap(),
                    r["range"][1].as_f64().unwrap(),
                ]
            })
            .collect();
        let lin = run(&path, trace, &ranges, RegionBaseline::Linear);
        let none = run(&path, trace, &ranges, RegionBaseline::None);
        assert_eq!(lin.points, o["points"].as_u64().unwrap(), "{id}: points");
        assert_eq!(
            lin.bands_are_minima,
            o["bands_are_minima"].as_bool().unwrap(),
            "{id}: minima"
        );
        let scale = lin
            .regions
            .iter()
            .map(|g| g.area_no_baseline.abs())
            .fold(1e-300, f64::max);
        let step = o["median_step"].as_f64().unwrap();
        let x_tol = o["tolerance"]["x"].as_f64().unwrap();
        let area_rel = o["tolerance"]["area_rel"].as_f64().unwrap();
        let check = |t: &mut Tally, what: String, ours: f64, theirs: f64, rel: f64, abs: f64| {
            t.check(format!("{id} {what}"), ours, theirs, rel, abs);
        };
        for (k, r) in o["regions"].as_array().unwrap().iter().enumerate() {
            for (name, out) in [("linear", &lin), ("none", &none)] {
                let want = &r[name];
                let g = &out.regions[k];
                let tag = |f: &str| format!("region {} {name} {f}", k + 1);
                check(
                    &mut t,
                    tag("points"),
                    f64::from(g.points),
                    want["points"].as_f64().unwrap(),
                    0.0,
                    0.0,
                );
                check(
                    &mut t,
                    tag("x_first"),
                    g.x_first,
                    want["x_first"].as_f64().unwrap(),
                    0.0,
                    x_tol,
                );
                check(
                    &mut t,
                    tag("x_last"),
                    g.x_last,
                    want["x_last"].as_f64().unwrap(),
                    0.0,
                    x_tol,
                );
                let a = 1e-9 * scale;
                check(
                    &mut t,
                    tag("area"),
                    g.area,
                    want["area"].as_f64().unwrap(),
                    area_rel,
                    a,
                );
                check(
                    &mut t,
                    tag("area_no_baseline"),
                    g.area_no_baseline,
                    want["area_no_baseline"].as_f64().unwrap(),
                    area_rel,
                    a,
                );
                check(
                    &mut t,
                    tag("max_x"),
                    g.max_x,
                    want["max_x"].as_f64().unwrap(),
                    0.0,
                    x_tol,
                );
                check(
                    &mut t,
                    tag("max_height"),
                    g.max_height,
                    want["max_height"].as_f64().unwrap(),
                    area_rel,
                    1e-9 * scale,
                );
                match (g.centroid, want.get("centroid").and_then(Value::as_f64)) {
                    (Some(c), Some(w)) => {
                        check(
                            &mut t,
                            tag("centroid"),
                            c,
                            w,
                            1e-6,
                            x_tol.max(1e-6 * step.abs()),
                        );
                    }
                    (None, None) => {}
                    (c, w) => t.fail(format!("{id} {}: ours {c:?} vs {w:?}", tag("centroid"))),
                }
            }
        }
        // the vendor's export of the same spectrum (printed to ~6 digits)
        if let Some(ex) = o.get("export") {
            for (k, r) in ex["regions"].as_array().unwrap().iter().enumerate() {
                let want = &r["linear"];
                let g = &lin.regions[k];
                check(
                    &mut t,
                    format!("region {} vs export area", k + 1),
                    g.area,
                    want["area"].as_f64().unwrap(),
                    2e-4,
                    2e-5 * scale,
                );
                check(
                    &mut t,
                    format!("region {} vs export max_x", k + 1),
                    g.max_x,
                    want["max_x"].as_f64().unwrap(),
                    0.0,
                    1.01 * step.abs(),
                );
            }
        }
        // the most prominent bands of scipy.signal.find_peaks
        let all = run(&path, trace, &[], RegionBaseline::Linear);
        for b in o["prominent_bands"].as_array().unwrap() {
            let x = b.as_f64().unwrap();
            // the same band: inside one of ours, its apex within 10 % of that band's width (the
            // detector's apex is the vertex of the smoothed signal; SciPy's the raw sample)
            let found = all.peaks.iter().any(|p| {
                let width = p.width_half.unwrap_or(p.end - p.start);
                x >= p.start && x <= p.end && (p.x - x).abs() <= (1.5 * step.abs()).max(0.1 * width)
            });
            if found {
                t.checks += 1;
            } else {
                t.fail(format!("{id}: prominent band at {x} not detected"));
            }
        }
        // TopSpin's own integrals (no baseline), normalised to region 1
        if let Some(v) = o.get("vendor") {
            let want: Vec<f64> = v["integrals"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_f64().unwrap())
                .collect();
            let reference = none.regions[0].area_no_baseline;
            for (k, w) in want.iter().enumerate() {
                let ours = none.regions[k].area_no_baseline / reference * want[0];
                check(
                    &mut t,
                    format!("TopSpin integral {}", k + 1),
                    ours,
                    *w,
                    5e-3,
                    0.0,
                );
            }
        }
        eprintln!(
            "ok {id}: {} regions, {} bands",
            lin.regions.len(),
            all.peak_count
        );
    }
    eprintln!("{} checks, {} failures", t.checks, t.failures.len());
    for (source, w) in &t.worst_rel {
        eprintln!("  largest relative difference vs {source}: {w:.2e}");
    }
    assert!(t.failures.is_empty(), "{}", t.failures.join("\n"));
}
