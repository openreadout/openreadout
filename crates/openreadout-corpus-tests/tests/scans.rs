//! Scan headers (`spectra`, `Dataset::visit_scan_headers`) against third-party ground truth: for
//! every mass-spectrometry file of the corpus with an oracle (`corpus/oracle/<id>.json`:
//! pyteomics on the file itself or on the depositor's mzML/mzXML export of a vendor file;
//! pyimzML, timsrust, rainbow-api/Aston, scipy netCDF for the others), every oracle scan's
//! header must agree scan by scan — scan number (or native id), MS level, retention time,
//! polarity, centroid mode, filter string, precursor m/z, charge, activation, isolation target,
//! 1/K0, imaging position — under the same rules and tolerances as `tests/corpus/` compares decoded
//! spectra. The headers must come from the reader's header-only path (no peaks decoded), and on
//! up to 40 scans per file they must equal the metadata of the decoded spectrum.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test scans -- --nocapture`
//! Env: `OPENREADOUT_CORPUS_DIR` overrides `corpus/files`; `CORPUS_ONLY=<substring>` filters ids.
#![cfg(feature = "corpus")]
#![allow(clippy::float_cmp, clippy::too_many_lines, clippy::cast_precision_loss)]

use std::path::{Path, PathBuf};

use openreadout_core::scans::{HeaderSource, visit_scans};
use openreadout_core::{Dataset, Registry, ScanHeader, SpectrumView};
use serde::Deserialize;

const MS_FORMATS: &[&str] = &[
    "thermo-raw",
    "mzml",
    "mzxml",
    "mzmlb",
    "imzml",
    "bruker-tdf",
    "agilent-masshunter",
    "waters-raw",
    "sciex-wiff",
    "andi-chrom",
    "chemstation",
];

#[derive(Deserialize)]
struct Manifest {
    file: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    id: String,
    format: String,
    filename: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    oracle_skip: Option<String>,
    #[serde(default)]
    precursor_tolerance: Option<f64>,
    #[serde(default)]
    peaks_not_compared: Option<String>,
    /// The oracle converts 1/K0 differently (see `tests/corpus/`): not compared.
    #[serde(default)]
    mz_not_compared: Option<String>,
    /// The export names spectra with ids of its own (see `tests/corpus/`): not compared.
    #[serde(default)]
    native_id_not_compared: Option<String>,
    /// The export reports a precursor and activation the file does not hold (see
    /// `tests/corpus/`): neither is compared.
    #[serde(default)]
    precursor_not_compared: Option<String>,
}

#[derive(Deserialize)]
struct Oracle {
    #[serde(default)]
    spectra: Option<OracleSpectra>,
}

#[derive(Deserialize)]
struct OracleSpectra {
    /// The oracle addresses spectra by position (it read this file, or an export in the
    /// reader's order), not by scan number.
    #[serde(default)]
    by_index: bool,
    #[serde(default)]
    scans: Vec<OracleScan>,
}

#[derive(Deserialize)]
struct OracleScan {
    index: u64,
    scan_number: u64,
    #[serde(default)]
    native_id: Option<String>,
    ms_level: u32,
    rt_s: Option<f64>,
    #[serde(default)]
    rt_tolerance_s: Option<f64>,
    #[serde(default)]
    polarity: Option<String>,
    #[serde(default)]
    centroided: Option<bool>,
    #[serde(default)]
    filter: Option<String>,
    #[serde(default)]
    precursor_mz: Option<f64>,
    #[serde(default)]
    precursor_charge: Option<i32>,
    #[serde(default)]
    activation: Option<serde_json::Value>,
    #[serde(default)]
    isolation_target_mz: Option<f64>,
    #[serde(default)]
    collision_energy: Option<f64>,
    #[serde(default)]
    inverse_reduced_mobility: Option<f64>,
    #[serde(default)]
    position: Option<Vec<f64>>,
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn registry() -> Registry {
    Registry::new()
        .with(Box::new(openreadout_thermo::ThermoRawReader))
        .with(Box::new(openreadout_agilent_ms::AgilentMsReader))
        .with(Box::new(openreadout_sciex::SciexWiffReader))
        .with(Box::new(openreadout_chrom::ChemStationReader))
        .with(Box::new(openreadout_chrom::AndiReader))
        .with(Box::new(openreadout_waters::WatersRawReader))
        .with(Box::new(openreadout_mzml::ImzmlReader))
        .with(Box::new(openreadout_mzml::MzmlReader))
        .with(Box::new(openreadout_mzml::MzxmlReader))
        .with(Box::new(openreadout_mzml::MzmlbReader))
        .with(Box::new(openreadout_bruker_tims::BrukerTimsReader))
}

fn rel_close(a: f64, b: f64, rel: f64) -> bool {
    (a - b).abs() <= rel * a.abs().max(b.abs()).max(1e-300)
}

/// `hcd`, `cid` or `other` of an activation (ours upper case; the oracle's CV names or words).
fn bucket(s: &str) -> &'static str {
    let s = s.to_ascii_lowercase();
    if s.contains("beam-type") || s.contains("hcd") || s.contains("higher energy") {
        "hcd"
    } else if s.contains("collision-induced") || s.contains("cid") {
        "cid"
    } else {
        "other"
    }
}

