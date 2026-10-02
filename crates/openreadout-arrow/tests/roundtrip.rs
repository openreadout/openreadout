//! Parquet and Arrow IPC exports of a synthetic dataset (a table with integer, float32 and
//! categorical columns, a two-sweep trace, three spectra), read back with `read_file`.
#![allow(clippy::float_cmp)] // the exports are bit-exact: compare exactly

use std::collections::BTreeMap;

use arrow_array::cast::AsArray;
use arrow_array::types::{Float32Type, Float64Type, UInt16Type, UInt32Type, UInt64Type};
use arrow_array::{Array, RecordBatch};
use openreadout_arrow::{
    ColumnarCompression, ColumnarFormat, ColumnarOptions, ColumnarSelection, export_columnar,
    read_columnar, read_file, summary_output,
};
use openreadout_core::model::{
    CheckReport, ColumnInfo, FileInfo, FormatDescriptor, LsEntry, SignalChannelInfo, SpectraInfo,
    Table, TableInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::{Confidence, ProvenanceMap, Source};
use openreadout_core::{Dataset, Error, Plane, PlaneIndex, Result, Spectrum};
use serde_json::json;

const ROWS: u64 = 70_000; // more than one 65,536-row batch
const SAMPLES: u64 = 1000;

#[derive(Debug)]
struct Mock;

fn col(index: u32, name: &str, dtype: &str, unit: Option<&str>) -> ColumnInfo {
    ColumnInfo {
        index,
        name: name.into(),
        label: Some(format!("{name} label")),
        dtype: dtype.into(),
        unit: unit.map(str::to_string),
        range: None,
        extra: BTreeMap::new(),
    }
}

fn value(c: usize, r: u64) -> f64 {
    match c {
        0 => (r % 96) as f64,                    // well (categorical)
        1 => (r % 65_536) as f64,                // uint16
        2 => f64::from((r as f32) * 0.25 - 3.5), // float32
        _ => {
            if r % 1000 == 7 {
                f64::NAN
            } else {
                r as f64 / 3.0
            }
        } // float64 with NaN
    }
}

fn trace_value(s: u32, c: usize, i: u64) -> f64 {
    f64::from(s) * 1000.0 + c as f64 * 0.5 + i as f64 / 7.0
}

impl Dataset for Mock {
    fn info(&self) -> Result<FileInfo> {
        let mut well = col(0, "well", "uint32", None);
        let names: Vec<String> = (0..96)
            .map(|i| format!("{}{}", (b'A' + (i / 12) as u8) as char, i % 12 + 1))
            .collect();
        well.extra.insert("categories".into(), json!(names));
        let mut trace_extra = BTreeMap::new();
        trace_extra.insert("kind".into(), json!("time_domain"));
        Ok(FileInfo {
            path: "/private/mock.bin".into(),
            size_bytes: 1,
            format: FormatDescriptor {
                id: "mock".into(),
                name: "Mock".into(),
                vendor: "test".into(),
                extensions: vec![],
                family: "test".into(),
                can_read: true,
                can_write: false,
                confidence: Confidence::Low,
                known_gaps: vec![],
            },
            format_version: None,
            images: vec![],
            tables: vec![TableInfo {
                index: 0,
                name: Some("events".into()),
                row_count: ROWS,
                columns: vec![
                    well,
                    col(1, "counts", "uint16", None),
                    col(2, "FSC-A", "float32", Some("V")),
                    col(3, "time", "float64", Some("s")),
                ],
                extra: BTreeMap::new(),
            }],
            spectra: vec![SpectraInfo {
                index: 0,
                name: None,
                scan_count: 3,
                ms_levels: vec![1, 2],
                rt_range_s: Some([1.0, 3.0]),
                instrument: None,
                extra: BTreeMap::new(),
            }],
            traces: vec![TraceInfo {
                index: 0,
                name: Some("sweeps".into()),
                sample_rate_hz: 1000.0,
                sample_count: SAMPLES,
                sweep_count: 2,
                channels: ["IN 0", "IN 0"]
                    .iter()
                    .enumerate()
                    .map(|(i, n)| SignalChannelInfo {
                        index: i as u32,
                        name: (*n).into(),
                        unit: Some(if i == 0 { "pA" } else { "mV" }.into()),
                        dtype: "int16".into(),
                        scale: 0.5,
                        offset: 0.0,
                        extra: BTreeMap::new(),
                    })
                    .collect(),
                start_s: None,
                extra: trace_extra,
            }],
            plane_count: 0,
            notes: vec![],
        })
    }
    fn vendor_metadata(&self) -> Result<serde_json::Value> {
        Ok(serde_json::Value::Null)
    }
    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        p.insert("tables[].columns[].unit".into(), Source::Spec);
        p.insert(
            "traces[kind=time_domain].channels[].scale".into(),
            Source::Inferred,
        );
        p
    }
    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(vec![])
    }
    fn read_plane(&mut self, _: u32, _: PlaneIndex) -> Result<Plane> {
        Err(Error::Usage("no images".into()))
    }
    fn check(&mut self) -> Result<CheckReport> {
        Ok(CheckReport::new("mock", "mock"))
    }
    fn read_table(&mut self, _: u32, first: u64, n: u64) -> Result<Table> {
        let n = n.min(ROWS - first);
        Ok(Table {
            table: 0,
            first_row: first,
            columns: (0..4)
                .map(|c| (first..first + n).map(|r| value(c, r)).collect())
                .collect(),
        })
    }
    fn read_trace(&mut self, _: u32, sweep: u32, first: u64, n: u64) -> Result<Trace> {
        let n = n.min(SAMPLES - first);
        Ok(Trace {
            trace: 0,
            sweep,
            first_sample: first,
            channels: (0..2)
                .map(|c| {
                    (first..first + n)
                        .map(|i| trace_value(sweep, c, i))
                        .collect()
                })
                .collect(),
        })
    }
    fn read_spectrum(&mut self, _: u32, i: u64) -> Result<Spectrum> {
        Ok(Spectrum {
            index: i,
            scan_number: i + 1,
            ms_level: if i == 1 { 2 } else { 1 },
            rt_s: Some(1.0 + i as f64),
            polarity: "positive".into(),
            precursor_mz: (i == 1).then_some(445.12),
            mz: (0..=i).map(|k| 100.0 + k as f64).collect(),
            intensity: (0..=i).map(|k| k as f32 * 2.5).collect(),
            ..Spectrum::default()
        })
    }
}

