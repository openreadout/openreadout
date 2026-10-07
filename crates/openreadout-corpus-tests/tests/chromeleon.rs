//! Chromeleon archives (`.cmbx`) of the corpus, beyond the vendor-export comparisons of the main
//! corpus test (`chromeleon_oracle`): every archive is detected, every signal decodes and equals
//! its sequence file's description (point count, times, scale, recorded minimum and maximum),
//! every 3D field decodes whole and equals its description, every peak Chromeleon stored is
//! reproduced by our integration on our decoded signal, Chromeleon's PDA-extracted channels equal
//! our 3D field at their wavelength, the detector's own UV channels agree with the 3D field, a
//! member damaged as deposited is reported and refused, and MS data points at the embedded
//! Thermo `.raw`.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test chromeleon -- --nocapture`
//!
//! With `VENDOR_RESULTS=<file>`, the stored-peak reproduction of each archive is written as a
//! results line of evidence class `vendor_stored_result` for `cargo xtask assurance-audit
//! refresh` (docs/assurance.md § Vendor-stored results): Chromeleon computed those peaks from
//! the same raw signal, so reproducing them from our decoded signal independently confirms the
//! signal's values, time axis and scaling (`traces`), and nothing else. The line names the
//! features (codec, layout) of the signals Chromeleon integrated, so a pressure or flow signal
//! in the same archive is not confirmed by peaks found on a detector signal.
#![cfg(feature = "corpus")]
#![allow(
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::path::{Path, PathBuf};

use openreadout_core::FileInfo;
use openreadout_core::assurance::Scope;
use openreadout_core::model::TraceInfo;
use openreadout_core::{Dataset, FormatReader, Registry};

fn corpus_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/files"),
        PathBuf::from,
    )
}

/// Fewest stored peaks an archive must have, all reproduced (area and height within 1e-6
/// relative), before its reproduction counts as evidence (`vendor_stored_result`).
const MIN_STORED_PEAKS: usize = 10;

/// (id, traces, of which 3D fields, signals damaged as deposited, stored peaks)
const ARCHIVES: [(&str, usize, usize, usize, usize); 16] = [
    ("cmbx-lauterbach-invivo-cascade", 57, 0, 0, 15),
    ("cmbx-lauterbach-plate-screening", 57, 0, 0, 0),
    ("cmbx-lauterbach-repuox", 33, 0, 0, 0),
    ("cmbx-figshare-milks-sugars", 286, 0, 1, 2800),
    ("cmbx-figshare-sugars-qev", 109, 0, 0, 967),
    // 4 injections + 8 calibration standards of its processing method
    ("cmbx-lim-tsoye-48h", 12, 0, 0, 13),
    ("cmbx-lim-carvone-standards", 33, 0, 0, 68),
    ("cmbx-lim-levodione-calibration", 34, 0, 0, 21),
    ("cmbx-lim-r-carvone-reactions", 28, 0, 0, 60),
    ("cmbx-flavokawain-skrining-20221109", 598, 78, 0, 1230),
    ("cmbx-textiles-2019-088", 26, 2, 0, 186),
    ("cmbx-lim-cyclohexenone-standards", 19, 0, 0, 195),
    ("cmbx-lim-s-carvone-reactions", 18, 0, 0, 18),
    ("cmbx-lim-con-sp22-group-ab", 17, 0, 0, 19),
    ("cmbx-lim-cyclohexanone-reaction", 14, 0, 0, 14),
    ("cmbx-lim-ketoisophorone-standards", 50, 0, 0, 84),
];

/// Stored peaks our integration does not reproduce, per archive, and why. Such an archive's
/// results line is a failure: the disagreement is recorded, not hidden.
const UNREPRODUCED: [(&str, usize, &str); 1] = [(
    "cmbx-lim-ketoisophorone-standards",
    2,
    "injection blank, GC_1: a peak that starts at the first sample, and its rider. Their areas \
     differ from ours by the same amount in opposite directions and the rider's height by 1 %, \
     so the rider's skim line is not the straight line between its stored ends. Not explained.",
)];