/// Differences between our header and an oracle scan (the rules of `corpus/spectra.rs::check_spectra`;
/// `same_file`: the oracle read this very file, so its centroid flag is the file's, where an
/// export's reflects the converter's peak picking).
fn compare(h: &ScanHeader, s: &OracleScan, e: &Entry, same_file: bool) -> Vec<String> {
    let mut m = Vec::new();
    if let (Some(theirs), Some(ours)) = (&s.native_id, &h.native_id)
        && !theirs.contains("scan=")
    {
        if theirs != ours && e.native_id_not_compared.is_none() {
            m.push(format!("native id {ours} != {theirs}"));
        }
    } else if h.scan_number != s.scan_number {
        m.push(format!(
            "scan number {} != {}",
            h.scan_number, s.scan_number
        ));
    }
    if h.ms_level != s.ms_level {
        m.push(format!("ms level {} != {}", h.ms_level, s.ms_level));
    }
    if let Some(p) = &s.polarity
        && &h.polarity != p
    {
        m.push(format!("polarity {} != {p}", h.polarity));
    }
    if let Some(c) = s.centroided
        && h.centroided != c
        && e.peaks_not_compared.is_none()
        && same_file
    {
        m.push(format!("centroided {} != {c}", h.centroided));
    }
    if let Some(rt) = s.rt_s
        && h.rt_s
            .is_none_or(|o| (o - rt).abs() > s.rt_tolerance_s.unwrap_or(0.0).max(1e-3) + 1e-9)
    {
        m.push(format!("rt {:?} != {rt}", h.rt_s));
    }
    if let Some(f) = &s.filter
        && h.scan_filter.as_deref() != Some(f.as_str())
    {
        m.push(format!("filter {:?} != {f:?}", h.scan_filter));
    }
    if let Some(p) = s.precursor_mz
        && e.precursor_not_compared.is_none()
        && !h
            .precursor_mz
            .into_iter()
            // an MS^n or multiplexed scan lists every precursor in `extra.precursors`
            .chain(
                h.extra
                    .get("precursors")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(serde_json::Value::as_f64),
            )
            .any(|q| rel_close(q, p, e.precursor_tolerance.unwrap_or(1e-6)))
    {
        m.push(format!("precursor {:?} != {p}", h.precursor_mz));
    }
    if let Some(c) = s.precursor_charge
        && c != 0
        && h.precursor_charge != Some(c)
    {
        m.push(format!("charge {:?} != {c}", h.precursor_charge));
    }
    if let Some(act) = &s.activation
        && !act.is_null()
        && e.precursor_not_compared.is_none()
    {
        let theirs = bucket(&act.to_string());
        let ours = h.activation.as_deref().map_or_else(
            || {
                h.scan_filter
                    .as_deref()
                    .and_then(|f| f.split('@').nth(1))
                    .map_or("none", bucket)
            },
            bucket,
        );
        let staged = h.extra.contains_key("precursors")
            && h.scan_filter
                .as_deref()
                .is_some_and(|f| f.contains(&format!("@{theirs}")));
        if ours != theirs && !staged {
            m.push(format!("activation {ours} != {theirs}"));
        }
    }
    // A staged or multiplexed scan lists every stage's window (`extra.precursor_isolation_windows_mz`)
    // and energy (`extra.collision_energies`); exports differ in which stage they name.
    let stage_windows: Vec<[f64; 2]> = h
        .extra
        .get("precursor_isolation_windows_mz")
        .and_then(|v| serde_json::from_value::<Vec<Option<[f64; 2]>>>(v.clone()).ok())
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .collect();
    let stage_energies: Vec<f64> = h
        .extra
        .get("collision_energies")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    if let (Some(t), Some([lo, hi])) = (s.isolation_target_mz, h.isolation_window_mz)
        && !(lo - 1e-6..=hi + 1e-6).contains(&t)
        && !stage_windows
            .iter()
            .any(|[a, b]| (a - 1e-6..=b + 1e-6).contains(&t))
    {
        m.push(format!(
            "isolation window [{lo}, {hi}] excludes the target {t}"
        ));
    }
    // Sciex records a negative-mode collision energy with its sign (-30 V); exports list the
    // magnitude.
    if let (Some(ce), Some(ours)) = (s.collision_energy, h.collision_energy)
        && !rel_close(ce.abs(), ours.abs(), 1e-6)
        && !stage_energies
            .iter()
            .any(|e| rel_close(ce.abs(), e.abs(), 1e-6))
    {
        m.push(format!("collision energy {ours} != {ce}"));
    }
    if let Some(im) = s.inverse_reduced_mobility
        && e.mz_not_compared.is_none()
        && !h
            .inverse_reduced_mobility
            .is_some_and(|q| rel_close(q, im, 1e-12))
    {
        m.push(format!("1/K0 {:?} != {im}", h.inverse_reduced_mobility));
    }
    if let Some(pos) = &s.position {
        let ours: Option<Vec<f64>> = h
            .extra
            .get("position")
            .and_then(|v| serde_json::from_value(v.clone()).ok());
        let same = ours
            .as_ref()
            .is_some_and(|o| o.len() >= 2 && o.iter().zip(pos).all(|(a, b)| (a - b).abs() < 1e-9));
        if !same {
            m.push(format!("position {ours:?} != {pos:?}"));
        }
    }
    m
}

