//! Synthetic Neuralynx files built from the published record layouts: segments, partial
//! records, truncation, events and tetrode spikes.

use std::path::PathBuf;

use openreadout_core::reader::FormatReader;
use openreadout_core::{Dataset, Error};
use openreadout_neuralynx::{HEADER_LEN, NeuralynxReader};

struct Built {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

fn write(name: &str, bytes: &[u8]) -> Built {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    Built { _dir: dir, path }
}

fn header(lines: &str) -> Vec<u8> {
    let mut h = format!("######## Neuralynx Data File Header\r\n{lines}").into_bytes();
    h.resize(HEADER_LEN as usize, 0);
    h
}

/// NCS at 32 kHz (16000 µs per 512 samples); records given as (timestamp, valid).
fn ncs(records: &[(u64, u32)]) -> Vec<u8> {
    let mut b = header(
        "-FileType CSC\r\n-RecordSize 1044\r\n-SamplingFrequency 32000\r\n-ADBitVolts 0.000000030517578125\r\n-AcqEntName CSC7\r\n-ADChannel 6\r\n-InputInverted True\r\n-TimeCreated 2020/01/02 03:04:05\r\n",
    );
    for (r, &(ts, valid)) in records.iter().enumerate() {
        b.extend_from_slice(&ts.to_le_bytes());
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&32000u32.to_le_bytes());
        b.extend_from_slice(&valid.to_le_bytes());
        for i in 0..512i32 {
            let v = if (i as u32) < valid {
                r as i32 * 1000 + i
            } else {
                -1
            };
            b.extend_from_slice(&(v as i16).to_le_bytes());
        }
    }
    b
}

fn open(name: &str, bytes: &[u8]) -> (Built, Box<dyn Dataset>) {
    let f = write(name, bytes);
    let ds = NeuralynxReader.open(&f.path).unwrap();
    (f, ds)
}

#[test]
fn ncs_segments_skip_invalid_samples() {
    // records 0,1 contiguous; record 1 has 100 valid samples; record 2 after a gap
    let recs = [(1_000_000, 512), (1_016_000, 100), (9_000_000, 512)];
    let (_f, mut ds) = open("a.ncs", &ncs(&recs));
    let info = ds.info().unwrap();
    let t = &info.traces[0];
    assert_eq!(t.sweep_count, 2);
    assert_eq!(t.channels[0].name, "CSC7");
    assert_eq!(t.channels[0].unit.as_deref(), Some("µV"));
    assert_eq!(
        t.extra["sweep_sample_counts"],
        serde_json::json!([612, 512])
    );
    let s0 = ds.read_trace(0, 0, 0, u64::MAX).unwrap();
    assert_eq!(s0.channels[0].len(), 612);
    let uv = 0.000_000_030_517_578_125 * 1e6;
    assert!((s0.channels[0][511] - 511.0 * uv).abs() < 1e-12);
    assert!((s0.channels[0][512] - 1000.0 * uv).abs() < 1e-12);
    assert!((s0.channels[0][611] - 1099.0 * uv).abs() < 1e-12); // never the stale -1
    let w = ds.read_trace(0, 0, 510, 4).unwrap();
    assert_eq!(w.channels[0].len(), 4);
    assert!((w.channels[0][2] - 1000.0 * uv).abs() < 1e-12);
    let s1 = ds.read_trace(0, 1, 0, 3).unwrap();
    assert!((s1.channels[0][0] - 2000.0 * uv).abs() < 1e-12);
    assert!(ds.read_trace(0, 2, 0, 1).is_err());
    let r = ds.check().unwrap();
    assert!(r.ok, "{:?}", r.findings);
    assert!(r.findings.iter().any(|f| f.code == "segments"));
    assert_eq!(t.extra["opened_at"], "2020-01-02T03:04:05");
}

#[test]
fn ncs_fast_path_single_segment() {
    let recs: Vec<(u64, u32)> = (0..10).map(|i| (5_000 + i * 16_000, 512)).collect();
    let (_f, mut ds) = open("b.ncs", &ncs(&recs));
    let t = ds.info().unwrap().traces[0].clone();
    assert_eq!(t.sweep_count, 1);
    assert_eq!(t.sample_count, 5120);
    assert_eq!(
        ds.read_trace(0, 0, 5000, 500).unwrap().channels[0].len(),
        120
    );
}

#[test]
fn ncs_truncated_record() {
    let mut b = ncs(&[(0, 512), (16_000, 512)]);
    b.truncate(b.len() - 100);
    let (_f, mut ds) = open("c.ncs", &b);
    let r = ds.check().unwrap();
    assert!(!r.ok);
    assert!(r.findings.iter().any(|f| f.code == "truncated"));
    assert_eq!(ds.info().unwrap().traces[0].sample_count, 512);
}

#[test]
fn short_file_is_corrupt() {
    let f = write("d.ncs", b"######## Neuralynx Data File Header\r\n");
    match NeuralynxReader.open(&f.path) {
        Err(e @ Error::Corrupt { .. }) => assert_eq!(e.exit_code(), 4),
        Err(e) => panic!("unexpected error {e}"),
        Ok(_) => panic!("accepted a header-only fragment"),
    }
}

