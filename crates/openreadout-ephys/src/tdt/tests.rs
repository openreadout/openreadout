//! Synthetic-block tests of the TDT reader.

use super::tsq::*;
use super::*;
use openreadout_core::FormatReader;

/// One 40-byte index record.
#[allow(clippy::too_many_arguments)]
fn rec(
    size: i32,
    kind: i32,
    name: &[u8; 4],
    channel: u16,
    sort: u16,
    time: f64,
    offset: [u8; 8],
    format: i32,
    rate: f32,
) -> Vec<u8> {
    let mut v = Vec::with_capacity(40);
    v.extend_from_slice(&size.to_le_bytes());
    v.extend_from_slice(&kind.to_le_bytes());
    v.extend_from_slice(name);
    v.extend_from_slice(&channel.to_le_bytes());
    v.extend_from_slice(&sort.to_le_bytes());
    v.extend_from_slice(&time.to_le_bytes());
    v.extend_from_slice(&offset);
    v.extend_from_slice(&format.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v
}

fn off(o: u64) -> [u8; 8] {
    o.to_le_bytes()
}

/// A block: stream `Wave` (2 channels, int16, 4 samples per chunk, 3 chunks, 100 Hz), snippet
/// store `eSpk` (2 snippets of 3 float32 samples), strobe store `Tri/` with offsets in `Tri\`.
/// Returns (tsq, tev).
fn synthetic() -> (Vec<u8>, Vec<u8>) {
    let t0 = 1_600_000_000.0;
    let mut tev = Vec::new();
    let mut recs: Vec<Vec<u8>> = Vec::new();
    recs.push(rec(
        20,
        EVENT_MARK,
        &[1, 0, 0, 0],
        0,
        0,
        t0,
        off(40),
        0,
        0.0,
    ));
    for k in 0..3u16 {
        for ch in 1..=2u16 {
            let at = tev.len() as u64;
            for i in 0..4i16 {
                let v = (ch as i16) * 1000 + (k as i16) * 4 + i;
                tev.extend_from_slice(&v.to_le_bytes());
            }
            recs.push(rec(
                12,
                EVENT_STREAM,
                b"Wave",
                ch,
                0,
                t0 + f64::from(k) * 0.04 + 0.001,
                off(at),
                2,
                100.0,
            ));
        }
        if k < 2 {
            let at = tev.len() as u64;
            for i in 0..3 {
                tev.extend_from_slice(&(f32::from(k) + 0.5 * i as f32).to_le_bytes());
            }
            recs.push(rec(
                13,
                EVENT_SNIPPET,
                b"eSpk",
                1 + k,
                k,
                t0 + 0.05 * f64::from(k),
                off(at),
                0,
                24414.0,
            ));
        }
    }
    recs.push(rec(
        10,
        EVENT_STROBE_ON,
        b"Tri/",
        0,
        0,
        t0 + 0.02,
        7.0f64.to_le_bytes(),
        4,
        0.0,
    ));
    let buddy = u16::from_le_bytes(*b"Tr");
    let buddy2 = u16::from_le_bytes(*b"i/");
    recs.push(rec(
        10,
        EVENT_STROBE_OFF,
        b"Tri\\",
        buddy,
        buddy2,
        t0 + 0.07,
        7.0f64.to_le_bytes(),
        4,
        0.0,
    ));
    recs.push(rec(
        10,
        EVENT_STROBE_ON,
        b"Tri/",
        0,
        0,
        t0 + 0.09,
        8.0f64.to_le_bytes(),
        4,
        0.0,
    ));
    recs.push(rec(
        20,
        EVENT_MARK,
        &[2, 0, 0, 0],
        0,
        0,
        t0 + 0.12,
        off(tev.len() as u64),
        0,
        0.0,
    ));
    let n = (recs.len() + 1) as i32 * 40;
    let mut tsq = rec(n, 0, &[0; 4], 0, 0, 0.0, off(0), 0, 0.0);
    for r in recs {
        tsq.extend_from_slice(&r);
    }
    (tsq, tev)
}

fn write_block(dir: &Path, stem: &str, tsq: &[u8], tev: &[u8]) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(format!("{stem}.tsq")), tsq).unwrap();
    std::fs::write(dir.join(format!("{stem}.tev")), tev).unwrap();
    dir.join(format!("{stem}.tsq"))
}