/// Differences between a header and the metadata of the decoded spectrum: every field the
/// spectrum carries must be the header's (the header may carry more, e.g. stored base peaks).
fn consistent(ds: &mut dyn Dataset, h: &ScanHeader) -> Vec<String> {
    let sp = match ds.read_spectrum_view(0, h.index, SpectrumView::Primary) {
        Ok(sp) => sp,
        Err(e) => return vec![format!("spectrum {}: {e}", h.index)],
    };
    let mut m = Vec::new();
    let mut eq = |what: &str, a: String, b: String| {
        if a != b {
            m.push(format!("{what}: header {a} != spectrum {b}"));
        }
    };
    eq(
        "scan_number",
        h.scan_number.to_string(),
        sp.scan_number.to_string(),
    );
    eq(
        "native_id",
        format!("{:?}", h.native_id),
        format!("{:?}", sp.native_id),
    );
    eq("ms_level", h.ms_level.to_string(), sp.ms_level.to_string());
    eq("rt_s", format!("{:?}", h.rt_s), format!("{:?}", sp.rt_s));
    eq("polarity", h.polarity.clone(), sp.polarity.clone());
    eq(
        "centroided",
        h.centroided.to_string(),
        sp.centroided.to_string(),
    );
    eq(
        "precursor_mz",
        format!("{:?}", h.precursor_mz),
        format!("{:?}", sp.precursor_mz),
    );
    eq(
        "precursor_charge",
        format!("{:?}", h.precursor_charge),
        format!("{:?}", sp.precursor_charge),
    );
    eq(
        "scan_filter",
        format!("{:?}", h.scan_filter),
        format!("{:?}", sp.scan_filter),
    );
    for (what, ours, theirs) in [("activation", h.activation.clone(), sp.activation.clone())] {
        if theirs.is_some() && ours != theirs {
            m.push(format!("{what}: header {ours:?} != spectrum {theirs:?}"));
        }
    }
    for (what, ours, theirs) in [
        ("collision_energy", h.collision_energy, sp.collision_energy),
        (
            "inverse_reduced_mobility",
            h.inverse_reduced_mobility,
            sp.inverse_reduced_mobility,
        ),
        (
            "precursor_intensity",
            h.precursor_intensity,
            sp.precursor_intensity,
        ),
    ] {
        if theirs.is_some() && ours != theirs {
            m.push(format!("{what}: header {ours:?} != spectrum {theirs:?}"));
        }
    }
    if sp.isolation_window_mz.is_some() && h.isolation_window_mz != sp.isolation_window_mz {
        m.push(format!(
            "isolation window: header {:?} != spectrum {:?}",
            h.isolation_window_mz, sp.isolation_window_mz
        ));
    }
    if sp.scan_window_mz.is_some() && h.scan_window_mz != sp.scan_window_mz {
        m.push(format!(
            "scan window: header {:?} != spectrum {:?}",
            h.scan_window_mz, sp.scan_window_mz
        ));
    }
    m
}

