//! NWB export of a synthetic two-trace recording, re-read with the NWB reader.
#![allow(clippy::float_cmp)] // the exports are bit-exact: compare exactly

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::model::{
    CheckReport, FileInfo, FormatDescriptor, LsEntry, SignalChannelInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::{Confidence, ProvenanceMap};
use openreadout_core::{Dataset, Error, FormatReader, Plane, PlaneIndex, Result};
use openreadout_hdf5::{NwbExportOptions, NwbReader, default_nwb_output, export_nwb};
use serde_json::json;

#[derive(Debug)]
struct Ephys;

fn chan(index: u32, name: &str, unit: &str) -> SignalChannelInfo {
    SignalChannelInfo {
        index,
        name: name.into(),
        unit: Some(unit.into()),
        dtype: "int16".into(),
        scale: 0.1,
        offset: 0.0,
        extra: BTreeMap::new(),
    }
}

fn sample(t: u32, s: u32, c: usize, i: u64) -> f64 {
    f64::from(t) * 10.0 + f64::from(s) + c as f64 * 0.25 + (i as f64 * 0.1).sin()
}

impl Dataset for Ephys {
    fn info(&self) -> Result<FileInfo> {
        let mut extra = BTreeMap::new();
        extra.insert("acquired_at".into(), json!("2021-05-06T07:08:09"));
        extra.insert("operator".into(), json!("Doe, Jane"));
        Ok(FileInfo {
            path: "/data/cell.abf".into(),
            size_bytes: 1,
            format: FormatDescriptor {
                id: "abf".into(),
                name: "Mock ephys".into(),
                vendor: "test".into(),
                extensions: vec![],
                family: "electrophysiology".into(),
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
                TraceInfo {
                    index: 0,
                    name: Some("IV steps".into()),
                    sample_rate_hz: 20_000.0,
                    sample_count: 5000,
                    sweep_count: 2,
                    channels: vec![
                        chan(0, "IN 0", "pA"),
                        chan(1, "IN 1", "mV"),
                        chan(2, "IN 2", "pA"),
                    ],
                    start_s: None,
                    extra: extra.clone(),
                },
                TraceInfo {
                    index: 1,
                    name: None,
                    sample_rate_hz: 30_000.123,
                    sample_count: 70_000,
                    sweep_count: 1,
                    channels: vec![chan(0, "LFP", "uV")],
                    start_s: Some(1.5),
                    extra,
                },
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
        let (count, chans) = if t == 0 { (5000, 3) } else { (70_000, 1) };
        let n = n.min(count - first);
        Ok(Trace {
            trace: t,
            sweep: s,
            first_sample: first,
            channels: (0..chans)
                .map(|c| (first..first + n).map(|i| sample(t, s, c, i)).collect())
                .collect(),
        })
    }
}

#[test]
fn nwb_round_trip_every_trace_sweep_and_unit() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("cell.nwb");
    let r = export_nwb(
        &mut Ephys,
        Path::new("/data/cell.abf"),
        &out,
        &NwbExportOptions::default(),
    )
    .unwrap();
    assert!(r.verified);
    let names: Vec<&str> = r.series.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "trace0_sweep0_pA",
            "trace0_sweep0_mV",
            "trace0_sweep1_pA",
            "trace0_sweep1_mV",
            "trace1"
        ]
    );
    assert_eq!(r.series[0].channel_indices, [0, 2]);
    assert_eq!(r.session_start_time, "2021-05-06T07:08:09+00:00");
    assert!(r.notes.iter().any(|n| n.contains("time zone")));
    assert_eq!(r.samples_written, 2 * 5000 * 3 + 70_000);
    // re-read with the NWB reader
    let mut ds = NwbReader.open(&out).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces.len(), 5);
    let lfp = info
        .traces
        .iter()
        .find(|t| t.name.as_deref() == Some("trace1"))
        .unwrap()
        .clone();
    assert_eq!(lfp.sample_rate_hz, 30_000.123);
    assert_eq!(lfp.start_s, Some(1.5));
    let back = ds.read_trace(lfp.index, 0, 65_530, 20).unwrap();
    assert_eq!(back.channels[0].len(), 20); // across the chunk boundary at 65,535
    assert_eq!(back.channels[0][3], sample(1, 0, 0, 65_533));
    let pa = info
        .traces
        .iter()
        .find(|t| t.name.as_deref() == Some("trace0_sweep1_pA"))
        .unwrap()
        .clone();
    assert_eq!(pa.channels.len(), 2);
    assert_eq!(pa.channels[0].unit.as_deref(), Some("pA"));
    let back = ds.read_trace(pa.index, 0, 0, 5000).unwrap();
    assert_eq!(back.channels[1][17], sample(0, 1, 2, 17));
    assert!(info.notes.iter().any(|n| n.contains("2021-05-06T07:08:09")));
}

#[test]
fn nwb_selection_and_refusals() {
    let dir = tempfile::tempdir().unwrap();
    let mut o = NwbExportOptions::default();
    o.trace = Some(0);
    o.sweep = Some(1);
    o.rows = Some((100, Some(199)));
    let out = default_nwb_output(&dir.path().join("cell.abf"), &o);
    assert_eq!(out.file_name().unwrap(), "cell.trace0.sweep1.nwb");
    let r = export_nwb(&mut Ephys, Path::new("cell.abf"), &out, &o).unwrap();
    assert_eq!(r.series.len(), 2);
    assert_eq!(r.series[0].samples, 100);
    assert!((r.series[0].starting_time_s - 100.0 / 20_000.0).abs() < 1e-15);
    // exists without overwrite
    assert_eq!(
        export_nwb(&mut Ephys, Path::new("cell.abf"), &out, &o)
            .unwrap_err()
            .exit_code(),
        2
    );
    o.sweep = Some(5);
    o.overwrite = true;
    assert_eq!(
        export_nwb(&mut Ephys, Path::new("cell.abf"), &out, &o)
            .unwrap_err()
            .exit_code(),
        2
    );
}