#[test]
fn nev_events_table() {
    let mut b = header("-FileType Event\r\n-RecordSize 184\r\n");
    for (ts, id, ttl, label) in [
        (10u64, 11i16, 0i16, "Starting Recording"),
        (20, 20, 2, "TTL"),
        (30, 11, 0, "Stopping Recording"),
    ] {
        let mut rec = vec![0u8; 184];
        rec[2..4].copy_from_slice(&7i16.to_le_bytes());
        rec[4..6].copy_from_slice(&2i16.to_le_bytes());
        rec[6..14].copy_from_slice(&ts.to_le_bytes());
        rec[14..16].copy_from_slice(&id.to_le_bytes());
        rec[16..18].copy_from_slice(&ttl.to_le_bytes());
        rec[56..56 + label.len()].copy_from_slice(label.as_bytes());
        b.extend_from_slice(&rec);
    }
    let (_f, mut ds) = open("Events.nev", &b);
    let info = ds.info().unwrap();
    assert!(info.traces.is_empty());
    let t = &info.tables[0];
    assert_eq!(t.row_count, 3);
    assert_eq!(
        t.extra["labels"],
        serde_json::json!(["Starting Recording", "TTL", "Stopping Recording"])
    );
    let tab = ds.read_table(0, 1, 5).unwrap();
    assert_eq!(tab.columns[0], vec![20.0, 30.0]);
    assert_eq!(tab.columns[1], vec![20.0, 11.0]);
    assert_eq!(tab.columns[2], vec![2.0, 0.0]);
    assert_eq!(tab.columns[4], vec![1.0, 2.0]);
    assert!(ds.read_trace(0, 0, 0, 1).is_err());
    assert!(ds.check().unwrap().ok);
}

#[test]
fn ntt_waveforms_are_electrode_major_columns() {
    let mut b = header(
        "-FileType Spike\r\n-RecordSize 304\r\n-ADBitVolts 0.000001 0.000002 0.000003 0.000004\r\n-SamplingFrequency 32000\r\n",
    );
    let mut rec = vec![0u8; 304];
    rec[0..8].copy_from_slice(&99u64.to_le_bytes());
    rec[12..16].copy_from_slice(&5u32.to_le_bytes());
    rec[16..20].copy_from_slice(&(-7i32).to_le_bytes());
    for p in 0..32usize {
        for e in 0..4usize {
            let at = 48 + 2 * (p * 4 + e);
            rec[at..at + 2].copy_from_slice(&((p * 10 + e) as i16).to_le_bytes());
        }
    }
    b.extend_from_slice(&rec);
    let (_f, mut ds) = open("TT1.ntt", &b);
    let t = ds.info().unwrap().tables[0].clone();
    assert_eq!(t.columns.len(), 3 + 8 + 128);
    let tab = ds.read_table(0, 0, 10).unwrap();
    assert_eq!(tab.columns[0], vec![99.0]);
    assert_eq!(tab.columns[2], vec![5.0]);
    assert_eq!(tab.columns[3], vec![-7.0]);
    let col = |name: &str| t.columns.iter().position(|c| c.name == name).unwrap();
    // electrode 2, sample 3: raw 32 × 3 µV
    assert!((tab.columns[col("w2_3")][0] - 32.0 * 3.0).abs() < 1e-9);
    assert!((tab.columns[col("w0_31")][0] - 310.0).abs() < 1e-9);
}

#[test]
fn sniffing() {
    let f = write("x.ncs", &ncs(&[(0, 512)]));
    let head = std::fs::read(&f.path).unwrap();
    assert_eq!(
        NeuralynxReader.sniff(&head, &f.path).unwrap().confidence,
        openreadout_core::model::DetectConfidence::Definite
    );
    assert!(
        NeuralynxReader
            .sniff(b"hello", std::path::Path::new("a.txt"))
            .is_none()
    );
}

