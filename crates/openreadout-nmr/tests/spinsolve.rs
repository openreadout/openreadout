//! Magritek Spinsolve experiment directories built in a temporary directory: layout, parameters,
//! x-axis recognition, and malformed inputs (clean errors, never a panic).
#![allow(clippy::many_single_char_names)] // t/d/b/e/r: temp dir, directory, bytes, extra, report

use std::path::Path;

use openreadout_core::{Error, FormatReader};
use openreadout_nmr::SpinsolveReader;

const ACQU: &str = "b1Freq = 43.451680000000003\nbandwidth = 5\ndwellTime = 200\nexperiment = \"1D PROTON\"\nexpName = \"241220-104035 1D PROTON (ethanol)\"\nfilter = \"no\"\nlowestFrequency = -2294.9080795165255\nnrPnts = 8\nnrScans = 4\nnucleus = \"1H\"\nrxChannel = \"1H\"\nsoftwareVersion = \"2.01.19\"\nspecID = \"SPA712\"\nspecType = \"C43\"\nzf = 1\n";

fn prospa(data_type: u32, dims: [u32; 4], values: &[f32]) -> Vec<u8> {
    let mut b = b"SORPATAD1.1V".to_vec();
    b.extend_from_slice(&data_type.to_le_bytes());
    for d in dims {
        b.extend_from_slice(&d.to_le_bytes());
    }
    for v in values {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b
}

fn experiment(dir: &Path, files: &[(&str, Vec<u8>)]) {
    std::fs::create_dir_all(dir).unwrap();
    for (n, b) in files {
        std::fs::write(dir.join(n), b).unwrap();
    }
}

fn fid_values(rows: usize) -> Vec<f32> {
    (0..rows * 16).map(|i| i as f32 * 0.5 - 3.0).collect()
}

#[test]
fn fid_series_and_parameters() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path().join("241220-104035 1D PROTON (ethanol)");
    experiment(
        &d,
        &[
            ("acqu.par", ACQU.as_bytes().to_vec()),
            ("data.2d", prospa(501, [8, 3, 1, 1], &fid_values(3))),
            (
                "processing.script",
                b"Phase(12.5,0);\nZoom(-2,14);\n".to_vec(),
            ),
            ("spectrum.pt1", b"SORPD1LP....".to_vec()),
        ],
    );
    // the directory, the data file and acqu.par all open the experiment
    for p in [d.clone(), d.join("data.2d"), d.join("acqu.par")] {
        let det = SpinsolveReader.sniff(b"", &p);
        assert!(det.is_some(), "{}", p.display());
    }
    let mut ds = SpinsolveReader.open(&d).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces.len(), 1);
    let tr = &info.traces[0];
    assert_eq!(tr.name.as_deref(), Some("data.2d"));
    assert_eq!((tr.sample_count, tr.sweep_count), (8, 3));
    assert_eq!(tr.channels.len(), 2);
    assert!((tr.sample_rate_hz - 5000.0).abs() < 1e-9);
    let e = &tr.extra;
    assert_eq!(e["kind"], "time_domain");
    assert_eq!(e["nucleus"], "1H");
    assert_eq!(e["acquired_at"], "2024-12-20T10:40:35");
    assert_eq!(e["sample_name"], "ethanol");
    assert_eq!(e["instrument"], "C43");
    assert_eq!(e["instrument_serial"], "SPA712");
    let off = -2_294.908_079_516_525 + 2_500.0;
    assert!((e["carrier_offset_hz"].as_f64().unwrap() - off).abs() < 1e-9);
    assert!(info.notes.iter().any(|n| n.contains("spectrum.pt1")));
    let row = ds.read_trace(0, 2, 1, 3).unwrap();
    let v = fid_values(3);
    assert_eq!(
        row.channels[0],
        vec![f64::from(v[34]), f64::from(v[36]), f64::from(v[38])]
    );
    assert_eq!(
        row.channels[1],
        vec![f64::from(v[35]), f64::from(v[37]), f64::from(v[39])]
    );
    assert!(matches!(ds.read_trace(0, 3, 0, 8), Err(Error::Usage(_))));
    let vendor = ds.vendor_metadata().unwrap();
    assert_eq!(vendor["script_phase_deg"], serde_json::json!([12.5, 0.0]));
    let exp = ds.experiment().unwrap();
    assert_eq!(exp.sample.unwrap().name.as_deref(), Some("ethanol"));
    let r = ds.check().unwrap();
    assert!(
        r.findings
            .iter()
            .all(|f| f.severity != openreadout_core::model::Severity::Error)
    );
}