fn open(id: &str) -> Option<Box<dyn Dataset>> {
    let path = corpus_dir().join(format!("chromeleon/{id}.cmbx"));
    if !path.exists() {
        eprintln!("skip {id}");
        return None;
    }
    Some(openreadout_chrom::ChromeleonReader.open(&path).unwrap())
}

#[test]
fn every_signal_equals_its_sequence_description() {
    let reg = Registry::new().with(Box::new(openreadout_chrom::ChromeleonReader));
    let (mut signals, mut fields_total, mut peaks_total) = (0usize, 0usize, 0usize);
    let mut results = String::new();
    for (id, traces, fields, damaged, peaks) in ARCHIVES {
        let path = corpus_dir().join(format!("chromeleon/{id}.cmbx"));
        let Some(mut ds) = open(id) else { continue };
        let (_, det) = reg.detect(&path).unwrap();
        assert_eq!(det.format_id, "chromeleon", "{id}");
        let info = ds.info().unwrap();
        assert_eq!(info.traces.len(), traces, "{id}");
        for t in &info.traces {
            assert!(
                t.channels.iter().all(|c| c.unit.is_some()),
                "{id}: {:?} has a channel without unit",
                t.name
            );
            assert!(t.sample_count > 0, "{id}: {:?}", t.name);
        }
        let rep = ds.check().unwrap();
        let bad: Vec<_> = rep
            .findings
            .iter()
            .filter(|f| !matches!(f.severity, openreadout_core::model::Severity::Info))
            .filter(|f| !matches!(f.code.as_str(), "bad_member" | "bad_signal" | "bad_blob"))
            .filter(|f| {
                !(f.code == "vendor_peak_mismatch" && UNREPRODUCED.iter().any(|u| u.0 == id))
            })
            .collect();
        assert!(bad.is_empty(), "{id}: {bad:?}");
        let performed = rep.checks_performed.join(" ");
        let n = traces - fields;
        // injections' signals: their file id names the local start (calibration standards held
        // by a processing method, and data copied later, have another id)
        let timed = info
            .traces
            .iter()
            .filter(|t| t.extra.contains_key("acquired_local"))
            .count()
            - damaged;
        let want = format!(
            "{} of {n} signals decoded; {} equal the sequence file's recorded minimum and maximum; {timed} start times",
            n - damaged,
            n - damaged,
        );
        assert!(performed.contains(&want), "{id}: {performed}");
        if fields > 0 {
            let want = format!("spectral fields (3D): {fields} of {fields} decoded whole");
            assert!(performed.contains(&want), "{id}: {performed}");
        }
        if peaks > 0 {
            let missing = UNREPRODUCED.iter().find(|u| u.0 == id).map_or(0, |u| u.1);
            let want = format!("{} of {peaks} stored peaks reproduced", peaks - missing);
            if peaks >= MIN_STORED_PEAKS {
                let status = if missing == 0 && performed.contains(&want) {
                    "pass"
                } else {
                    "FAIL"
                };
                results.push_str(
                    &serde_json::json!({
                        "id": id, "format": "chromeleon", "status": status, "independent": true,
                        "compared": ["traces"], "evidence": "vendor_stored_result",
                        "features": integrated_features(ds.as_mut(), &info),
                        "detail": format!("{peaks} stored peaks: area and height within 1e-6 relative")
                    })
                    .to_string(),
                );
                results.push('\n');
            }
            assert!(performed.contains(&want), "{id}: {performed}");
            let t = info
                .tables
                .iter()
                .find(|t| t.name.as_deref() == Some("vendor_peaks"))
                .expect("vendor_peaks");
            assert_eq!(t.row_count as usize, peaks, "{id}");
        }
        signals += n - damaged;
        fields_total += fields;
        peaks_total += peaks;
        eprintln!(
            "{id}: {} signals, {fields} fields, {peaks} stored peaks",
            n - damaged
        );
    }
    eprintln!(
        "{signals} signals and {fields_total} 3D fields decoded and consistent; {peaks_total} stored peaks reproduced"
    );
    if let Ok(p) = std::env::var("VENDOR_RESULTS") {
        std::fs::write(p, results).unwrap();
    }
}

