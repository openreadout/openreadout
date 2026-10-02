//! JCAMP-DX export of synthetic spectra (a ppm spectrum, a complex two-sweep FID, a peak table and
//! arbitrary floats), re-read with the JCAMP-DX reader.
#![allow(clippy::float_cmp, clippy::needless_range_loop)] // bit-exact exports: compare exactly

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::model::{
    CheckReport, FileInfo, FormatDescriptor, LsEntry, SignalChannelInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::{Confidence, ProvenanceMap};
use openreadout_core::{Dataset, Error, FormatReader, Plane, PlaneIndex, Result};
use openreadout_nmr::{
    JcampEncoding, JcampExportOptions, JcampReader, default_jcamp_output, export_jcamp,
};
use serde_json::json;

#[derive(Debug)]
struct Spectra;

fn chan(index: u32, name: &str, scale: f64) -> SignalChannelInfo {
    SignalChannelInfo {
        index,
        name: name.into(),
        unit: None,
        dtype: "int32".into(),
        scale,
        offset: 0.0,
        extra: BTreeMap::new(),
    }
}

/// Trace 0: ppm spectrum (integers × 2^-3); 1: complex FID, 2 sweeps; 2: peak table;
/// 3: arbitrary floats with a wide range.
fn value(t: u32, s: u32, c: usize, i: u64) -> f64 {
    let i = i as f64;
    match t {
        0 => ((i * 37.0) % 2001.0 - 1000.0) * 0.125,
        1 => (f64::from(s) * 100.0 + i * if c == 0 { 3.0 } else { -2.0 }).round() * 4.0,
        2 => {
            if c == 0 {
                50.0 + i * 1.5
            } else {
                (i * 7.0) % 100.0
            }
        }
        _ => (i * 0.37).sin() * 1e6 + 1e-9,
    }
}

const N: [u64; 4] = [1000, 300, 12, 500];

impl Dataset for Spectra {
    fn info(&self) -> Result<FileInfo> {
        let mut spec = BTreeMap::new();
        spec.insert(
            "axis".into(),
            json!({"quantity": "chemical_shift", "unit": "ppm", "first": 12.5, "step": -0.0125}),
        );
        spec.insert("kind".into(), json!("processed_spectrum"));
        spec.insert("nucleus".into(), json!("1H"));
        spec.insert("spectrometer_frequency_mhz".into(), json!(400.13));
        let mut fid = BTreeMap::new();
        fid.insert("kind".into(), json!("time_domain"));
        fid.insert(
            "axis".into(),
            json!({"quantity": "time", "unit": "s", "first": 0.0, "step": 1e-4}),
        );
        let mut peaks = BTreeMap::new();
        peaks.insert("kind".into(), json!("peak_table"));
        peaks.insert("axis".into(), json!({"irregular": true, "channel": 0}));
        let t = |index: u32, name: &str, sweeps: u32, channels, extra| TraceInfo {
            index,
            name: Some(name.into()),
            sample_rate_hz: 0.0,
            sample_count: N[index as usize],
            sweep_count: sweeps,
            channels,
            start_s: None,
            extra,
        };
        Ok(FileInfo {
            path: "exp/1".into(),
            size_bytes: 1,
            format: FormatDescriptor {
                id: "bruker-nmr".into(),
                name: "Mock NMR".into(),
                vendor: "test".into(),
                extensions: vec![],
                family: "nmr".into(),
                can_read: true,
                can_write: false,
                confidence: Confidence::Low,
                known_gaps: vec![],
            },
            format_version: None,
            images: vec![],
            tables: vec![],
            spectra: vec![],
            traces: vec![
                t(0, "1r", 1, vec![chan(0, "real", 1.0)], spec),
                t(
                    1,
                    "ser",
                    2,
                    vec![chan(0, "real", 4.0), chan(1, "imag", 4.0)],
                    fid,
                ),
                t(
                    2,
                    "peaks",
                    1,
                    vec![chan(0, "m/z", 1.0), chan(1, "y", 1.0)],
                    peaks,
                ),
                t(3, "floats", 1, vec![chan(0, "y", 1.0)], BTreeMap::new()),
            ],
            plane_count: 0,
            notes: vec![],
        })
    }
    fn vendor_metadata(&self) -> Result<serde_json::Value> {
        Ok(serde_json::Value::Null)
    }
    fn provenance(&self) -> ProvenanceMap {
        ProvenanceMap::new()
    }
    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(vec![])
    }
    fn read_plane(&mut self, _: u32, _: PlaneIndex) -> Result<Plane> {
        Err(Error::Usage("no images".into()))
    }
    fn check(&mut self) -> Result<CheckReport> {
        Ok(CheckReport::new("m", "m"))
    }
    fn read_trace(&mut self, t: u32, s: u32, first: u64, n: u64) -> Result<Trace> {
        let n = n.min(N[t as usize] - first);
        let chans = if t == 0 || t == 3 { 1 } else { 2 };
        Ok(Trace {
            trace: t,
            sweep: s,
            first_sample: first,
            channels: (0..chans)
                .map(|c| (first..first + n).map(|i| value(t, s, c, i)).collect())
                .collect(),
        })
    }
}

