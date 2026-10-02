//! ÄKTA / UNICORN results through the binary: a synthetic UNICORN 7 export (info, trace
//! statistics with the elution volume of the maximum, a volume window, the fraction table,
//! peaks on the time base, CSV export, check) and a zip that is not an export (exit 6).
#![allow(clippy::many_single_char_names)] // builders: f (file), z (zip), o (options), …

use std::path::Path;
use std::process::{Command, Output};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_openreadout"))
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON: {e}\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn run(args: &[&str], file: &Path) -> Output {
    bin().args(args).arg(file).output().unwrap()
}

fn nrbf_floats(v: &[f32]) -> Vec<u8> {
    let mut b = vec![
        0u8, 1, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 1, 0, 0, 0, 0, 0, 0, 0, 15,
    ];
    b.extend_from_slice(&1i32.to_le_bytes());
    b.extend_from_slice(&(v.len() as i32).to_le_bytes());
    b.push(11);
    for x in v {
        b.extend_from_slice(&x.to_le_bytes());
    }
    b.push(11);
    b
}

/// A UNICORN 7 export: one UV curve (a Gaussian peak at 12 ml / 12 min, 1 ml/min, sampled every
/// 0.05 min from 0.05 min) and two fraction marks.
fn export() -> Vec<u8> {
    let n = 400;
    let vol: Vec<f32> = (0..n).map(|i| 0.05 * (i + 1) as f32).collect();
    let amp: Vec<f32> = vol
        .iter()
        .map(|v| 0.5 + 250.0 * (-((v - 12.0) / 0.4).powi(2) / 2.0).exp())
        .collect();
    let mut curve = openreadout_core::zip::zip_bytes(&[
        ("CoordinateData.AmplitudesDataType", b"System.Single[]\r\n"),
        ("CoordinateData.Amplitudes", &nrbf_floats(&amp)),
        ("CoordinateData.VolumesDataType", b"System.Single[]\r\n"),
        ("CoordinateData.Volumes", &nrbf_floats(&vol)),
    ])
    .unwrap();
    curve.extend(std::iter::repeat_n(0u8, 100_000)); // padding past the end record, as UNICORN writes
    let chrom = "<Chromatogram FormatVersion=\"9\" UNICORNVersion=\"7.3.0.473\"><Curves><Curve CurveDataType=\"UV\"><Name>UV 1_280</Name><IsoChroneType>Time</IsoChroneType><DistanceBetweenPoints>0.05</DistanceBetweenPoints><DistanceToStartPoint>0.05</DistanceToStartPoint><TimeUnit>min</TimeUnit><AmplitudeUnit>mAU</AmplitudeUnit><IsOriginalData>true</IsOriginalData><CurveNumber>1</CurveNumber><CurvePoints><CurvePoint><IsFullResolution>true</IsFullResolution><BinaryCurvePointsFileName>Chrom.1_1_True</BinaryCurvePointsFileName></CurvePoint></CurvePoints></Curve></Curves><EventCurves><EventCurve EventCurveType=\"Fraction\"><Events><Event><EventTime>11</EventTime><EventVolume>11</EventVolume><EventText>A1</EventText></Event><Event><EventTime>13</EventTime><EventVolume>13</EventVolume><EventText>Waste</EventText></Event></Events></EventCurve></EventCurves></Chromatogram>";
    openreadout_core::zip::zip_bytes(&[
        ("Result.xml", b"<Result UNICORNVersion=\"7.3.0.473\"><Name>SEC 001</Name><CreatedBy>alice</CreatedBy></Result>"),
        ("Chrom.1.Xml", chrom.as_bytes()),
        ("Chrom.1_1_True", &curve),
    ])
    .unwrap()
}

#[test]
fn unicorn_export_through_the_cli() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("run.zip");
    std::fs::write(&f, export()).unwrap();

    let out = run(&["info", "--json"], &f);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let d = &json(&out)["data"];
    assert_eq!(d["format"]["id"], "cytiva-unicorn-zip");
    assert_eq!(d["traces"][0]["name"], "UV 1_280");
    assert_eq!(d["traces"][0]["sample_count"], 400);
    assert_eq!(
        d["traces"][0]["extra"]["axis"]["quantity"],
        "retention_volume"
    );
    assert_eq!(d["tables"][0]["name"], "fractions");

    // the maximum: its elution volume (axis) and its time
    let out = run(&["trace", "--json"], &f);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let s = &json(&out)["data"]["channels"][0]["stats"];
    assert!(
        (s["argmax_axis_value"].as_f64().unwrap() - 12.0).abs() < 1e-5,
        "{s}"
    );
    assert!(
        (s["argmax_time_s"].as_f64().unwrap() - 720.0).abs() < 1e-6,
        "{s}"
    );
    assert!((s["max"].as_f64().unwrap() - 250.5).abs() < 1e-3, "{s}");

    // a volume window
    let out = run(&["trace", "--json", "--x-range", "15:16"], &f);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v = &json(&out)["data"];
    assert_eq!(v["sample_count"], 21, "{v}");

    // the fraction marks
    let out = run(&["table", "--json"], &f);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let t = json(&out);
    assert_eq!(t["data"]["total_rows"], 2, "{t}");

    // one peak on the time base, at 12 min
    let out = run(&["analyze", "peaks", "--json"], &f);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let p = json(&out);
    let peaks = p["data"]["chromatograms"][0]["peaks"].as_array().unwrap();
    assert_eq!(peaks.len(), 1, "{p}");
    assert!(
        (peaks[0]["rt_min"].as_f64().unwrap() - 12.0).abs() < 0.06,
        "{p}"
    );

    // CSV of the curve: time, volume and value columns
    let csv = dir.path().join("uv.csv");
    let out = bin()
        .args(["export", "--to", "csv", "--trace", "0", "--json", "-o"])
        .arg(&csv)
        .arg(&f)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let text = std::fs::read_to_string(&csv).unwrap();
    assert_eq!(
        text.lines().count(),
        401,
        "{}",
        &text[..text.len().min(300)]
    );

    let out = run(&["check", "--json"], &f);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(json(&out)["data"]["ok"], true);
}

#[test]
fn a_zip_that_is_not_an_export_is_not_claimed() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("notes.zip");
    std::fs::write(
        &f,
        openreadout_core::zip::zip_bytes(&[("Chrom.1_1_True", b"x"), ("readme.txt", b"hello")])
            .unwrap(),
    )
    .unwrap();
    let out = run(&["info", "--json"], &f);
    assert!(!out.status.success());
    let code = out.status.code().unwrap();
    assert!(
        code == 3 || code == 6,
        "exit {code}: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}