/// The trace features (codec, layout, ...) of the signals Chromeleon integrated, as the
/// assurance profile names them: stored peaks confirm those signals and no others, so the
/// results line lists them (`features`) and the audit validates only these.
fn integrated_features(ds: &mut dyn Dataset, info: &FileInfo) -> Vec<[String; 2]> {
    let Some(t) = info
        .tables
        .iter()
        .find(|t| t.name.as_deref() == Some("vendor_peaks"))
    else {
        return Vec::new();
    };
    let rows = ds.read_table(t.index, 0, t.row_count).unwrap();
    let integrated: std::collections::BTreeSet<u64> = rows.columns[0]
        .iter()
        .filter(|v| v.is_finite() && **v >= 0.0)
        .map(|v| v.round() as u64)
        .collect();
    let mut sub = info.clone();
    sub.traces
        .retain(|t| integrated.contains(&u64::from(t.index)));
    sub.tables.clear();
    let profile = openreadout_chrom::ChromeleonReader
        .assurance()
        .expect("chromeleon has an assurance profile");
    let mut out: Vec<[String; 2]> = (profile.observe)(&sub)
        .features
        .into_iter()
        .filter(|f| f.scope.contains(&Scope::Traces))
        .map(|f| [f.kind.as_str().to_string(), f.value])
        .collect();
    out.sort();
    out.dedup();
    out
}

fn trace<'a>(info: &'a [TraceInfo], name: &str) -> &'a TraceInfo {
    info.iter()
        .find(|t| t.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no trace {name}"))
}

fn read(ds: &mut dyn Dataset, t: &TraceInfo) -> Vec<Vec<f64>> {
    ds.read_trace(t.index, 0, 0, t.sample_count)
        .unwrap()
        .channels
}

/// Chromeleon's PDA-extracted channels (EXT350NM, …, stored as their own signals) are the 3D
/// field at the grid wavelength nearest theirs, bit for bit.
#[test]
fn extracted_channels_equal_the_3d_field() {
    let Some(mut ds) = open("cmbx-textiles-2019-088") else {
        return;
    };
    let info = ds.info().unwrap();
    let mut points = 0usize;
    for inj in ["2019-088-058-System_A", "2019-088-058-System_B"] {
        let field = trace(&info.traces, &format!("{inj} / 3DField"));
        let cube = read(ds.as_mut(), field);
        let waves: Vec<f64> = field
            .channels
            .iter()
            .map(|c| c.extra["wavelength_nm"].as_f64().unwrap())
            .collect();
        for ext in ["EXT350NM", "EXT450NM", "EXT600NM"] {
            let t = trace(&info.traces, &format!("{inj} / {ext}"));
            let w = t.extra["wavelength_nm"].as_f64().unwrap();
            let c = (0..waves.len())
                .min_by(|&a, &b| (waves[a] - w).abs().total_cmp(&(waves[b] - w).abs()))
                .unwrap();
            let ch = read(ds.as_mut(), t).remove(0);
            assert_eq!(ch.len(), cube[c].len(), "{inj} {ext}");
            assert!(
                ch == cube[c],
                "{inj} {ext}: not equal to the 3D field at {} nm",
                waves[c]
            );
            points += ch.len();
        }
    }
    eprintln!("6 extracted channels, {points} points equal to the 3D field");
}