#[test]
fn x_blocks_become_axes_only_when_they_match() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path().join("exp");
    let n = 8u32;
    // time axis in ms stepping by the dwell time (0.2 ms)
    let mut fid: Vec<f32> = (0..n).map(|i| 0.2 * i as f32).collect();
    fid.extend((0..2 * n).map(|i| i as f32));
    // a ppm axis spanning bandwidth / b1Freq
    let step = 5000.0 / 43.451_68 / f64::from(n);
    let mut spec: Vec<f32> = (0..n)
        .map(|i| (-52.8 + step * f64::from(i)) as f32)
        .collect();
    spec.extend((0..2 * n).map(|i| i as f32));
    // an x block that is neither (echo times)
    let mut echoes: Vec<f32> = (0..n).map(|i| 3.5 * (i + 1) as f32).collect();
    echoes.extend((0..n).map(|i| i as f32));
    experiment(
        &d,
        &[
            ("acqu.par", ACQU.as_bytes().to_vec()),
            ("data.1d", prospa(504, [n, 1, 1, 1], &fid)),
            ("spectrum.1d", prospa(504, [n, 1, 1, 1], &spec)),
            ("echoes.1d", prospa(503, [n, 1, 1, 1], &echoes)),
            ("odd.1d", prospa(500, [n, 1, 1, 1], &[0.0; 8])),
        ],
    );
    let mut ds = SpinsolveReader.open(&d).unwrap();
    let info = ds.info().unwrap();
    let names: Vec<_> = info
        .traces
        .iter()
        .map(|t| t.name.clone().unwrap())
        .collect();
    assert_eq!(names, ["data.1d", "echoes.1d", "spectrum.1d"]);
    assert_eq!(info.traces[0].channels.len(), 2);
    assert_eq!(info.traces[1].channels[0].name, "x");
    assert_eq!(info.traces[2].extra["kind"], "spectrum");
    assert_eq!(info.traces[2].extra["axis"]["unit"], "ppm");
    assert!(info.notes.iter().any(|m| m.contains("odd.1d")));
    let x = ds.read_trace(1, 0, 0, 8).unwrap();
    assert_eq!(x.channels.len(), 2);
    assert!((x.channels[0][1] - 7.0).abs() < 1e-6);
    let s = ds.read_trace(2, 0, 0, 2).unwrap();
    assert_eq!(s.channels, vec![vec![0.0, 2.0], vec![1.0, 3.0]]);
}

#[test]
fn malformed_directories_fail_cleanly() {
    let t = tempfile::tempdir().unwrap();
    // truncated FID: listed with a note, check reports it, reading the missing row is corrupt
    let d = t.path().join("trunc");
    let mut b = prospa(501, [8, 2, 1, 1], &fid_values(2));
    b.truncate(b.len() - 20);
    experiment(
        &d,
        &[("acqu.par", ACQU.as_bytes().to_vec()), ("data.2d", b)],
    );
    let mut ds = SpinsolveReader.open(&d).unwrap();
    let info = ds.info().unwrap();
    assert!(info.notes.iter().any(|n| n.contains("1 of 2 rows")));
    assert!(ds.read_trace(0, 0, 0, 8).is_ok());
    assert!(matches!(
        ds.read_trace(0, 1, 0, 8),
        Err(Error::Corrupt { .. })
    ));
    let r = ds.check().unwrap();
    assert!(r.findings.iter().any(|f| f.code == "truncated"));

    // overflowing sizes, a header cut short, parameter garbage
    let d = t.path().join("bad");
    experiment(
        &d,
        &[
            (
                "acqu.par",
                b"nrPnts = 8\nb1Freq1H = 43\n=\ngarbage\n\xff\xfe".to_vec(),
            ),
            ("data.1d", prospa(504, [u32::MAX; 4], &[])),
            ("short.1d", b"SORPATAD1.1V".to_vec()),
        ],
    );
    let mut ds = SpinsolveReader.open(&d).unwrap();
    let info = ds.info().unwrap();
    assert!(info.traces.is_empty());
    let r = ds.check().unwrap();
    assert!(r.findings.iter().any(|f| f.code == "parameter_syntax"));
    assert!(r.findings.iter().any(|f| f.code == "bad_header"));
    assert!(ds.read_trace(0, 0, 0, 1).is_err());

    // a folder of experiments is a usage error naming them; an unrelated folder is not ours
    let top = t.path();
    match SpinsolveReader.open(top) {
        Err(Error::Usage(m)) => assert!(m.contains("trunc"), "{m}"),
        Err(e) => panic!("{e}"),
        Ok(_) => panic!("opened a folder of experiments"),
    }
    let other = tempfile::tempdir().unwrap();
    std::fs::write(other.path().join("notes.txt"), "x").unwrap();
    assert!(SpinsolveReader.sniff(b"x", other.path()).is_none());
}
