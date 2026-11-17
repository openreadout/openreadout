//! 2D NMR on the command line: cross peaks from `analyze nmr-peaks`, tables, contour previews,
//! slices and projections, and the matrix export, on a small synthetic Bruker HSQC (and on
//! corpus spectra when they are present).

use std::path::{Path, PathBuf};

use crate::common::{bin, corpus, json};

/// A processed Bruker 2D spectrum (`pdata/1/2rr`, 32 F1 × 64 F2 points, ¹³C × ¹H): positive
/// peaks at (110 ppm, 6.875 ppm) and (135 ppm, 2.19 ppm), a negative one at (50 ppm, 3.75
/// ppm), on reproducible noise.
pub fn tiny_bruker_2d(dir: &Path) -> PathBuf {
    let exp = dir.join("hsqc").join("7");
    std::fs::create_dir_all(exp.join("pdata/1")).unwrap();
    let params = |pairs: &[(&str, &str)]| {
        let mut s = String::from("##TITLE= Parameter file, TopSpin 3.6.2\n##JCAMPDX= 5.0\n");
        for (k, v) in pairs {
            s.push_str(&format!("##${k}= {v}\n"));
        }
        s + "##END=\n"
    };
    let w =
        |name: &str, pairs: &[(&str, &str)]| std::fs::write(exp.join(name), params(pairs)).unwrap();
    w(
        "acqus",
        &[
            ("TD", "128"),
            ("AQ_mod", "3"),
            ("NC", "0"),
            ("SW_h", "4000"),
            ("NUC1", "<1H>"),
            ("SFO1", "400.1"),
        ],
    );
    w(
        "acqu2s",
        &[
            ("TD", "64"),
            ("SW_h", "16000"),
            ("NUC1", "<13C>"),
            ("SFO1", "100.6"),
        ],
    );
    w(
        "pdata/1/procs",
        &[
            ("SI", "64"),
            ("XDIM", "64"),
            ("NC_proc", "0"),
            ("OFFSET", "10"),
            ("SW_p", "4000"),
            ("SF", "400"),
            ("AXNUC", "<1H>"),
            ("BYTORDP", "0"),
            ("DTYPP", "0"),
        ],
    );
    w(
        "pdata/1/proc2s",
        &[
            ("SI", "32"),
            ("XDIM", "32"),
            ("NC_proc", "0"),
            ("OFFSET", "160"),
            ("SW_p", "16000"),
            ("SF", "100"),
            ("AXNUC", "<13C>"),
            ("BYTORDP", "0"),
            ("DTYPP", "0"),
        ],
    );
    let mut seed: u64 = 12345;
    let mut rnd = || {
        seed = (seed * 1_103_515_245 + 12345) % (1 << 31);
        seed as f64 / f64::from(1u32 << 31) - 0.5
    };
    let mut bytes = Vec::with_capacity(32 * 64 * 4);
    for i in 0..32 {
        for j in 0..64 {
            let mut v = 0.0f64;
            for (ci, cj, h) in [(10.0, 20.0, 1e5), (22.0, 40.0, -6e4), (5.0, 50.0, 4e4)] {
                let (di, dj) = (f64::from(i) - ci, f64::from(j) - cj);
                v += h * (-(di * di) / 1.5 - (dj * dj) / 2.0).exp();
            }
            v += 400.0 * (rnd() + rnd() + rnd());
            bytes.extend_from_slice(&(v.round() as i32).to_le_bytes());
        }
    }
    std::fs::write(exp.join("pdata/1/2rr"), bytes).unwrap();
    exp
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn cross_peaks_with_signs_ranges_and_boxes() {
    let dir = tempfile::tempdir().unwrap();
    let exp = tiny_bruker_2d(dir.path());
    let out = bin()
        .args(["analyze", "nmr-peaks", "--json", s(&exp)])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let d = &json(&out)["data"];
    assert_eq!(d["shape"], serde_json::json!([32, 64]));
    assert_eq!(d["nuclei"], serde_json::json!(["13C", "1H"]));
    assert_eq!(d["peak_count"], 3);
    let peaks = d["peaks"].as_array().unwrap();
    // high F2 shift first
    let f2: Vec<f64> = peaks
        .iter()
        .map(|p| p["ppm"][1].as_f64().unwrap())
        .collect();
    assert!(f2.windows(2).all(|w| w[0] >= w[1]), "{f2:?}");
    assert!((peaks[0]["ppm"][0].as_f64().unwrap() - 110.0).abs() < 0.5);
    assert!((peaks[0]["ppm"][1].as_f64().unwrap() - 6.875).abs() < 0.02);
    assert_eq!(peaks[1]["sign"], "negative");
    assert!(peaks[1]["volume"].as_f64().unwrap() < 0.0);
    assert_eq!(d["main_peak"]["sign"], "positive");
    // --no-negative leaves the CH2 out; per-dimension ranges limit the read
    let out = bin()
        .args(["analyze", "nmr-peaks", "--json", "--no-negative", s(&exp)])
        .output()
        .unwrap();
    assert_eq!(json(&out)["data"]["peak_count"], 2);
    let out = bin()
        .args([
            "analyze",
            "nmr-peaks",
            "--json",
            "--range-ppm",
            "f1=140:100",
            "--range-ppm",
            "f2=8:5",
            "--integrate",
            "f1=120:100,f2=7.5:6.2",
            "--integrate",
            "140:130,3:1.5",
            "--integral-reference",
            "0=1",
            s(&exp),
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let d = &json(&out)["data"];
    assert_eq!(d["peak_count"], 1);
    let g = d["integrals"].as_array().unwrap();
    assert_eq!(g.len(), 2);
    assert_eq!(g[0]["normalized"], 1.0);
    assert!(g[1]["normalized"].as_f64().unwrap() > 0.1);
}

#[test]
fn one_dimensional_views_of_a_2d_spectrum() {
    let dir = tempfile::tempdir().unwrap();
    let exp = tiny_bruker_2d(dir.path());
    // a row by sweep, and by slice
    let out = bin()
        .args(["analyze", "nmr-peaks", "--json", "--sweep", "10", s(&exp)])
        .output()
        .unwrap();
    let d = &json(&out)["data"];
    assert_eq!(d["sweep"], 10);
    assert!(d["axis"].is_object() && d.get("axes").is_none());
    assert!((d["main_peak"]["ppm"].as_f64().unwrap() - 6.875).abs() < 0.1);
    let out = bin()
        .args([
            "analyze",
            "nmr-peaks",
            "--json",
            "--slice",
            "column:3.75",
            "--negative",
            s(&exp),
        ])
        .output()
        .unwrap();
    let d = &json(&out)["data"];
    assert_eq!(d["slice"], "column:3.75");
    assert!(
        d["peaks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| (p["ppm"].as_f64().unwrap() - 50.0).abs() < 1.0
                && p["height"].as_f64().unwrap() < 0.0)
    );
    // trace a projection
    let out = bin()
        .args([
            "trace",
            "--json",
            "--slice",
            "skyline:f2",
            "--max-samples",
            "64",
            s(&exp),
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let d = &json(&out)["data"];
    assert_eq!(d["sweep_sample_count"], 64);
    assert!(
        (d["channels"][0]["stats"]["argmax_axis_value"]
            .as_f64()
            .unwrap()
            - 6.875)
            .abs()
            < 0.1
    );
}

#[test]
fn tables_and_the_matrix_export() {
    let dir = tempfile::tempdir().unwrap();
    let exp = tiny_bruker_2d(dir.path());
    let csv = dir.path().join("peaks.csv");
    let out = bin()
        .args(["analyze", "nmr-peaks", s(&exp), "-o", s(&csv)])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&csv).unwrap();
    let header = text.lines().next().unwrap();
    assert!(
        header.starts_with("peak,f1_ppm,f2_ppm,height,sign"),
        "{header}"
    );
    assert_eq!(text.lines().count(), 4);
    // the matrix, one row per point with both shift axes
    let m = dir.path().join("hsqc.csv");
    let out = bin()
        .args(["export", "--format", "csv", s(&exp), "-o", s(&m)])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&m).unwrap();
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("f1_ppm,f2_ppm,intensity"));
    assert_eq!(lines.count(), 32 * 64);
    // a slice as a trace
    let row = dir.path().join("row.csv");
    let out = bin()
        .args([
            "export",
            "--format",
            "csv",
            "--slice",
            "row:110",
            s(&exp),
            "-o",
            s(&row),
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(std::fs::read_to_string(&row).unwrap().lines().count(), 65);
}

#[test]
fn contour_preview_structure() {
    let dir = tempfile::tempdir().unwrap();
    let exp = tiny_bruker_2d(dir.path());
    let svg = dir.path().join("p.svg");
    let out = bin()
        .args([
            "preview",
            "--json",
            s(&exp),
            "-o",
            s(&svg),
            "--max-size",
            "600",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let d = &json(&out)["data"];
    assert_eq!(d["kind"], "nmr_2d");
    let p = &d["nmr_2d"];
    assert_eq!(p["negative"], true);
    // both axes run high to low: F2 left to right, F1 bottom to top
    assert!(p["axes"][1]["first"].as_f64().unwrap() > p["axes"][1]["last"].as_f64().unwrap());
    assert_eq!(p["axes"][0]["nucleus"], "13C");
    let panel = &d["panels"][0];
    assert!(panel["x"]["reversed"].as_bool().unwrap_or(true));
    let levels = p["levels"].as_array().unwrap();
    assert!(levels.len() >= 3);
    let svg = std::fs::read_to_string(&svg).unwrap();
    assert!(svg.contains("data-series=\"contours positive\""));
    assert!(svg.contains("data-series=\"contours negative\""));
    assert!(svg.contains("data-series=\"projection f2\""));
    // positive contours in ink, negative ones in the accent colour
    let group = |name: &str| {
        let a = svg.find(&format!("data-series=\"{name}\"")).unwrap();
        let b = svg[a..].find("</g>").unwrap() + a;
        svg[a..b].to_string()
    };
    // stroke colours of a group: greyish ink for positive, reddish for negative
    let reds = |g: &str| -> Vec<(i64, i64, i64)> {
        g.match_indices("stroke=\"#")
            .map(|(i, _)| {
                let h = &g[i + 9..i + 15];
                let c = |k: usize| i64::from_str_radix(&h[k..k + 2], 16).unwrap();
                (c(0), c(2), c(4))
            })
            .collect()
    };
    let pos = reds(&group("contours positive"));
    let neg = reds(&group("contours negative"));
    assert!(!pos.is_empty() && !neg.is_empty());
    assert!(
        pos.iter()
            .all(|(r, g, b)| (r - g).abs() < 12 && (g - b).abs() < 12),
        "{pos:?}"
    );
    assert!(neg.iter().all(|(r, g, b)| r > g && r > b), "{neg:?}");
    // zoom and marks
    let png = dir.path().join("z.png");
    let out = bin()
        .args([
            "preview",
            "--json",
            s(&exp),
            "-o",
            s(&png),
            "--x-range",
            "8:5",
            "--y-range",
            "130:90",
            "--marks",
            "peaks",
        ])
        .output()
        .unwrap();
    let d = &json(&out)["data"];
    assert_eq!(d["marks"].as_array().unwrap().len(), 1);
    assert_eq!(d["marks"][0]["kind"], "cross_peak");
}

#[test]
fn refusals_carry_hints() {
    let dir = tempfile::tempdir().unwrap();
    let exp = tiny_bruker_2d(dir.path());
    let out = bin()
        .args([
            "analyze",
            "nmr-peaks",
            "--json",
            "--max-points",
            "100",
            s(&exp),
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(6));
    let e = &json(&out)["error"];
    assert!(e["hint"].as_str().unwrap().contains("--range-ppm f1="));
    for bad in [
        vec!["--range-ppm", "f9=1:2"],
        vec!["--range-ppm", "x"],
        vec!["--integrate", "f1=1:2,3:4"],
        vec!["--slice", "diagonal:1"],
        vec!["--integrate", "7:6"],
    ] {
        let out = bin()
            .args(["analyze", "nmr-peaks", "--json"])
            .args(&bad)
            .arg(s(&exp))
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(2),
            "{bad:?}: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert!(json(&out)["error"]["hint"].is_string());
    }
    // a 2D FID with no transformable data: --from fid with no ser is a clean refusal
    let out = bin()
        .args(["analyze", "nmr-peaks", "--json", "--from", "fid", s(&exp)])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(6));
    let out = bin()
        .args([
            "analyze",
            "nmr-peaks",
            "--json",
            "--nus",
            "unknown-mode",
            s(&exp),
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(6));
    // a truncated 2rr is corrupt, not a panic
    let rr = exp.join("pdata/1/2rr");
    let bytes = std::fs::read(&rr).unwrap();
    std::fs::write(&rr, &bytes[..bytes.len() / 3]).unwrap();
    let out = bin()
        .args(["analyze", "nmr-peaks", "--json", s(&exp)])
        .output()
        .unwrap();
    assert!(matches!(out.status.code(), Some(4 | 6)), "{:?}", out.status);
    let out = bin()
        .args([
            "preview",
            "--json",
            s(&exp),
            "-o",
            s(&dir.path().join("t.png")),
        ])
        .output()
        .unwrap();
    assert!(
        matches!(out.status.code(), Some(0 | 4 | 6)),
        "{:?}",
        out.status
    );
}

/// The report of the synthetic spectrum, compared with `tests/snapshots/nmr2d-report.json`
/// (`SNAPSHOT_ACCEPT=1` rewrites it).
#[test]
fn report_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let exp = tiny_bruker_2d(dir.path());
    let out = bin()
        .args(["analyze", "nmr-peaks", "--json", "--compact", s(&exp)])
        .output()
        .unwrap();
    let mut d = json(&out)["data"].clone();
    // the path is the temporary directory's
    d["path"] = serde_json::json!("hsqc/7");
    let round = |v: &mut serde_json::Value| {
        fn r(v: &mut serde_json::Value) {
            match v {
                serde_json::Value::Number(n) if n.is_f64() => {
                    let x = n.as_f64().unwrap();
                    let k = 10f64.powi(6 - x.abs().log10().ceil().max(-12.0) as i32);
                    *v = serde_json::json!((x * k).round() / k);
                }
                serde_json::Value::Array(a) => a.iter_mut().for_each(r),
                serde_json::Value::Object(o) => o.values_mut().for_each(r),
                _ => {}
            }
        }
        r(v);
    };
    round(&mut d);
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/nmr2d-report.json");
    let text = serde_json::to_string_pretty(&d).unwrap() + "\n";
    if std::env::var_os("SNAPSHOT_ACCEPT").is_some() || !path.exists() {
        std::fs::write(&path, &text).unwrap();
        return;
    }
    // a Windows checkout may store the snapshot with CRLF line endings
    let want = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\r\n", "\n");
    assert_eq!(
        text, want,
        "the 2D report changed; review, then rerun with SNAPSHOT_ACCEPT=1"
    );
}

/// Corpus spectra: a COSY's cross peaks sit on both sides of the diagonal, and the FID
/// processed here gives a spectrum of the same shape as the stored one.
#[test]
fn corpus_cosy_stored_and_from_the_fid() {
    let Some(cosy) = corpus("nmrxiv-s1407-bundle/COSY") else {
        return;
    };
    let out = bin()
        .args(["analyze", "nmr-peaks", "--json", s(&cosy)])
        .output()
        .unwrap();
    let d = &json(&out)["data"];
    assert_eq!(d["source"], "processed");
    assert_eq!(d["homonuclear_dimensions"], serde_json::json!([0, 1]));
    let peaks = d["peaks"].as_array().unwrap();
    let diagonal = peaks
        .iter()
        .filter(|p| {
            p["flags"]
                .as_array()
                .is_some_and(|f| f.iter().any(|f| f == "diagonal"))
        })
        .count();
    assert!(diagonal > 10 && diagonal < peaks.len());
    let out = bin()
        .args(["analyze", "nmr-peaks", "--json", "--from", "fid", s(&cosy)])
        .output()
        .unwrap();
    let f = &json(&out)["data"];
    assert_eq!(f["source"], "fid");
    assert_eq!(f["processing"]["dimensions"].as_array().unwrap().len(), 2);
    assert!(f["peak_count"].as_u64().unwrap() > 20);
}