#[test]
fn scan_headers_match_oracles() {
    let root = root();
    let manifest: Manifest =
        toml::from_str(&std::fs::read_to_string(root.join("corpus/manifest.toml")).unwrap())
            .unwrap();
    let dir = std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root.join("corpus/files"), PathBuf::from);
    let only = std::env::var("CORPUS_ONLY").ok();
    let reg = registry();
    let (mut files, mut compared, mut checked) = (0usize, 0usize, 0usize);
    let mut per_format: std::collections::BTreeMap<String, (usize, usize)> =
        std::collections::BTreeMap::new();
    let mut problems = Vec::new();
    for e in &manifest.file {
        // Held-out files are never development inputs (docs/benchmark/heldout.md).
        let dev = e.role.is_empty()
            || e.role == "input"
            || (e.role == "oracle-export" && (e.format == "mzml" || e.format == "mzxml"));
        if !dev || e.id.starts_with("ho-") || !MS_FORMATS.contains(&e.format.as_str()) {
            continue;
        }
        if e.oracle_skip.is_some() || only.as_ref().is_some_and(|o| !e.id.contains(o.as_str())) {
            continue;
        }
        let path = dir.join(&e.filename);
        let oracle_path = root.join("corpus/oracle").join(format!("{}.json", e.id));
        if !path.exists() || !openreadout_corpus_tests::oracle_json::exists(&oracle_path) {
            continue;
        }
        let oracle: Oracle = serde_json::from_str(
            &openreadout_corpus_tests::oracle_json::read_to_string(&oracle_path).unwrap(),
        )
        .unwrap();
        let Some(o) = oracle.spectra.filter(|s| !s.scans.is_empty()) else {
            continue;
        };
        let same_file = matches!(e.format.as_str(), "mzml" | "mzxml" | "imzml");
        let by_index = same_file || o.by_index;
        let (_, mut ds) = match reg.open(&path) {
            Ok(v) => v,
            Err(err) => {
                problems.push(format!("{}: open: {err}", e.id));
                continue;
            }
        };
        let mut headers = Vec::new();
        let t0 = std::time::Instant::now();
        let src = match visit_scans(ds.as_mut(), 0, &mut |h| {
            headers.push(h);
            true
        }) {
            Ok(s) => s,
            Err(err) => {
                problems.push(format!("{}: scan headers: {err}", e.id));
                continue;
            }
        };
        let took = t0.elapsed();
        files += 1;
        if src != HeaderSource::Headers {
            problems.push(format!("{}: headers came from decoding spectra", e.id));
        }
        let first_scan = if by_index {
            0
        } else {
            headers.first().map_or(0, |h| h.scan_number)
        };
        let mut bad = 0usize;
        let mut notes = Vec::new();
        for s in &o.scans {
            let at = if by_index {
                s.index
            } else {
                s.scan_number.wrapping_sub(first_scan)
            };
            let Some(h) = usize::try_from(at).ok().and_then(|i| headers.get(i)) else {
                bad += 1;
                if notes.len() < 5 {
                    notes.push(format!("scan {}: no header at index {at}", s.scan_number));
                }
                continue;
            };
            compared += 1;
            let d = compare(h, s, e, same_file);
            if !d.is_empty() {
                bad += 1;
                if notes.len() < 5 {
                    notes.push(format!("scan {}: {}", s.scan_number, d.join("; ")));
                }
            }
        }
        // Header == decoded spectrum on up to 40 scans spread over the run.
        let step = (headers.len() / 40).max(1);
        let mut incons = Vec::new();
        for h in headers.iter().step_by(step).take(40) {
            checked += 1;
            let d = consistent(ds.as_mut(), h);
            if !d.is_empty() && incons.len() < 3 {
                incons.push(format!("index {}: {}", h.index, d.join("; ")));
            }
        }
        let pf = per_format.entry(e.format.clone()).or_default();
        pf.0 += 1;
        pf.1 += o.scans.len();
        println!(
            "{:<5} {:<20} {:<55} {} headers in {:.1} ms, {} oracle scans, {} differ{}",
            if bad == 0 && incons.is_empty() {
                "pass"
            } else {
                "FAIL"
            },
            e.format,
            e.id,
            headers.len(),
            took.as_secs_f64() * 1e3,
            o.scans.len(),
            bad,
            if notes.is_empty() && incons.is_empty() {
                String::new()
            } else {
                format!(
                    ": {}",
                    notes
                        .iter()
                        .chain(&incons)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" | ")
                )
            }
        );
        if bad > 0 {
            problems.push(format!(
                "{}: {bad} of {} scans differ: {}",
                e.id,
                o.scans.len(),
                notes.join(" | ")
            ));
        }
        if !incons.is_empty() {
            problems.push(format!(
                "{}: header != spectrum: {}",
                e.id,
                incons.join(" | ")
            ));
        }
    }
    println!(
        "scans: {files} files, {compared} oracle scans compared, {checked} headers checked against decoded spectra"
    );
    for (f, (n, s)) in &per_format {
        println!("  {f:<20} {n} files, {s} scans");
    }
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}
