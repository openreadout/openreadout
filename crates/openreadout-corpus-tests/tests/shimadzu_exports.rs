//! Shimadzu LabSolutions files against the vendor's own ASCII exports of the same runs (read
//! here from the text, independently of the reader): chromatograms and status traces point for
//! point in the export's units, and LabSolutions' peak tables column for column. Files without
//! an export are checked for agreement between the vendor's peak areas and our traces (`check`
//! must not report `vendor_area_mismatch`).
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test shimadzu_exports -- --nocapture`
#![cfg(feature = "corpus")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::FormatReader;

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

/// Sections of a LabSolutions ASCII export: title → lines (tab-split).
fn sections(path: &Path) -> Vec<(String, Vec<Vec<String>>)> {
    let raw = std::fs::read(path).unwrap();
    let text: String = raw.iter().map(|&b| char::from(b)).collect();
    let mut out: Vec<(String, Vec<Vec<String>>)> = Vec::new();
    for line in text.lines() {
        let l = line.trim_end_matches('\r');
        if l.starts_with('[') && l.ends_with(']') {
            out.push((l[1..l.len() - 1].to_string(), Vec::new()));
        } else if let Some(last) = out.last_mut() {
            last.1.push(l.split('\t').map(str::to_string).collect());
        }
    }
    out
}

/// Printed decimals of a number (half a unit of the last one is the export's rounding).
fn half_unit(s: &str) -> f64 {
    let d = s.split_once('.').map_or(0, |(_, f)| f.len());
    0.5 * 10f64.powi(-i32::try_from(d).unwrap())
}

/// (unit, multiplier, [(time_min, intensity text)]) of a chromatogram/status section.
fn signal_section(lines: &[Vec<String>]) -> (String, f64, Vec<(f64, String)>) {
    let mut unit = String::new();
    let mut mult = 1.0;
    let mut rows = Vec::new();
    let mut body = false;
    for l in lines {
        match l.first().map(String::as_str) {
            Some("Intensity Units") => unit = l[1].clone(),
            Some("Intensity Multiplier") => mult = l[1].parse().unwrap(),
            Some(k) if k.starts_with("R.Time") => body = true,
            Some(t) if body && !t.is_empty() => {
                rows.push((t.parse().unwrap(), l[1].clone()));
            }
            _ => {}
        }
    }
    (unit, mult, rows)
}

fn norm_unit(u: &str) -> String {
    match u {
        "uV" => "µV".into(),
        "C" => "°C".into(),
        _ => u.into(),
    }
}

#[test]
fn traces_and_peak_tables_match_the_ascii_exports() {
    let pairs = [
        ("lcd-streamfind-adc-uv.lcd", "lcd-streamfind-adc-uv.txt"),
        ("lcd-streamfind-karl.lcd", "lcd-streamfind-karl.txt"),
    ];
    let mut total_points = 0usize;
    let mut total_peaks = 0usize;
    for (lcd, txt) in pairs {
        let (lcd, txt) = (corpus_dir().join(lcd), corpus_dir().join(txt));
        if !lcd.exists() || !txt.exists() {
            eprintln!("skip {}: missing", lcd.display());
            continue;
        }
        let mut ds = openreadout_chrom::ShimadzuReader.open(&lcd).unwrap();
        let info = ds.info().unwrap();
        let secs = sections(&txt);
        // older layout: status traces carry no export name; they follow the channel order, as
        // the export's status sections do
        let status_traces: Vec<u32> = info
            .traces
            .iter()
            .filter(|t| t.extra.get("detector").and_then(|v| v.as_str()) == Some("status"))
            .map(|t| t.index)
            .collect();
        let mut status_k = 0usize;
        for (title, lines) in &secs {
            let is_chrom = title.starts_with("LC Chromatogram(");
            let is_status = title.starts_with("LC Status Trace(");
            if !is_chrom && !is_status {
                continue;
            }
            let (unit, mult, rows) = signal_section(lines);
            let t = info
                .traces
                .iter()
                .find(|t| t.extra.get("export_section").and_then(|v| v.as_str()) == Some(title))
                .or_else(|| {
                    is_status
                        .then(|| status_traces.get(status_k))
                        .flatten()
                        .and_then(|&i| info.traces.get(i as usize))
                })
                .unwrap_or_else(|| panic!("{}: no trace for [{title}]", lcd.display()));
            if is_status {
                status_k += 1;
            }
            assert_eq!(
                t.sample_count,
                rows.len() as u64,
                "{}: [{title}] point count",
                lcd.display()
            );
            // the export's value in its units is Intensity × multiplier
            let want_unit = norm_unit(&unit);
            let ours_unit = t.channels[0].unit.clone().unwrap_or_default();
            assert_eq!(ours_unit, want_unit, "{}: [{title}] unit", lcd.display());
            let tr = ds.read_trace(t.index, 0, 0, t.sample_count).unwrap();
            for (k, (time, s)) in rows.iter().enumerate() {
                let want = s.parse::<f64>().unwrap() * mult;
                let got = tr.channels[0][k];
                assert!(
                    (got - want).abs() <= half_unit(s) * mult.abs() + 1e-9,
                    "{}: [{title}] point {k} at {time} min: ours {got}, export {want}",
                    lcd.display()
                );
            }
            total_points += rows.len();
        }
        // LabSolutions' peak tables
        let peaks: BTreeMap<String, Vec<Vec<f64>>> = if let Some(pt) = info
            .tables
            .iter()
            .find(|t| t.name.as_deref() == Some("vendor_peaks"))
        {
            let cats: Vec<String> = pt.columns[0].extra["categories"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c.as_str().unwrap().to_string())
                .collect();
            let tab = ds.read_table(0, 0, pt.row_count).unwrap();
            let mut m: BTreeMap<String, Vec<Vec<f64>>> = BTreeMap::new();
            for i in 0..tab.columns[0].len() {
                let sig = cats[tab.columns[0][i] as usize].clone();
                m.entry(sig)
                    .or_default()
                    .push(tab.columns.iter().map(|c| c[i]).collect());
            }
            m
        } else {
            BTreeMap::new()
        };
        for (title, lines) in &secs {
            let Some(sig) = title
                .strip_prefix("Peak Table(")
                .and_then(|s| s.strip_suffix(')'))
            else {
                continue;
            };
            let Some(header) = lines.iter().position(|l| l[0] == "Peak#") else {
                continue; // `# of Peaks 0`
            };
            let names = &lines[header];
            let col = |n: &str| names.iter().position(|x| x == n).unwrap();
            let ours = peaks.get(sig).cloned().unwrap_or_default();
            let rows: Vec<&Vec<String>> = lines[header + 1..]
                .iter()
                .filter(|l| l[0].parse::<u32>().is_ok())
                .collect();
            assert_eq!(ours.len(), rows.len(), "{}: [{title}] peaks", lcd.display());
            // (export column, our column)
            for (row, o) in rows.iter().zip(&ours) {
                for (name, c) in [
                    ("R.Time", 2),
                    ("I.Time", 3),
                    ("F.Time", 4),
                    ("Area", 5),
                    ("Height", 6),
                    ("k'", 9),
                    ("Plate #", 10),
                    ("Plate Ht.", 11),
                    ("Tailing", 12),
                    ("Resolution", 13),
                ] {
                    let s = &row[col(name)];
                    let want: f64 = s.trim().parse().unwrap();
                    assert!(
                        (o[c] - want).abs() <= half_unit(s.trim()) + 1e-9,
                        "{}: [{title}] peak {} {name}: ours {}, export {want}",
                        lcd.display(),
                        row[0],
                        o[c]
                    );
                }
            }
            total_peaks += rows.len();
        }
    }
    eprintln!("{total_points} exported trace points and {total_peaks} vendor peaks agree");
}

