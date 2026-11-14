//! 2D peak picking (`openreadout_signal::nmr::analyze_2d`) on every processed Bruker 2D
//! spectrum of the development corpus (`pdata/<n>/2rr`), checked three ways:
//!
//! - against nmrglue's `analysis.peakpick.pick` run as a black box on the same `2rr` at the same
//!   absolute threshold (`tests/corpus/nmr2d_oracle/*.json`, written by
//!   `tests/corpus/nmr2d_oracle/nmrglue_peaks.py`): recall and precision of the maxima within
//!   1.5 points in each dimension, and the largest position difference;
//! - chemistry: homonuclear spectra are symmetric about the diagonal, so most off-diagonal cross
//!   peaks have a partner at the mirror image; one-bond 1H–13C spectra (HSQC, HMQC) put every
//!   cross peak on a 1H line and a 13C line of the same sample's TopSpin 1D peak lists
//!   (`pdata/1/peaklist.xml`);
//! - the JCAMP-DX `nD NMR SPECTRUM` export of the `nmrxiv-s200` limonene HSQC.
//!
//! Tuning set and test set: experiments are split by a hash of their path (`tuning_set`); the
//! defaults were chosen on the tuning half and the tables report both halves.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test
//! nmr_2d_peaks -- --nocapture`. `OPENREADOUT_NMR2D_OUT=DIR` also writes our picks as JSON
//! there (input of `nmrglue_peaks.py`).
#![cfg(feature = "corpus")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use openreadout_core::FormatReader;
use openreadout_signal::nmr::{self, CrossPeak, CrossPeakOptions, Nmr2dReport, Nmr2dRequest, PeakFlag};
use serde::Deserialize;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn files_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root().join("corpus/files"), PathBuf::from)
}

/// Every `pdata/<n>` with a `2rr` up to six levels below the corpus directory, held-out files
/// excluded.
fn processed_2d() -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for p in entries {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            if depth == 0 && (name.starts_with("ho-") || name == "heldout") {
                continue;
            }
            if p.is_dir() {
                if p.join("2rr").is_file() && p.parent().is_some_and(|q| q.ends_with("pdata")) {
                    out.push(p.clone());
                } else if depth < 5 {
                    walk(&p, depth + 1, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(&files_dir(), 0, &mut out);
    out
}

/// `nmrxiv-s279/23/pdata/1` → `nmrxiv-s279-23-1`.
fn id_of(pdata: &Path) -> String {
    let rel = pdata.strip_prefix(files_dir()).unwrap_or(pdata);
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().replace(' ', "_"))
        .filter(|c| c != "pdata")
        .collect();
    parts.join("-").replace("-bundle", "")
}

/// Split by a hash of the id: about half the experiments tune the defaults.
fn tuning_set(id: &str) -> bool {
    let h = id
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3));
    h % 2 == 0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Cosy,
    Tocsy,
    Noesy,
    Roesy,
    OneBond,
    LongRange,
    JRes,
    Dosy,
    Other,
}

fn kind_of(pulprog: &str) -> Kind {
    let p = pulprog.to_ascii_lowercase();
    if p.contains("cosy") {
        Kind::Cosy
    } else if p.contains("mlev") || p.contains("dipsi") || p.contains("tocsy") {
        Kind::Tocsy
    } else if p.contains("noesy") {
        Kind::Noesy
    } else if p.contains("roesy") {
        Kind::Roesy
    } else if p.contains("hsqc") || p.contains("hmqc") {
        Kind::OneBond
    } else if p.contains("hmbc") {
        Kind::LongRange
    } else if p.contains("jres") {
        Kind::JRes
    } else if p.contains("ledbp") || p.contains("dosy") || p.contains("stebp") {
        Kind::Dosy
    } else {
        Kind::Other
    }
}

fn param(path: &Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let tag = format!("##${key}=");
    let line = text.lines().find(|l| l.starts_with(&tag))?;
    Some(
        line[tag.len()..]
            .trim()
            .trim_matches(['<', '>'])
            .to_string(),
    )
}

/// TopSpin 1D peak list of an experiment: shifts (ppm) and intensities.
fn peaklist_1d(exp: &Path) -> Vec<(f64, f64)> {
    let Ok(text) = std::fs::read_to_string(exp.join("pdata/1/peaklist.xml")) else {
        return Vec::new();
    };
    let attr = |tag: &str, name: &str| -> Option<f64> {
        let k = format!("{name}=\"");
        let i = tag.find(&k)? + k.len();
        let j = tag[i..].find('"')? + i;
        tag[i..j].trim().parse().ok()
    };
    text.split("<Peak1D ")
        .skip(1)
        .filter_map(|t| Some((attr(t, "F1")?, attr(t, "intensity").unwrap_or(0.0))))
        .collect()
}

