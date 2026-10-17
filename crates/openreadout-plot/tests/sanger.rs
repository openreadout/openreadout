//! A Sanger read drawn from a synthetic data set in the shape the ABIF reader gives: a trace
//! with A, C, G and T channels and a base-call table whose `scan` column indexes it.

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::model::{
    CheckReport, ColumnInfo, DetectConfidence, FileInfo, FormatDescriptor, LsEntry,
    SignalChannelInfo, Table, TableInfo, Trace, TraceInfo,
};
use openreadout_core::pixel::Plane;
use openreadout_core::provenance::{Confidence, ProvenanceMap};
use openreadout_core::reader::{Dataset, Detection, FormatReader, PlaneIndex};
use openreadout_core::{Error, Registry, Result};
use openreadout_plot::{parse_spec, plot};
use serde_json::json;

const BASES: usize = 40;
const SPACING: usize = 12;
const ORDER: [&str; 4] = ["A", "C", "G", "T"];

fn scan_of(i: usize) -> usize {
    10 + SPACING * i
}

fn samples() -> usize {
    scan_of(BASES) + 10
}

fn quality(i: usize) -> f64 {
    if i < 10 || i == 24 { 10.0 } else { 40.0 }
}

fn descriptor() -> FormatDescriptor {
    FormatDescriptor {
        id: "synth-sanger".into(),
        name: "Synthetic Sanger".into(),
        vendor: "test".into(),
        extensions: vec!["ab1".into()],
        family: "electrophoresis".into(),
        can_read: true,
        can_write: false,
        confidence: Confidence::High,
        known_gaps: vec![],
    }
}

struct Read;

impl Dataset for Read {
    fn info(&self) -> Result<FileInfo> {
        let channels = ORDER
            .iter()
            .enumerate()
            .map(|(i, n)| SignalChannelInfo {
                index: i as u32,
                name: (*n).into(),
                unit: Some("RFU".into()),
                dtype: "int16".into(),
                scale: 1.0,
                offset: 0.0,
                extra: BTreeMap::new(),
            })
            .collect();
        let mut extra = BTreeMap::new();
        extra.insert("kind".into(), json!("sanger_trace"));
        extra.insert("base_calls_table".into(), json!(0));
        let col = |i: u32, name: &str, extra: BTreeMap<String, serde_json::Value>| ColumnInfo {
            index: i,
            name: name.into(),
            dtype: "float64".into(),
            extra,
            ..ColumnInfo::default()
        };
        let mut cats = BTreeMap::new();
        cats.insert("categories".into(), json!(ORDER));
        let mut table_extra = BTreeMap::new();
        table_extra.insert("kind".into(), json!("base_calls"));
        Ok(FileInfo {
            path: "read.ab1".into(),
            size_bytes: 1,
            format: descriptor(),
            format_version: None,
            images: vec![],
            tables: vec![TableInfo {
                index: 0,
                name: Some("base calls".into()),
                row_count: BASES as u64,
                columns: vec![
                    col(0, "position", BTreeMap::new()),
                    col(1, "base", cats),
                    col(2, "quality", BTreeMap::new()),
                    col(3, "scan", BTreeMap::new()),
                ],
                extra: table_extra,
            }],
            spectra: vec![],
            traces: vec![TraceInfo {
                index: 0,
                name: Some("analyzed".into()),
                sample_rate_hz: 0.0,
                sample_count: samples() as u64,
                sweep_count: 1,
                channels,
                start_s: None,
                extra,
            }],
            plane_count: 0,
            notes: vec![],
        })
    }
    fn vendor_metadata(&self) -> Result<serde_json::Value> {
        Ok(serde_json::Value::Null)
    }
    fn provenance(&self) -> ProvenanceMap {
        ProvenanceMap::default()
    }
    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(vec![])
    }
    fn check(&mut self) -> Result<CheckReport> {
        Ok(CheckReport::new("synth-sanger", "read.ab1"))
    }
    fn read_plane(&mut self, _: u32, _: PlaneIndex) -> Result<Plane> {
        Err(Error::Usage("no images".into()))
    }
    fn read_table(&mut self, _: u32, first: u64, max: u64) -> Result<Table> {
        let rows = (first as usize..BASES).take(max as usize);
        Ok(Table {
            table: 0,
            first_row: first,
            columns: vec![
                rows.clone().map(|i| (i + 1) as f64).collect(),
                rows.clone().map(|i| (i % 4) as f64).collect(),
                rows.clone().map(quality).collect(),
                rows.map(|i| scan_of(i) as f64).collect(),
            ],
        })
    }
    fn read_trace(&mut self, _: u32, _: u32, first: u64, max: u64) -> Result<Trace> {
        let end = (first + max).min(samples() as u64);
        let channels = (0..4)
            .map(|c| {
                (first..end)
                    .map(|s| {
                        (0..BASES)
                            .filter(|i| i % 4 == c)
                            .map(|i| {
                                let d = s as f64 - scan_of(i) as f64;
                                1000.0 * (-d * d / 8.0).exp()
                            })
                            .sum()
                    })
                    .collect()
            })
            .collect();
        Ok(Trace {
            trace: 0,
            sweep: 0,
            first_sample: first,
            channels,
        })
    }
}