fn opts(format: ColumnarFormat, select: ColumnarSelection) -> ColumnarOptions {
    let mut o = ColumnarOptions::default();
    o.format = format;
    o.select = select;
    o
}

fn concat(batches: &[RecordBatch], c: usize) -> Vec<f64> {
    let mut out = Vec::new();
    for b in batches {
        let a = b.column(c);
        match a.data_type() {
            arrow_schema::DataType::Float64 => {
                out.extend(
                    a.as_primitive::<Float64Type>()
                        .iter()
                        .map(|v| v.unwrap_or(f64::NAN)),
                );
            }
            arrow_schema::DataType::Float32 => out.extend(
                a.as_primitive::<Float32Type>()
                    .iter()
                    .map(|v| v.map_or(f64::NAN, f64::from)),
            ),
            arrow_schema::DataType::UInt16 => out.extend(
                a.as_primitive::<UInt16Type>()
                    .iter()
                    .map(|v| v.map_or(f64::NAN, f64::from)),
            ),
            arrow_schema::DataType::UInt32 => out.extend(
                a.as_primitive::<UInt32Type>()
                    .iter()
                    .map(|v| v.map_or(f64::NAN, f64::from)),
            ),
            arrow_schema::DataType::UInt64 => out.extend(
                a.as_primitive::<UInt64Type>()
                    .iter()
                    .map(|v| v.map_or(f64::NAN, |x| x as f64)),
            ),
            other => panic!("unexpected {other}"),
        }
    }
    out
}

#[test]
fn table_round_trips_in_both_formats() {
    let dir = tempfile::tempdir().unwrap();
    for (format, name) in [
        (ColumnarFormat::Parquet, "t.parquet"),
        (ColumnarFormat::ArrowIpc, "t.arrow"),
    ] {
        let out = dir.path().join(name);
        let r = export_columnar(
            &mut Mock,
            std::path::Path::new("/private/mock.bin"),
            &out,
            &opts(format, ColumnarSelection::Auto),
        )
        .unwrap();
        assert!(r.verified);
        assert_eq!(
            r.kind, "spectra",
            "auto picks spectra for a file without images"
        );
        let r = export_columnar(
            &mut Mock,
            std::path::Path::new("/private/mock.bin"),
            &out,
            &{
                let mut o = opts(format, ColumnarSelection::Table(0));
                o.overwrite = true;
                o
            },
        )
        .unwrap();
        assert_eq!(r.rows_written, ROWS);
        assert_eq!(r.columns, ["well", "counts", "FSC-A", "time"]);
        let (schema, batches) = read_file(&out).unwrap();
        assert_eq!(schema.metadata()["openreadout.kind"], "table");
        assert_eq!(schema.metadata()["openreadout.source_format"], "mock");
        let info: serde_json::Value =
            serde_json::from_str(&schema.metadata()["openreadout.info"]).unwrap();
        assert_eq!(info["path"], "mock.bin", "the local path is not embedded");
        assert_eq!(schema.field(2).metadata()["unit"], "V");
        assert!(schema.field(2).metadata()["openreadout.provenance"].contains("tables[]"));
        // categorical well names
        let wells = batches[0]
            .column(0)
            .as_dictionary::<arrow_array::types::Int32Type>();
        let names = wells.values().as_string::<i32>();
        assert_eq!(names.value(wells.keys().value(13) as usize), "B2");
        for c in 1..4 {
            let got = concat(&batches, c);
            let want: Vec<f64> = (0..ROWS).map(|r| value(c, r)).collect();
            assert_eq!(got.len(), want.len());
            assert!(
                got.iter()
                    .zip(&want)
                    .all(|(a, b)| a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())),
                "column {c}"
            );
        }
    }
}