/// A recording directory: two channels on one grid join one trace; a channel with other segment
/// boundaries is a trace of its own; logs are companions; a foreign file disqualifies the
/// directory; a truncated member fails `check` (exit 4 from the CLI).
#[test]
fn a_directory_is_one_session() {
    let dir = tempfile::tempdir().unwrap();
    let grid = [(0, 512), (16_000, 512)];
    std::fs::write(dir.path().join("CSC1.ncs"), ncs(&grid)).unwrap();
    std::fs::write(dir.path().join("CSC2.ncs"), ncs(&grid)).unwrap();
    // the sample interval is the median record step, so the gap needs a few regular records
    let gapped = [
        (0, 512),
        (16_000, 512),
        (32_000, 512),
        (900_000, 512),
        (916_000, 512),
    ];
    std::fs::write(dir.path().join("CSC3.ncs"), ncs(&gapped)).unwrap();
    std::fs::write(dir.path().join("CheetahLogFile.txt"), b"log").unwrap();
    let det = NeuralynxReader.sniff(&[], dir.path()).unwrap();
    assert_eq!(det.format_id, "neuralynx");
    let mut ds = NeuralynxReader.open(dir.path()).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces.len(), 2);
    assert_eq!(info.traces[0].channels.len(), 2);
    assert_eq!(info.traces[0].sweep_count, 1);
    assert_eq!(info.traces[1].sweep_count, 2);
    assert_eq!(info.traces[0].channels[1].extra["source_file"], "CSC2.ncs");
    let t = ds.read_trace(0, 0, 510, 4).unwrap();
    assert_eq!(t.channels.len(), 2);
    assert_eq!(t.channels[0], t.channels[1]);
    assert_eq!(t.channels[0].len(), 4);
    assert!(ds.check().unwrap().ok);
    // a truncated member
    let mut b = ncs(&grid);
    b.truncate(b.len() - 100);
    std::fs::write(dir.path().join("CSC3.ncs"), b).unwrap();
    let r = NeuralynxReader.open(dir.path()).unwrap().check().unwrap();
    assert!(!r.ok);
    assert!(
        r.findings
            .iter()
            .any(|f| f.code == "truncated" && f.message.starts_with("CSC3.ncs"))
    );
    // one foreign file and the directory is walked file by file instead
    std::fs::write(dir.path().join("photo.tif"), b"II*\0").unwrap();
    assert!(NeuralynxReader.sniff(&[], dir.path()).is_none());
}

#[test]
fn memory_session_combines_channels_without_local_paths() {
    use openreadout_core::source::{Fs, Input, MemFs, MemSource};
    use std::sync::Arc;
    let grid = [(0, 512), (16_000, 512)];
    let fs = MemFs::new()
        .with(
            "memory/CSC1.ncs",
            Arc::new(MemSource::new("CSC1.ncs", ncs(&grid))),
        )
        .with(
            "memory/CSC2.ncs",
            Arc::new(MemSource::new("CSC2.ncs", ncs(&grid))),
        );
    let input = Input::new("memory", Fs::new(Arc::new(fs)));
    assert!(NeuralynxReader.sniff_input(&[], &input).is_some());
    let mut ds = NeuralynxReader.open_input(&input).unwrap();
    let t = ds.read_trace(0, 0, 510, 4).unwrap();
    assert_eq!(t.channels.len(), 2);
    assert_eq!(t.channels[0], t.channels[1]);
    assert!(ds.check().unwrap().ok);
}

/// One video-tracker record (the published layout) with `targets` non-zero targets.
fn nvt_record(start: u16, ts: u64, x: i32, y: i32, angle: i32, targets: usize) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&start.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1828u16.to_le_bytes());
    b.extend_from_slice(&ts.to_le_bytes());
    b.extend(std::iter::repeat_n(0xAAu8, 1600));
    b.extend_from_slice(&0i16.to_le_bytes());
    for v in [x, y, angle] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    for k in 0..50 {
        b.extend_from_slice(&(if k < targets { 0x0102_0304 } else { 0i32 }).to_le_bytes());
    }
    assert_eq!(b.len(), 1828);
    b
}

#[test]
fn nvt_positions() {
    let mut b = header(
        "-FileType Video\r\n-RecordSize 1828\r\n-SamplingFrequency 29.97\r\n-Resolution 720 480\r\n",
    );
    b.extend(nvt_record(0x800, 33_367, 120, 240, 90, 2));
    b.extend(nvt_record(0x800, 66_733, 0, 0, 0, 0));
    b.extend(nvt_record(0x7FF, 50_000, 5, 6, 7, 50));
    let (_f, mut ds) = open("VT1.nvt", &b);
    let info = ds.info().unwrap();
    let t = &info.tables[0];
    assert_eq!(t.row_count, 3);
    let names: Vec<_> = t.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["timestamp_us", "x", "y", "angle", "target_count"]);
    assert_eq!(t.extra["frame_rate_hz"], "29.97");
    let rows = ds.read_table(0, 0, 10).unwrap();
    assert_eq!(rows.columns[0], [33_367.0, 66_733.0, 50_000.0]);
    assert_eq!(rows.columns[1], [120.0, 0.0, 5.0]);
    assert_eq!(rows.columns[3], [90.0, 0.0, 7.0]);
    assert_eq!(rows.columns[4], [2.0, 0.0, 50.0]);
    let codes: Vec<_> = ds
        .check()
        .unwrap()
        .findings
        .iter()
        .map(|f| f.code.clone())
        .collect();
    assert!(codes.contains(&"bad_video_record".to_string()), "{codes:?}");
    assert!(
        codes.contains(&"timestamps_decrease".to_string()),
        "{codes:?}"
    );
    // a cut-off last record is truncation, and reads stop at whole records
    let (_g, mut cut) = open("VT2.nvt", &b[..b.len() - 10]);
    assert_eq!(cut.info().unwrap().tables[0].row_count, 2);
    assert!(
        cut.check()
            .unwrap()
            .findings
            .iter()
            .any(|f| f.code == "truncated")
    );
}
