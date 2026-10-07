//! Synthetic NSx/NEV files built from the published layout: packets as sweeps, PTP gaps,
//! spec 2.1 scaling from the companion NEV, truncation, NEV packet kinds.

use std::path::{Path, PathBuf};

use openreadout_blackrock::BlackrockReader;
use openreadout_core::reader::FormatReader;
use openreadout_core::{Dataset, Error};

fn dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn open(p: &Path) -> Box<dyn Dataset> {
    BlackrockReader.open(p).unwrap()
}

fn cc(id: u16, label: &str, units: &str) -> Vec<u8> {
    let mut b = vec![0u8; 66];
    b[0..2].copy_from_slice(b"CC");
    b[2..4].copy_from_slice(&id.to_le_bytes());
    b[4..4 + label.len()].copy_from_slice(label.as_bytes());
    b[22..24].copy_from_slice(&(-32764i16).to_le_bytes());
    b[24..26].copy_from_slice(&32764i16.to_le_bytes());
    b[26..28].copy_from_slice(&(-8191i16).to_le_bytes());
    b[28..30].copy_from_slice(&8191i16.to_le_bytes());
    b[30..30 + units.len()].copy_from_slice(units.as_bytes());
    b
}

/// NSx 2.3 (`NEURALCD`) with 2 channels at 1 kS/s and the given packets (timestamp, samples).
fn nsx23(packets: &[(u32, u32)]) -> Vec<u8> {
    let mut b = vec![0u8; 314];
    b[0..8].copy_from_slice(b"NEURALCD");
    b[8] = 2;
    b[9] = 3;
    b[10..14].copy_from_slice(&(314u32 + 2 * 66).to_le_bytes());
    b[14..20].copy_from_slice(b"1 kS/s");
    b[286..290].copy_from_slice(&30u32.to_le_bytes());
    b[290..294].copy_from_slice(&30000u32.to_le_bytes());
    for (i, v) in [2020u16, 5, 2, 12, 13, 14, 15, 16].iter().enumerate() {
        b[294 + 2 * i..296 + 2 * i].copy_from_slice(&v.to_le_bytes());
    }
    b[310..314].copy_from_slice(&2u32.to_le_bytes());
    b.extend(cc(1, "elec1", "uV"));
    b.extend(cc(2, "elec2", "uV"));
    let mut k = 0i16;
    for &(ts, n) in packets {
        b.push(1);
        b.extend_from_slice(&ts.to_le_bytes());
        b.extend_from_slice(&n.to_le_bytes());
        for _ in 0..n {
            b.extend_from_slice(&k.to_le_bytes());
            b.extend_from_slice(&(-k).to_le_bytes());
            k += 1;
        }
    }
    b
}

fn write(d: &tempfile::TempDir, name: &str, b: &[u8]) -> PathBuf {
    let p = d.path().join(name);
    std::fs::write(&p, b).unwrap();
    p
}

#[test]
fn nsx23_packets_are_sweeps() {
    let d = dir();
    let p = write(&d, "a.ns2", &nsx23(&[(0, 5), (3000, 4)]));
    let mut ds = open(&p);
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("2.3"));
    let t = &info.traces[0];
    assert_eq!(t.sweep_count, 2);
    assert!((t.sample_rate_hz - 1000.0).abs() < 1e-12);
    assert_eq!(t.channels[1].name, "elec2");
    assert_eq!(t.extra["recorded_at"], "2020-05-12T13:14:15.016Z");
    assert_eq!(t.extra["sweep_sample_counts"], serde_json::json!([5, 4]));
    let scale = 16382.0 / 65528.0;
    let s1 = ds.read_trace(0, 1, 1, 2).unwrap();
    assert!((s1.channels[0][0] - 6.0 * scale).abs() < 1e-12);
    assert!((s1.channels[1][1] + 7.0 * scale).abs() < 1e-12);
    let r = ds.check().unwrap();
    assert!(r.ok, "{:?}", r.findings);
    assert!(r.findings.iter().any(|f| f.code == "segments"));
}

#[test]
fn nsx_truncated_packet() {
    let d = dir();
    let mut b = nsx23(&[(0, 10)]);
    b.truncate(b.len() - 6);
    let p = write(&d, "b.ns5", &b);
    let mut ds = open(&p);
    let r = ds.check().unwrap();
    assert!(!r.ok);
    assert!(r.findings.iter().any(|f| f.code == "truncated"));
    // the whole samples that remain are readable
    assert_eq!(ds.read_trace(0, 0, 0, 100).unwrap().channels[0].len(), 8);
}