fn opts(trace: u32) -> JcampExportOptions {
    let mut o = JcampExportOptions::default();
    o.trace = Some(trace);
    o
}

fn reread(path: &Path, sweep: u32) -> (TraceInfo, Vec<Vec<f64>>) {
    let mut ds = JcampReader.open(path).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces.len(), 1);
    let t = info.traces[0].clone();
    let tr = ds.read_trace(0, sweep, 0, t.sample_count).unwrap();
    let check = ds.check().unwrap();
    assert!(check.ok, "{:?}", check.findings);
    (t, tr.channels)
}

#[test]
fn ppm_spectrum_is_exact_in_difdup_and_affn() {
    let dir = tempfile::tempdir().unwrap();
    for enc in [JcampEncoding::Difdup, JcampEncoding::Affn] {
        let out = dir.path().join(format!("{}.jdx", enc.id()));
        let mut o = opts(0);
        o.encoding = enc;
        let r = export_jcamp(&mut Spectra, Path::new("exp/1"), &out, &o).unwrap();
        assert!(r.exact && r.verified);
        assert_eq!(r.data_type, "NMR SPECTRUM");
        assert_eq!(r.data_class, "XYDATA");
        assert_eq!(
            r.factors,
            [0.125],
            "integers x 2^-3: a power of two holds the values"
        );
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.contains("##.OBSERVE NUCLEUS= ^1H"));
        assert!(text.contains("##XUNITS= PPM"));
        assert!(text.lines().all(|l| l.len() <= 80));
        let (t, back) = reread(&out, 0);
        let axis = &t.extra["axis"];
        assert_eq!(axis["first"], 12.5);
        assert!((axis["step"].as_f64().unwrap() + 0.0125).abs() < 1e-15);
        for (i, v) in back[0].iter().enumerate() {
            assert_eq!(*v, value(0, 0, 0, i as u64));
        }
    }
}

#[test]
fn complex_fid_sweeps_are_ntuples_pages() {
    let dir = tempfile::tempdir().unwrap();
    let out = default_jcamp_output(&dir.path().join("fid"), &opts(1));
    assert_eq!(out.file_name().unwrap(), "fid.trace1.jdx");
    let r = export_jcamp(&mut Spectra, Path::new("exp/1"), &out, &opts(1)).unwrap();
    assert_eq!(r.data_class, "NTUPLES");
    assert_eq!(r.data_type, "NMR FID");
    assert_eq!(r.pages, 4);
    assert_eq!(r.factors, [4.0, 4.0]);
    for s in 0..2u32 {
        let (t, back) = reread(&out, s);
        assert_eq!(t.sweep_count, 2);
        assert_eq!(t.channels[0].name, "real");
        assert_eq!(t.channels[1].name, "imag");
        for c in 0..2 {
            for (i, v) in back[c].iter().enumerate() {
                assert_eq!(*v, value(1, s, c, i as u64));
            }
        }
    }
    // one sweep, a row range
    let mut o = opts(1);
    o.sweep = Some(1);
    o.rows = Some((10, Some(59)));
    let out = dir.path().join("one.jdx");
    let r = export_jcamp(&mut Spectra, Path::new("exp/1"), &out, &o).unwrap();
    assert_eq!((r.pages, r.samples_written), (2, 50));
    let (t, back) = reread(&out, 0);
    assert!((t.extra["axis"]["first"].as_f64().unwrap() - 1e-3).abs() < 1e-15);
    assert_eq!(back[1][0], value(1, 1, 1, 10));
}

#[test]
fn peak_tables_and_arbitrary_floats() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("peaks.jdx");
    let r = export_jcamp(&mut Spectra, Path::new("exp/1"), &out, &opts(2)).unwrap();
    assert_eq!(r.data_class, "PEAK TABLE");
    let (_, back) = reread(&out, 0);
    assert_eq!(back[0][3], 54.5);
    assert_eq!(back[1][3], 21.0);
    // wide-range floats: exact with a power-of-two factor when 53 bits hold them
    let out = dir.path().join("floats.jdx");
    let r = export_jcamp(&mut Spectra, Path::new("exp/1"), &out, &opts(3)).unwrap();
    let (_, back) = reread(&out, 0);
    for (i, v) in back[0].iter().enumerate() {
        let want = value(3, 0, 0, i as u64);
        if r.exact {
            assert_eq!(*v, want);
        } else {
            assert!((v - want).abs() <= r.factors[0] / 2.0);
        }
    }
    assert!(r.max_abs_error <= r.factors[0] / 2.0);
}
