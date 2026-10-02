//! Synthetic PLX files written from scratch (layout in docs/formats/plexon.md).

use std::path::Path;

use openreadout_core::FormatReader;
use openreadout_core::model::Severity;
use openreadout_plexon::{
    CONTINUOUS_HEADER_LEN, EVENT_HEADER_LEN, FILE_HEADER_LEN, PlexonReader, SPIKE_HEADER_LEN,
};

fn put_i32(b: &mut [u8], at: usize, v: i32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn name(b: &mut [u8], at: usize, s: &str) {
    b[at..at + s.len()].copy_from_slice(s.as_bytes());
}

fn block(out: &mut Vec<u8>, kind: u16, ts: u64, ch: u16, unit: u16, samples: &[i16]) {
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&u16::try_from(ts >> 32).unwrap().to_le_bytes());
    out.extend_from_slice(&u32::try_from(ts & 0xFFFF_FFFF).unwrap().to_le_bytes());
    out.extend_from_slice(&ch.to_le_bytes());
    out.extend_from_slice(&unit.to_le_bytes());
    let waves: u16 = u16::from(!samples.is_empty());
    out.extend_from_slice(&waves.to_le_bytes());
    out.extend_from_slice(&u16::try_from(samples.len()).unwrap().to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
}

/// Version 106, 40 kHz clock; one spike channel (ch 1), one event channel (ch 7), two
/// continuous channels at 1 kHz (ch 0 and 1, the second on a different start tick).
fn plx(gap: bool) -> Vec<u8> {
    let mut h = vec![0u8; FILE_HEADER_LEN as usize];
    h[..4].copy_from_slice(b"PLEX");
    put_i32(&mut h, 4, 106);
    put_i32(&mut h, 136, 40_000);
    put_i32(&mut h, 140, 1);
    put_i32(&mut h, 144, 1);
    put_i32(&mut h, 148, 2);
    put_i32(&mut h, 152, 4);
    put_i32(&mut h, 156, 1);
    for (k, v) in [2024, 5, 6, 7, 8, 9].iter().enumerate() {
        put_i32(&mut h, 160 + 4 * k, *v);
    }
    put_i32(&mut h, 188, 40_000);
    h[202] = 12; // bits per spike sample
    h[203] = 12;
    h[204..206].copy_from_slice(&3000u16.to_le_bytes());
    h[206..208].copy_from_slice(&5000u16.to_le_bytes());
    h[208..210].copy_from_slice(&1000u16.to_le_bytes());
    let mut spike = vec![0u8; SPIKE_HEADER_LEN as usize];
    name(&mut spike, 0, "SPK01");
    put_i32(&mut spike, 64, 1);
    put_i32(&mut spike, 80, 2);
    let mut event = vec![0u8; EVENT_HEADER_LEN as usize];
    name(&mut event, 0, "Strobed");
    put_i32(&mut event, 32, 7);
    let mut cont = Vec::new();
    for (ch, nm) in [(0, "FP01"), (1, "FP02")] {
        let mut c = vec![0u8; CONTINUOUS_HEADER_LEN as usize];
        name(&mut c, 0, nm);
        put_i32(&mut c, 32, ch);
        put_i32(&mut c, 36, 1000);
        put_i32(&mut c, 40, 1);
        put_i32(&mut c, 44, 1);
        put_i32(&mut c, 48, 1000);
        cont.extend(c);
    }
    let mut d = Vec::new();
    // channel 0: 3 + 2 samples contiguous (40 ticks per sample), or a gap before the second block
    block(&mut d, 5, 0, 0, 0, &[1, 2, 3]);
    block(&mut d, 5, if gap { 400 } else { 120 }, 0, 0, &[4, 5]);
    // channel 1 starts later: its own trace
    block(&mut d, 5, 40, 1, 0, &[10, 20]);
    block(&mut d, 1, 1000, 1, 2, &[-4, 8, 12, 16]);
    block(&mut d, 4, 2000, 7, 65_281, &[]);
    let mut out = h;
    out.extend(spike);
    out.extend(event);
    out.extend(cont);
    out.extend(d);
    out
}

fn write(dir: &Path, bytes: &[u8]) -> std::path::PathBuf {
    let p = dir.join("x.plx");
    std::fs::write(&p, bytes).unwrap();
    p
}

#[test]
fn reads_traces_spikes_and_events() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), &plx(false));
    assert!(
        PlexonReader
            .sniff(&std::fs::read(&p).unwrap(), &p)
            .is_some()
    );
    let mut ds = PlexonReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces.len(), 2);
    let t = &info.traces[0];
    assert_eq!((t.sample_count, t.sweep_count, t.channels.len()), (5, 1, 1));
    assert_eq!(t.name.as_deref(), Some("FP"));
    assert_eq!(t.channels[0].unit.as_deref(), Some("mV"));
    // 5000 mV / (0.5 × 2^12 × 1 × 1000)
    let scale = 5000.0 / (0.5 * 4096.0 * 1000.0);
    assert!((t.channels[0].scale - scale).abs() < 1e-15);
    let tr = ds.read_trace(0, 0, 1, 10).unwrap();
    assert_eq!(tr.channels[0], [2.0, 3.0, 4.0, 5.0].map(|v| v * scale));
    assert_eq!(info.traces[1].start_s, Some(40.0 / 40_000.0));
    assert_eq!(info.tables.len(), 2);
    let spikes = ds.read_table(0, 0, 10).unwrap();
    // 3000 mV / (0.5 × 2^12 × gain 2 × preamp 1000)
    let ws = 3000.0 / (0.5 * 4096.0 * 2.0 * 1000.0);
    assert_eq!(spikes.columns[0], [1000.0 / 40_000.0]);
    assert_eq!(spikes.columns[2], [1.0]);
    assert_eq!(spikes.columns[3], [2.0]);
    assert_eq!(spikes.columns[4], [-4.0 * ws]);
    let events = ds.read_table(1, 0, 10).unwrap();
    assert_eq!(events.columns[2], [7.0]);
    assert_eq!(events.columns[3], [65_281.0]);
    assert!(ds.check().unwrap().ok);
}

#[test]
fn a_gap_starts_a_sweep() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), &plx(true));
    let mut ds = PlexonReader.open(&p).unwrap();
    let t = &ds.info().unwrap().traces[0];
    assert_eq!(t.sweep_count, 2);
    let s1 = ds.read_trace(0, 1, 0, 10).unwrap();
    assert_eq!(s1.channels[0].len(), 2);
}

#[test]
fn truncation_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut b = plx(false);
    b.truncate(b.len() - 3);
    let p = write(dir.path(), &b);
    let mut ds = PlexonReader.open(&p).unwrap();
    let r = ds.check().unwrap();
    assert!(!r.ok);
    assert!(
        r.findings
            .iter()
            .any(|f| f.severity == Severity::Error && f.code == "truncated")
    );
    // a file cut inside its headers does not open
    let p2 = write(dir.path(), &plx(false)[..8000]);
    assert!(matches!(
        PlexonReader.open(&p2),
        Err(openreadout_core::Error::Corrupt { .. })
    ));
}

#[test]
fn plx_from_bytes_without_a_local_file() {
    let input = openreadout_core::source::Input::from_bytes("memory.plx", plx(false));
    let mut ds = PlexonReader.open_input(&input).unwrap();
    assert!(!ds.info().unwrap().traces.is_empty());
    assert!(!ds.read_trace(0, 0, 0, 5).unwrap().channels.is_empty());
    assert!(ds.check().unwrap().ok);
}