#[test]
fn trace_has_sweep_and_time_columns() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("tr.parquet");
    let mut o = opts(ColumnarFormat::Parquet, ColumnarSelection::Trace(0));
    o.compression = Some(ColumnarCompression::Lz4);
    o.rows = Some((10, Some(19)));
    let r = export_columnar(&mut Mock, std::path::Path::new("m"), &out, &o).unwrap();
    assert_eq!(r.rows_written, 20);
    assert_eq!(r.sweeps, Some(vec![0, 1]));
    assert_eq!(r.columns, ["sweep", "time_s", "IN 0", "IN 0_1"]);
    let (schema, batches) = read_file(&out).unwrap();
    assert_eq!(schema.field(2).metadata()["unit"], "pA");
    assert_eq!(schema.field(3).metadata()["unit"], "mV");
    assert!(schema.field(2).metadata()["openreadout.provenance"].contains("time_domain"));
    let time = concat(&batches, 1);
    assert_eq!(time[0], 10.0 / 1000.0);
    let sweep = concat(&batches, 0);
    assert_eq!(sweep[..10], [0.0; 10]);
    assert_eq!(sweep[10..], [1.0; 10]);
    let v = concat(&batches, 3);
    assert_eq!(v[10], trace_value(1, 1, 10));
}

#[test]
fn spectra_write_points_and_a_scan_summary() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("sp.arrow");
    let r = export_columnar(
        &mut Mock,
        std::path::Path::new("m"),
        &out,
        &opts(ColumnarFormat::ArrowIpc, ColumnarSelection::Spectra(0)),
    )
    .unwrap();
    assert_eq!(r.rows_written, 6);
    assert_eq!(r.spectra_written, Some(3));
    let summary = summary_output(&out);
    assert_eq!(summary.file_name().unwrap(), "sp.scans.arrow");
    assert_eq!(r.summary_output.as_deref(), Some(summary.to_str().unwrap()));
    let (_, points) = read_file(&out).unwrap();
    assert_eq!(concat(&points, 0), [1.0, 2.0, 2.0, 3.0, 3.0, 3.0]);
    assert_eq!(
        concat(&points, 4),
        [100.0, 100.0, 101.0, 100.0, 101.0, 102.0]
    );
    let (s, scans) = read_file(&summary).unwrap();
    assert_eq!(s.metadata()["openreadout.kind"], "scans");
    assert_eq!(scans[0].num_rows(), 3);
    // in memory, the same columns
    let data = read_columnar(
        &mut Mock,
        std::path::Path::new("m"),
        &opts(ColumnarFormat::Parquet, ColumnarSelection::Spectra(0)),
        None,
    )
    .unwrap();
    assert_eq!(data.summary.unwrap().num_rows(), 3);
    let bytes = openreadout_arrow::ipc_stream_bytes(&data.schema, &data.batches).unwrap();
    assert!(!bytes.is_empty());
}

#[test]
fn refuses_what_it_cannot_write() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("x.parquet");
    std::fs::write(&out, b"keep").unwrap();
    let e = export_columnar(
        &mut Mock,
        std::path::Path::new("m"),
        &out,
        &opts(ColumnarFormat::Parquet, ColumnarSelection::Table(0)),
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 2, "existing output without overwrite");
    assert_eq!(std::fs::read(&out).unwrap(), b"keep");
    for sel in [ColumnarSelection::Table(1), ColumnarSelection::Trace(3)] {
        let e = export_columnar(
            &mut Mock,
            std::path::Path::new("m"),
            &dir.path().join("y.parquet"),
            &opts(ColumnarFormat::Parquet, sel),
        )
        .unwrap_err();
        assert_eq!(e.exit_code(), 2, "{sel:?}");
    }
    let mut o = opts(ColumnarFormat::ArrowIpc, ColumnarSelection::Table(0));
    o.compression = Some(ColumnarCompression::Snappy);
    let e = export_columnar(
        &mut Mock,
        std::path::Path::new("m"),
        &dir.path().join("z.arrow"),
        &o,
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 2);
    assert!(!dir.path().join("z.arrow").exists());
    // read_columnar honours its row cap
    let e = read_columnar(
        &mut Mock,
        std::path::Path::new("m"),
        &opts(ColumnarFormat::Parquet, ColumnarSelection::Table(0)),
        Some(10),
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 2);
}