/// Sibling experiments' 1D lists of a nucleus (1H or 13C), and whether one is DEPT-135.
fn sibling_lists(exp: &Path, nucleus: &str) -> Vec<(String, Vec<(f64, f64)>)> {
    let Some(parent) = exp.parent() else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut dirs: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    dirs.sort();
    for d in dirs {
        if d.join("ser").exists() || !d.join("fid").exists() {
            continue;
        }
        if param(&d.join("acqus"), "NUC1").as_deref() != Some(nucleus) {
            continue;
        }
        let list = peaklist_1d(&d);
        if !list.is_empty() {
            let pp = param(&d.join("acqus"), "PULPROG").unwrap_or_default();
            out.push((pp, list));
        }
    }
    out
}

/// nmrglue's picks (`nmrglue_peaks.py`): `[row, column, height]`, tallest first, at most 300.
#[derive(Deserialize)]
struct NgOracle {
    threshold: f64,
    peaks_total: usize,
    peaks: Vec<[f64; 3]>,
    connected_total: usize,
    connected: Vec<[f64; 3]>,
}

fn ng_oracle(id: &str) -> Option<NgOracle> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/nmr2d_oracle")
        .join(format!("{id}.json"));
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

struct Row {
    id: String,
    kind: Kind,
    tuning: bool,
    shape: Vec<usize>,
    peaks: usize,
    maxima: usize,
    ridge: usize,
    secs: f64,
    ng: Option<NgStats>,
    sym: Option<(usize, usize)>,            // with partner, off-diagonal
    hsqc: Option<(usize, usize, usize, usize)>, // on 1H line, on 13C line, checked, 13C lines seen
}

/// `OPENREADOUT_NMR2D_OPTIONS='{"min_snr": 8}'` overrides options while tuning.
fn tuned(o: CrossPeakOptions) -> CrossPeakOptions {
    let Some(v) = std::env::var_os("OPENREADOUT_NMR2D_OPTIONS") else {
        return o;
    };
    let mut base = serde_json::to_value(&o).unwrap();
    let over: serde_json::Value = serde_json::from_str(&v.to_string_lossy()).unwrap();
    for (k, x) in over.as_object().unwrap() {
        base[k] = x.clone();
    }
    serde_json::from_value(base).unwrap()
}

fn analyze(pdata: &Path) -> (Nmr2dReport, f64) {
    let exp = pdata.parent().and_then(Path::parent).unwrap();
    let mut ds = openreadout_nmr::BrukerReader.open(exp).unwrap();
    let info = ds.info().unwrap();
    let name = format!(
        "pdata/{}",
        pdata.file_name().unwrap().to_string_lossy()
    );
    let trace = info
        .traces
        .iter()
        .find(|t| t.name.as_deref() == Some(name.as_str()))
        .map(|t| t.index);
    let req = Nmr2dRequest {
        trace,
        peaks: tuned(CrossPeakOptions {
            list_maxima: true,
            ..CrossPeakOptions::default()
        }),
        ..Nmr2dRequest::default()
    };
    let t0 = Instant::now();
    let r = nmr::analyze_2d(ds.as_mut(), &info, &req).unwrap();
    (r, t0.elapsed().as_secs_f64())
}