#[test]
fn nsx_clock_reset_is_reported() {
    let d = dir();
    let p = write(&d, "c.ns2", &nsx23(&[(0, 5), (60, 4)]));
    let r = open(&p).check().unwrap();
    assert!(r.findings.iter().any(|f| f.code == "clock_reset"));
}

#[test]
fn ptp_one_sample_packets_split_at_gaps() {
    let d = dir();
    let mut b = nsx23(&[]);
    b[0..8].copy_from_slice(b"BRSMPGRP");
    b[8] = 3;
    b[9] = 0;
    b[290..294].copy_from_slice(&1_000_000_000u32.to_le_bytes());
    let mut ts = 1_000_000_000u64;
    for i in 0..6i16 {
        if i == 4 {
            ts += 5_000_000; // 5 ms gap at 1 kHz
        }
        b.push(1);
        b.extend_from_slice(&ts.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&i.to_le_bytes());
        b.extend_from_slice(&(10 * i).to_le_bytes());
        ts += 1_000_000 + if i % 2 == 0 { 200 } else { 0 };
    }
    let p = write(&d, "d.ns2", &b);
    let mut ds = open(&p);
    let t = ds.info().unwrap().traces[0].clone();
    assert_eq!(t.extra["ptp"], true);
    assert_eq!(t.sweep_count, 2);
    assert_eq!(t.extra["sweep_sample_counts"], serde_json::json!([4, 2]));
    let s1 = ds.read_trace(0, 1, 0, 10).unwrap();
    let scale = 16382.0 / 65528.0;
    assert!((s1.channels[1][1] - 50.0 * scale).abs() < 1e-12);
}

#[test]
fn ptp_gaps_that_cancel_out_still_split_sweeps() {
    // a jump forward and a clock reset of the same size: first and last timestamps agree
    let d = dir();
    let mut b = nsx23(&[]);
    b[0..8].copy_from_slice(b"BRSMPGRP");
    b[8] = 3;
    b[9] = 0;
    b[290..294].copy_from_slice(&1_000_000_000u32.to_le_bytes());
    let mut ts = 1_000_000_000u64;
    for i in 0..8i16 {
        if i == 3 {
            ts += 5_000_000;
        }
        if i == 6 {
            ts -= 5_000_000;
        }
        b.push(1);
        b.extend_from_slice(&ts.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&i.to_le_bytes());
        b.extend_from_slice(&(10 * i).to_le_bytes());
        ts += 1_000_000;
    }
    let p = write(&d, "e.ns2", &b);
    let mut ds = open(&p);
    let trace = ds.info().unwrap().traces[0].clone();
    assert_eq!(trace.extra["ptp_timestamps_read"], "all");
    assert_eq!(trace.sweep_count, 3);
    assert_eq!(
        trace.extra["sweep_sample_counts"],
        serde_json::json!([3, 3, 2])
    );
    let report = ds.check().unwrap();
    assert!(report.findings.iter().any(|f| f.code == "segments"));
}

fn nev(version: (u8, u8), waveform_nv: u16) -> Vec<u8> {
    let plen = 8 + 8u32; // 4-byte ts, id, unit, reserved, 8 one-byte samples
    let mut b = vec![0u8; 336];
    b[0..8].copy_from_slice(b"NEURALEV");
    b[8] = version.0;
    b[9] = version.1;
    b[12..16].copy_from_slice(&(336u32 + 32).to_le_bytes());
    b[16..20].copy_from_slice(&plen.to_le_bytes());
    b[20..24].copy_from_slice(&30000u32.to_le_bytes());
    b[24..28].copy_from_slice(&30000u32.to_le_bytes());
    b[332..336].copy_from_slice(&1u32.to_le_bytes());
    let mut ext = vec![0u8; 32];
    ext[0..8].copy_from_slice(b"NEUEVWAV");
    ext[8..10].copy_from_slice(&3u16.to_le_bytes());
    ext[12..14].copy_from_slice(&waveform_nv.to_le_bytes());
    ext[21] = 1;
    b.extend(ext);
    // spike on electrode 3, unit 2
    let mut spike = vec![0u8; plen as usize];
    spike[0..4].copy_from_slice(&300u32.to_le_bytes());
    spike[4..6].copy_from_slice(&3u16.to_le_bytes());
    spike[6] = 2;
    for i in 0..8 {
        spike[8 + i] = (i as i8 - 4) as u8;
    }
    b.extend(&spike);
    // digital
    let mut digital = vec![0u8; plen as usize];
    digital[0..4].copy_from_slice(&600u32.to_le_bytes());
    digital[6] = 1;
    digital[8..10].copy_from_slice(&0xABCDu16.to_le_bytes());
    b.extend(&digital);
    // comment
    let mut comment = vec![0u8; plen as usize];
    comment[0..4].copy_from_slice(&900u32.to_le_bytes());
    comment[4..6].copy_from_slice(&0xFFFFu16.to_le_bytes());
    comment[12..16].copy_from_slice(b"hi!\0");
    b.extend(&comment);
    b
}