struct Reader;

impl FormatReader for Reader {
    fn descriptor(&self) -> FormatDescriptor {
        descriptor()
    }
    fn sniff(&self, head: &[u8], _: &Path) -> Option<Detection> {
        head.starts_with(b"SYNTH-SANGER").then_some(Detection {
            format_id: "synth-sanger",
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }
    fn open(&self, _: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(Read))
    }
}

fn setup() -> (tempfile::TempDir, String, Registry) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("read.ab1");
    std::fs::write(&p, b"SYNTH-SANGER").unwrap();
    let mut reg = Registry::new();
    reg.register(Box::new(Reader));
    let f = p.display().to_string();
    (dir, f, reg)
}

fn names(p: &openreadout_plot::Plotted) -> Vec<String> {
    p.output.panels[0]
        .series
        .iter()
        .map(|s| s.name.clone())
        .collect()
}

#[test]
fn a_sanger_read_draws_its_traces_calls_and_quality() {
    let (_dir, f, reg) = setup();
    // the default for a Sanger read, and what show=sanger asks for
    for show in [None, Some("sanger"), Some("chromatogram")] {
        let mut data = json!({"file": f});
        if let Some(s) = show {
            data["show"] = json!(s);
        }
        let spec = parse_spec(json!({"data": data, "x": {"range": [12, 30]}})).unwrap();
        let p = plot(&reg, &spec).unwrap();
        assert_eq!(names(&p), ["A", "C", "G", "T"], "show {show:?}");
        let x = &p.output.panels[0].x;
        assert_eq!(x.label, "base position");
        assert_eq!(x.range, Some([12.0, 30.0]));
        let svg = p.figure.to_svg();
        for g in ["A", "C", "G", "T", "base calls", "quality"] {
            assert!(
                svg.contains(&format!("data-series=\"{g}\"")),
                "no {g} group"
            );
        }
        assert!(
            p.output
                .notes
                .iter()
                .any(|n| n.contains("bases 12–30 of 40")),
            "{:?}",
            p.output.notes
        );
    }
}

#[test]
fn a_short_read_is_drawn_whole_and_a_channel_draws_the_trace() {
    let (_dir, f, reg) = setup();
    let spec = parse_spec(json!({"data": {"file": f, "show": "sanger"}})).unwrap();
    let p = plot(&reg, &spec).unwrap();
    // the default window is 80 bases; this read has 40
    assert!(
        p.output
            .notes
            .iter()
            .any(|n| n.contains("bases 1–40 of 40")),
        "{:?}",
        p.output.notes
    );
    // a channel asked for draws the trace itself
    let spec = parse_spec(json!({"data": {"file": f, "channel": 2}})).unwrap();
    let p = plot(&reg, &spec).unwrap();
    assert_eq!(names(&p).len(), 1);
    assert!(!p.figure.to_svg().contains("data-series=\"base calls\""));
}