/// Our maxima against nmrglue's within 1.5 points in each dimension, above the height of the
/// smallest pick nmrglue's list keeps (its threshold when the list is complete): (matched
/// nmrglue maxima, nmrglue maxima, matched ours, ours with 20 % margin above that height,
/// nmrglue connected regions with one of our maxima or footprints, connected regions, largest
/// offset in points).
fn compare_ng(r: &Nmr2dReport, ng: &NgOracle) -> NgStats {
    let floor = |list: &[[f64; 3]], total: usize| {
        if total > list.len() {
            list.last().map_or(ng.threshold, |p| p[2].abs())
        } else {
            ng.threshold
        }
    };
    let cut = floor(&ng.peaks, ng.peaks_total);
    let ours: Vec<(f64, f64, f64)> = r
        .maxima
        .iter()
        .map(|m| {
            (
                r.axes[0].index_of(m.ppm[0]),
                r.axes[1].index_of(m.ppm[1]),
                m.height.abs(),
            )
        })
        .collect();
    let near = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs().max((a.1 - b.1).abs());
    let theirs: Vec<(f64, f64)> = ng.peaks.iter().filter(|p| p[2].abs() >= cut).map(|p| (p[0], p[1])).collect();
    let mut st = NgStats::default();
    for t in &theirs {
        let best = ours.iter().map(|o| near((o.0, o.1), *t)).fold(f64::INFINITY, f64::min);
        st.theirs += 1;
        if best <= 1.5 {
            st.matched_theirs += 1;
            st.max_offset = st.max_offset.max(best);
        }
    }
    for o in ours.iter().filter(|o| o.2 >= 1.2 * cut) {
        st.ours += 1;
        if theirs.iter().any(|t| near((o.0, o.1), *t) <= 1.5) {
            st.matched_ours += 1;
        }
    }
    let ccut = floor(&ng.connected, ng.connected_total);
    for c in ng.connected.iter().filter(|p| p[2].abs() >= ccut) {
        st.connected += 1;
        let ppm = (r.axes[0].ppm(c[0]), r.axes[1].ppm(c[1]));
        let inside = r.peaks.iter().any(|p| {
            let (f1, f2) = (p.footprint_ppm[0], p.footprint_ppm[1]);
            ppm.0 <= f1.0 && ppm.0 >= f1.1 && ppm.1 <= f2.0 && ppm.1 >= f2.1
        });
        if inside || ours.iter().any(|o| near((o.0, o.1), (c[0], c[1])) <= 1.5) {
            st.connected_found += 1;
        }
    }
    st
}

#[derive(Debug, Clone, Copy, Default)]
struct NgStats {
    matched_theirs: usize,
    theirs: usize,
    matched_ours: usize,
    ours: usize,
    connected_found: usize,
    connected: usize,
    max_offset: f64,
}

fn symmetry(r: &Nmr2dReport) -> Option<(usize, usize)> {
    r.homonuclear_dimensions?;
    let off: Vec<&CrossPeak> = r
        .peaks
        .iter()
        .filter(|p| !p.has(PeakFlag::Diagonal) && !p.has(PeakFlag::Ridge) && !p.has(PeakFlag::Axial))
        .collect();
    Some((
        off.iter().filter(|p| p.symmetric_partner.is_some()).count(),
        off.len(),
    ))
}

/// HSQC: cross peaks (not ridge/axial) on a 1H list line (≤ 0.03 ppm or the F2 half width) and a
/// 13C list line (≤ 1.5 F1 points); and how many 13C lines carry a cross peak.
fn one_bond(r: &Nmr2dReport, h: &[(f64, f64)], c: &[(f64, f64)]) -> (usize, usize, usize, usize) {
    let peaks: Vec<&CrossPeak> = r
        .peaks
        .iter()
        .filter(|p| !p.has(PeakFlag::Ridge) && !p.has(PeakFlag::Axial))
        .collect();
    let f1_tol = 1.5 * r.axes[0].step_ppm.abs();
    let mut on_h = 0;
    let mut on_c = 0;
    for p in &peaks {
        let tol_h = 0.03_f64.max(p.width_ppm[1].unwrap_or(0.0));
        if h.iter().any(|l| (l.0 - p.ppm[1]).abs() <= tol_h) {
            on_h += 1;
        }
        if c.iter().any(|l| (l.0 - p.ppm[0]).abs() <= f1_tol) {
            on_c += 1;
        }
    }
    let (lo, hi) = {
        let a = &r.axes[0];
        let (x, y) = (a.first_ppm, a.last_ppm());
        (x.min(y), x.max(y))
    };
    let c_seen = c
        .iter()
        .filter(|l| l.0 > lo && l.0 < hi)
        .filter(|l| peaks.iter().any(|p| (l.0 - p.ppm[0]).abs() <= f1_tol))
        .count();
    (on_h, on_c, peaks.len(), c_seen)
}