#[test]
fn vendor_peak_areas_agree_with_our_traces() {
    for f in [
        "lcd-streamfind-adc-uv.lcd",
        "zenodo17868549-gp070190p-hplc.lcd",
        "gcd-cct-fs19-214.gcd",
        "zenodo13987390-chiral-11a-ad.lcd",
    ] {
        let p = corpus_dir().join(f);
        if !p.exists() {
            eprintln!("skip {f}: missing");
            continue;
        }
        let mut ds = openreadout_chrom::ShimadzuReader.open(&p).unwrap();
        let info = ds.info().unwrap();
        let n = info.tables.first().map_or(0, |t| t.row_count);
        assert!(n > 0, "{f}: no vendor peaks");
        let rep = ds.check().unwrap();
        let bad: Vec<_> = rep
            .findings
            .iter()
            .filter(|x| x.code == "vendor_area_mismatch")
            .collect();
        assert!(bad.is_empty(), "{f}: {bad:?}");
        eprintln!("{f}: {n} vendor peaks, areas consistent with the traces");
    }
}

/// A later LabSolutions writer (file version 5.01, LCMS-8060NX, MetaboLights MTBLS7425) gives
/// its UV chromatogram's `2D Data Item` entry `DT` 48, the status logs' value: the channel is
/// still named from its export title and scaled by its `Chromatogram Status` record (factor
/// 20000/2^22, divisor 1000, `mV`). No export of these runs exists, so the values themselves are
/// only checked for plausibility (a UV baseline of a few mV).
#[test]
fn dt_48_chromatograms_are_named_and_scaled() {
    for f in [
        "shimadzu-mtbls7425/native-rrnas.lcd",
        "shimadzu-mtbls7425/1_16S_Negative.lcd",
    ] {
        let p = corpus_dir().join(f);
        if !p.exists() {
            eprintln!("skip {f}: missing");
            continue;
        }
        let mut ds = openreadout_chrom::ShimadzuReader.open(&p).unwrap();
        let info = ds.info().unwrap();
        let t = info
            .traces
            .iter()
            .find(|t| {
                t.extra.get("stream").and_then(|v| v.as_str())
                    == Some("LSS Raw Data/Chromatogram Ch1")
            })
            .unwrap_or_else(|| panic!("{f}: no Chromatogram Ch1 trace"));
        assert_eq!(t.name.as_deref(), Some("Detector A-Ch1"), "{f}");
        assert_eq!(t.channels[0].unit.as_deref(), Some("mV"), "{f}");
        let scale = t.extra.get("raw_scale").and_then(serde_json::Value::as_f64);
        assert_eq!(scale, Some(20_000.0 / 4_194_304.0 / 1000.0), "{f}");
        let tr = ds.read_trace(t.index, 0, 0, t.sample_count).unwrap();
        let (lo, hi) = tr.channels[0]
            .iter()
            .fold((f64::MAX, f64::MIN), |(a, b), &v| (a.min(v), b.max(v)));
        assert!(lo > 0.0 && hi < 100.0, "{f}: {lo}..{hi} mV");
        eprintln!(
            "{f}: Detector A-Ch1, {} points, {lo:.3}..{hi:.3} mV",
            t.sample_count
        );
    }
}