#[test]
#[allow(clippy::float_cmp)]
fn nev_packets_table() {
    let d = dir();
    let p = write(&d, "e.nev", &nev((2, 3), 250));
    let mut ds = open(&p);
    let info = ds.info().unwrap();
    let t = &info.tables[0];
    assert_eq!(t.row_count, 3);
    let tab = ds.read_table(0, 0, 10).unwrap();
    let col = |n: &str| t.columns.iter().position(|c| c.name == n).unwrap();
    assert_eq!(tab.columns[col("kind")], vec![1.0, 0.0, 2.0]);
    assert!((tab.columns[col("time_s")][0] - 0.01).abs() < 1e-12);
    assert_eq!(tab.columns[col("code")][0], 2.0);
    assert_eq!(tab.columns[col("digital")][1], f64::from(0xABCDu16));
    assert!((tab.columns[col("w0")][0] + 4.0 * 0.25).abs() < 1e-12);
    assert!(tab.columns[col("w0")][1].is_nan());
    let v = ds.vendor_metadata().unwrap();
    assert_eq!(v["comments"][0]["text"], "hi!");
    assert!(ds.check().unwrap().ok);
}

#[test]
fn spec21_scaling_from_companion_nev() {
    let d = dir();
    let mut b = Vec::new();
    b.extend_from_slice(b"NEURALSG");
    b.extend_from_slice(&[0u8; 16]);
    b.extend_from_slice(&30u32.to_le_bytes());
    b.extend_from_slice(&1u32.to_le_bytes());
    b.extend_from_slice(&3u32.to_le_bytes());
    for v in [10i16, 20, 30] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    let p = write(&d, "f.ns2", &b);
    // without the NEV: raw counts
    let t = open(&p).info().unwrap().traces[0].clone();
    assert_eq!(t.channels[0].unit, None);
    assert_eq!(t.channels[0].name, "chan3");
    write(&d, "f.nev", &nev((2, 1), 21516));
    let mut ds = open(&p);
    let t = ds.info().unwrap().traces[0].clone();
    assert_eq!(t.channels[0].unit.as_deref(), Some("uV"));
    assert_eq!(t.sample_count, 3);
    let tr = ds.read_trace(0, 0, 0, 10).unwrap();
    assert!((tr.channels[0][2] - 30.0 * 152.592_547).abs() < 1e-9);
}

#[test]
fn garbage_is_rejected() {
    let d = dir();
    let p = write(&d, "g.ns5", b"NEURALCD\x02\x03");
    match BlackrockReader.open(&p) {
        Err(e @ Error::Corrupt { .. }) => assert_eq!(e.exit_code(), 4),
        Err(e) => panic!("unexpected {e}"),
        Ok(_) => panic!("accepted a 10-byte file"),
    }
}

#[test]
fn memory_session_includes_nsx_and_nev() {
    use openreadout_core::source::{Fs, Input, MemFs, MemSource};
    use std::sync::Arc;
    let fs = MemFs::new()
        .with(
            "memory/rec.ns5",
            Arc::new(MemSource::new("rec.ns5", nsx23(&[(0, 3)]))),
        )
        .with(
            "memory/rec.nev",
            Arc::new(MemSource::new("rec.nev", nev((2, 3), 250))),
        );
    let input = Input::new("memory", Fs::new(Arc::new(fs)));
    assert!(BlackrockReader.sniff_input(&[], &input).is_some());
    let mut ds = BlackrockReader.open_input(&input).unwrap();
    let info = ds.info().unwrap();
    assert!(!info.traces.is_empty());
    assert!(!info.tables.is_empty());
    assert!(!ds.read_trace(0, 0, 0, 2).unwrap().channels.is_empty());
    assert!(ds.check().unwrap().ok);
}