#[test]
fn bruker_2d_spectra() {
    let list = processed_2d();
    if list.is_empty() {
        eprintln!("no processed 2D spectra under {}", files_dir().display());
        return;
    }
    let out_dir = std::env::var_os("OPENREADOUT_NMR2D_OUT").map(PathBuf::from);
    let mut rows = Vec::new();
    let mut failures = Vec::new();
    for pdata in &list {
        let id = id_of(pdata);
        let exp = pdata.parent().and_then(Path::parent).unwrap();
        let pulprog = param(&exp.join("acqus"), "PULPROG").unwrap_or_default();
        let kind = kind_of(&pulprog);
        let (r, secs) = analyze(pdata);
        if let Some(dir) = &out_dir {
            std::fs::create_dir_all(dir).unwrap();
            let v = serde_json::json!({
                "id": id, "pdata": pdata, "kind": format!("{kind:?}"),
                "report": r,
            });
            std::fs::write(dir.join(format!("{id}.json")), v.to_string()).unwrap();
        }
        let ng = ng_oracle(&id).map(|o| compare_ng(&r, &o));
        let sym = symmetry(&r);
        let hsqc = (kind == Kind::OneBond && r.nuclei.get(0).cloned().flatten().as_deref() == Some("13C"))
            .then(|| {
                let h: Vec<(f64, f64)> = sibling_lists(exp, "1H").into_iter().flat_map(|x| x.1).collect();
                let c: Vec<(f64, f64)> = sibling_lists(exp, "13C")
                    .into_iter()
                    .filter(|x| !x.0.to_ascii_lowercase().contains("dept"))
                    .flat_map(|x| x.1)
                    .collect();
                (!h.is_empty() && !c.is_empty()).then(|| one_bond(&r, &h, &c))
            })
            .flatten();
        rows.push(Row {
            id: id.clone(),
            kind,
            tuning: tuning_set(&id),
            shape: r.shape.clone(),
            peaks: r.peak_count,
            maxima: r.maxima_count,
            ridge: r.ridge_suppressed,
            secs,
            ng,
            sym,
            hsqc,
        });
        if r.peak_count == 0 && kind != Kind::Dosy {
            failures.push(format!("{id}: no peaks"));
        }
    }
    eprintln!(
        "{:<42} {:<9} {:>3} {:>11} {:>5} {:>6} {:>5} {:>6}  {:<26} {:<12} {}",
        "experiment", "kind", "set", "shape", "peaks", "maxima", "ridge", "s", "nmrglue (rec/prec/off)", "symmetric", "HSQC on 1H/13C lines"
    );
    for r in &rows {
        let ng = r.ng.map_or("–".to_string(), |g| {
            format!(
                "{}/{} {}/{} {}/{} {:.2}",
                g.matched_theirs, g.theirs, g.matched_ours, g.ours, g.connected_found, g.connected, g.max_offset
            )
        });
        let sym = r
            .sym
            .map_or("–".to_string(), |(a, b)| format!("{a}/{b}"));
        let hs = r.hsqc.map_or("–".to_string(), |(h, c, n, cs)| {
            format!("{h}/{n}, {c}/{n}; 13C lines {cs}")
        });
        eprintln!(
            "{:<42} {:<9} {:>3} {:>11} {:>5} {:>6} {:>5} {:>6.2}  {:<26} {:<12} {}",
            r.id,
            format!("{:?}", r.kind),
            if r.tuning { "tune" } else { "test" },
            format!("{}x{}", r.shape[0], r.shape[1]),
            r.peaks,
            r.maxima,
            r.ridge,
            r.secs,
            ng,
            sym,
            hs
        );
    }
    let mut by_kind: BTreeMap<(Kind, bool), (NgStats, usize)> = BTreeMap::new();
    for r in &rows {
        if let Some(g) = r.ng {
            let e = by_kind.entry((r.kind, r.tuning)).or_default();
            e.0.matched_theirs += g.matched_theirs;
            e.0.theirs += g.theirs;
            e.0.matched_ours += g.matched_ours;
            e.0.ours += g.ours;
            e.0.connected_found += g.connected_found;
            e.0.connected += g.connected;
            e.0.max_offset = e.0.max_offset.max(g.max_offset);
            e.1 += 1;
        }
    }
    let ratio = |a: usize, b: usize| a as f64 / b.max(1) as f64;
    for ((k, tune), (g, n)) in &by_kind {
        eprintln!(
            "nmrglue {k:?} {}: {n} spectra, maxima recall {:.3}, precision {:.3}, regions found {:.3}, largest offset {:.2} points",
            if *tune { "tune" } else { "test" },
            ratio(g.matched_theirs, g.theirs),
            ratio(g.matched_ours, g.ours),
            ratio(g.connected_found, g.connected),
            g.max_offset
        );
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The MestReNova JCAMP-DX export of a JEOL limonene HSQC (`nD NMR SPECTRUM`, F1 pages in Hz):
/// every cross peak sits on a 1H line of the same sample's 1H spectrum and a 13C line of its
/// 13C spectrum (both JCAMP-DX exports, picked with the 1D picker), and every protonated carbon
/// of limonene (8 of 10 carbons) has a cross peak.
#[test]
fn jcamp_hsqc_limonene() {
    let dir = files_dir().join("nmrxiv-s200");
    let hsqc = dir.join("Limonene_7020ug200uL_CDCl3_HSQC_400MHz_JDX.jdx");
    if !hsqc.exists() {
        eprintln!("skipped: {} not on disk", hsqc.display());
        return;
    }
    let lines = |name: &str| -> Vec<nmr::Peak> {
        let mut ds = openreadout_nmr::JcampReader.open(&dir.join(name)).unwrap();
        let info = ds.info().unwrap();
        let req = nmr::NmrRequest {
            source: nmr::SpectrumSource::Processed,
            ..nmr::NmrRequest::default()
        };
        nmr::analyze(ds.as_mut(), &info, &req).unwrap().peaks
    };
    let h = lines("Limonene_7020ug200uL_CDCl3_qHNMR_400MHz_JDX.jdx");
    let c = lines("Limonene_7020ug200uL_CDCl3_13CNMR_400MHz_JDX.jdx");
    let mut ds = openreadout_nmr::JcampReader.open(&hsqc).unwrap();
    let info = ds.info().unwrap();
    let r = nmr::analyze_2d(ds.as_mut(), &info, &Nmr2dRequest::default()).unwrap();
    assert_eq!(r.nuclei, vec![Some("13C".to_string()), Some("1H".to_string())]);
    let f1_tol = 1.5 * r.axes[0].step_ppm.abs();
    let real: Vec<&CrossPeak> = r
        .peaks
        .iter()
        .filter(|p| !p.has(PeakFlag::Ridge) && !p.has(PeakFlag::Axial))
        .collect();
    // the two experiments are referenced separately: F1 offset = median distance of the 12
    // tallest cross peaks to their nearest 13C line
    let mut tall: Vec<&&CrossPeak> = real.iter().collect();
    tall.sort_by(|a, b| b.height.abs().total_cmp(&a.height.abs()));
    let mut d: Vec<f64> = tall
        .iter()
        .take(12)
        .filter_map(|p| {
            c.iter()
                .map(|l| p.ppm[0] - l.ppm)
                .min_by(|a, b| a.abs().total_cmp(&b.abs()))
        })
        .collect();
    d.sort_by(f64::total_cmp);
    let offset = d.get(d.len() / 2).copied().unwrap_or(0.0);
    let on = |p: &CrossPeak| {
        let th = 0.03_f64.max(p.width_ppm[1].unwrap_or(0.0));
        (
            h.iter().any(|l| (l.ppm - p.ppm[1]).abs() <= th),
            c.iter().any(|l| (l.ppm - p.ppm[0] + offset).abs() <= f1_tol),
        )
    };
    let strong: Vec<&&CrossPeak> = real.iter().filter(|p| p.snr.unwrap_or(0.0) >= 100.0).collect();
    let both = strong.iter().filter(|p| on(p) == (true, true)).count();
    // protonated carbons of limonene (CDCl3): =CH 120.7, =CH2 108.4, CH 41.1, CH2 30.9, 30.6,
    // 27.9, CH3 23.5, 20.8
    let carbons = [120.7, 108.4, 41.1, 30.9, 30.6, 27.9, 23.5, 20.8];
    let seen = carbons
        .iter()
        .filter(|&&x| real.iter().any(|p| (p.ppm[0] - offset - x).abs() <= 0.6))
        .count();
    eprintln!(
        "limonene JCAMP HSQC: {} cross peaks ({} maxima, noise SD {:.3e}), F1 offset {offset:+.2} ppm, {both}/{} with S/N >= 100 on a 1H and a 13C line, protonated carbons with a cross peak {seen}/8; notes {:?}",
        r.peak_count, r.maxima_count, r.noise_sd, strong.len(), r.notes
    );
    for p in &r.peaks {
        eprintln!(
            "  F1 {:8.3} F2 {:6.3} h {:9.3e} snr {:6.0} maxima {} {:?} {:?}",
            p.ppm[0],
            p.ppm[1],
            p.height,
            p.snr.unwrap_or(0.0),
            p.maxima,
            on(p),
            p.flags
        );
    }
    assert_eq!(seen, 8);
    assert!(both as f64 >= 0.6 * strong.len() as f64, "{both}/{}", strong.len());
}