/// The DAD's UV channels (recorded separately by the detector, 4 nm bandwidth) against the 3D
/// field averaged over the same band: a straight line of slope 1 within 1 %, r > 0.9999.
#[test]
fn detector_channels_agree_with_the_3d_field() {
    let Some(mut ds) = open("cmbx-flavokawain-skrining-20221109") else {
        return;
    };
    let info = ds.info().unwrap();
    let field = &info.traces[info
        .traces
        .iter()
        .position(|t| t.extra.contains_key("spectral_field"))
        .unwrap()];
    let inj = field.extra["injection_index"].as_u64().unwrap();
    let cube = read(ds.as_mut(), field);
    let waves: Vec<f64> = field
        .channels
        .iter()
        .map(|c| c.extra["wavelength_nm"].as_f64().unwrap())
        .collect();
    let mut compared = 0;
    for t in info.traces.iter().filter(|t| {
        t.extra["injection_index"].as_u64() == Some(inj)
            && t.extra.contains_key("wavelength_nm")
            && t.extra.get("encoding").and_then(|e| e.as_str()) == Some("PtsLL2Df")
    }) {
        let wavelength = t.extra["wavelength_nm"].as_f64().unwrap();
        let band: Vec<usize> = (0..waves.len())
            .filter(|&k| (waves[k] - wavelength).abs() <= 2.0 + 1e-9)
            .collect();
        if band.len() < 5 {
            continue; // outside the field's range (210 nm)
        }
        let ch = read(ds.as_mut(), t).remove(0);
        // the channel has a point at t = 0 that the field lacks: spectrum i is point i + 1
        let first = t.extra["axis"]["first"].as_f64().unwrap();
        let ffirst = field.extra["axis"]["first"].as_f64().unwrap();
        let step = t.extra["axis"]["step"].as_f64().unwrap();
        let shift = ((ffirst - first) / step).round() as usize;
        let (mut x, mut y) = (Vec::new(), Vec::new());
        for (i, &v) in ch.iter().skip(shift).take(cube[0].len()).enumerate() {
            x.push(band.iter().map(|&k| cube[k][i]).sum::<f64>() / band.len() as f64);
            y.push(v);
        }
        let n = x.len() as f64;
        let (mx, my) = (x.iter().sum::<f64>() / n, y.iter().sum::<f64>() / n);
        let sxx: f64 = x.iter().map(|a| (a - mx).powi(2)).sum();
        let syy: f64 = y.iter().map(|b| (b - my).powi(2)).sum();
        let sxy: f64 = x.iter().zip(&y).map(|(a, b)| (a - mx) * (b - my)).sum();
        let (slope, r) = (sxy / sxx, sxy / (sxx * syy).sqrt());
        assert!(
            (slope - 1.0).abs() < 0.01 && r > 0.9999,
            "{:?}: slope {slope}, r {r}",
            t.name
        );
        compared += 1;
    }
    assert!(compared >= 4, "{compared} channels compared");
    eprintln!("{compared} UV channels agree with the 3D field");
}

/// `85_140128179.raw` of the milk-sugars archive fails its CRC-32 as deposited: `check` says so
/// and reading that trace is a corrupt-file error (exit 4), not values.
#[test]
fn a_damaged_member_is_reported_and_refused() {
    let Some(mut ds) = open("cmbx-figshare-milks-sugars") else {
        return;
    };
    let info = ds.info().unwrap();
    let t = info
        .traces
        .iter()
        .find(|t| t.extra.get("member").and_then(|v| v.as_str()) == Some("85_140128179.raw"))
        .unwrap();
    let e = ds.read_trace(t.index, 0, 0, 10).unwrap_err();
    assert_eq!(e.exit_code(), 4, "{e}");
    let rep = ds.check().unwrap();
    assert!(!rep.ok);
    assert!(
        rep.findings
            .iter()
            .any(|f| f.code == "bad_member" && f.message.contains("85_140128179.raw"))
    );
}

#[test]
fn ms_data_points_at_the_embedded_raw_file() {
    let Some(ds) = open("cmbx-baobab-volatiles-ms") else {
        return;
    };
    let info = ds.info().unwrap();
    assert!(info.traces.is_empty());
    assert!(
        info.notes
            .iter()
            .any(|n| n.contains("210905_0.raw_2.raw") && n.contains("thermo-raw")),
        "{:?}",
        info.notes
    );
}