#[test]
fn reads_a_synthetic_block() {
    let d = tempfile::tempdir().unwrap();
    let (tsq, tev) = synthetic();
    let p = write_block(&d.path().join("T_B1"), "T_B1", &tsq, &tev);
    assert!(tsq::looks_like_tsq(&tsq));
    let det = TdtReader.sniff(&tsq, &p).unwrap();
    assert_eq!(
        det.confidence,
        openreadout_core::model::DetectConfidence::Definite
    );
    let det = TdtReader.sniff(&[], &d.path().join("T_B1")).unwrap();
    assert_eq!(
        det.confidence,
        openreadout_core::model::DetectConfidence::Definite
    );
    let mut ds = TdtReader.open(&d.path().join("T_B1")).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces.len(), 1);
    let t = &info.traces[0];
    assert_eq!(t.name.as_deref(), Some("Wave"));
    assert_eq!(t.sample_count, 12);
    assert_eq!(t.sweep_count, 1);
    assert_eq!(t.sample_rate_hz, 100.0);
    assert_eq!(t.channels[1].name, "Wave 2");
    assert_eq!(t.channels[0].dtype, "int16");
    assert!((t.start_s.unwrap() - 0.001).abs() < 1e-6);
    assert_eq!(t.extra["recorded_at"], "2020-09-13T12:26:40.000Z");
    let s = ds.read_trace(0, 0, 3, 6).unwrap();
    assert_eq!(
        s.channels[0],
        vec![1003.0, 1004.0, 1005.0, 1006.0, 1007.0, 1008.0]
    );
    assert_eq!(s.channels[1][0], 2003.0);
    assert_eq!(info.tables.len(), 2);
    let spikes = ds.read_table(0, 0, 10).unwrap();
    assert_eq!(spikes.columns.len(), 5 + 3);
    assert_eq!(spikes.columns[3], vec![1.0, 2.0]);
    assert_eq!(spikes.columns[4], vec![0.0, 1.0]);
    assert_eq!(spikes.columns[6], vec![0.5, 1.5]);
    let epocs = ds.read_table(1, 0, 10).unwrap();
    assert_eq!(epocs.columns[3], vec![7.0, 8.0]);
    assert!((epocs.columns[4][0] - 0.05).abs() < 1e-6);
    assert!(epocs.columns[4][1].is_nan());
    let r = ds.check().unwrap();
    assert!(
        !r.findings
            .iter()
            .any(|f| f.severity == openreadout_core::model::Severity::Error),
        "{:?}",
        r.findings
    );
}

#[test]
fn a_tank_makes_blocks_sweeps() {
    let d = tempfile::tempdir().unwrap();
    let (tsq, tev) = synthetic();
    write_block(&d.path().join("tank/B1"), "tank_B1", &tsq, &tev);
    // a later block with one chunk fewer
    let mut tsq2 = tsq.clone();
    for r in tsq2.chunks_mut(40).skip(1) {
        let t = f64::from_le_bytes(r[16..24].try_into().unwrap());
        r[16..24].copy_from_slice(&(t + 100.0).to_le_bytes());
    }
    write_block(
        &d.path().join("tank/B2"),
        "tank_B2",
        &tsq2,
        &tev[..tev.len() - 8],
    );
    let mut ds = TdtReader.open(&d.path().join("tank")).unwrap();
    let info = ds.info().unwrap();
    let t = &info.traces[0];
    assert_eq!(t.sweep_count, 2);
    assert_eq!(t.extra["sweep_sample_counts"], serde_json::json!([12, 8]));
    assert_eq!(
        t.extra["sweep_starts_s"][1].as_f64().unwrap().round(),
        100.0
    );
    assert_eq!(ds.read_trace(0, 1, 0, 100).unwrap().channels[0].len(), 8);
    let r = ds.check().unwrap();
    assert!(r.findings.iter().any(|f| f.code == "truncated"));
}

#[test]
fn damaged_indexes_give_clean_errors() {
    let d = tempfile::tempdir().unwrap();
    let (tsq, tev) = synthetic();
    // too short
    let p = write_block(&d.path().join("a"), "a", &tsq[..50], &tev);
    assert!(TdtReader.open(&p).is_err());
    // every prefix and a few byte flips must not panic
    for cut in (0..tsq.len()).step_by(7) {
        let p = write_block(&d.path().join("b"), "b", &tsq[..cut], &tev);
        if let Ok(mut ds) = TdtReader.open(&p) {
            let info = ds.info().unwrap();
            for t in &info.traces {
                let _ = ds.read_trace(t.index, 0, 0, 1000);
            }
            for t in &info.tables {
                let _ = ds.read_table(t.index, 0, 1000);
            }
            let _ = ds.check();
        }
    }
    for at in (0..tsq.len()).step_by(5) {
        let mut bad = tsq.clone();
        bad[at] ^= 0xA5;
        let p = write_block(&d.path().join("c"), "c", &bad, &tev);
        if let Ok(mut ds) = TdtReader.open(&p) {
            let info = ds.info().unwrap();
            for t in &info.traces {
                let _ = ds.read_trace(t.index, 0, 0, 1000);
            }
            for t in &info.tables {
                let _ = ds.read_table(t.index, 0, 1000);
            }
        }
    }
}

#[test]
fn sev_names_and_store_headers() {
    assert_eq!(
        sev_name_parts("blk_B24", "blk_B24_Wav5_Ch12.sev"),
        Some(("Wav5".into(), 12))
    );
    assert_eq!(sev_name_parts("blk", "other_Wav5_Ch1.sev"), None);
    let h = dataset::parse_store_headers(
        b"\x01junk[STOREHDRITEM]NAME=StoreName;TYPE=T;VALUE=LFPs;\nNAME=NumChan;TYPE=L;VALUE=16;\n[STOREHDRITEM]NAME=StoreName;TYPE=T;VALUE=Tick;",
    );
    assert_eq!(h.len(), 2);
    assert_eq!(h[0]["NumChan"], "16");
}

#[test]
fn files_without_an_index_are_refused() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("x_Wav5_Ch1.sev"), b"SEV").unwrap();
    let Err(e) = TdtReader.open(&d.path().join("x_Wav5_Ch1.sev")) else {
        panic!("opened a .sev without an index");
    };
    assert_eq!(e.exit_code(), 6);
}
