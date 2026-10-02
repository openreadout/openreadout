//! Synthetic PL2 files written from scratch from our own layout notes (docs/formats/plexon.md).

use openreadout_core::FormatReader;
use openreadout_core::model::Severity;
use openreadout_plexon::{PL2_FILE_HEADER_LEN, PlexonReader, record_len};

fn put(b: &mut [u8], at: usize, v: &[u8]) {
    b[at..at + v.len()].copy_from_slice(v);
}

fn channel_header(
    kind: u8,
    source: u8,
    len: usize,
    name: &str,
    ch: u32,
    rate: f64,
    vpc: f64,
) -> Vec<u8> {
    let mut r = vec![0u8; len];
    r[0] = kind;
    r[1] = source;
    put(
        &mut r,
        2,
        &u16::try_from((len - 16) / 2).unwrap().to_le_bytes(),
    );
    put(&mut r, 4, &ch.to_le_bytes());
    put(&mut r, 16, name.as_bytes());
    put(&mut r, 0x50, &u32::from(source).to_le_bytes());
    put(&mut r, 0x54, &ch.to_le_bytes());
    put(&mut r, 0x58, &1u32.to_le_bytes());
    put(&mut r, 0x5C, &1u32.to_le_bytes());
    if kind != 0xD6 {
        put(&mut r, 0x60, b"Volts");
        put(&mut r, 0x70, &rate.to_le_bytes());
        put(&mut r, 0x78, &vpc.to_le_bytes());
    }
    r
}

fn record(kind: u8, source: u8, words: u16, fields: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut r = vec![kind, source];
    r.extend_from_slice(&words.to_le_bytes());
    r.extend_from_slice(fields);
    r.resize(16, 0);
    r.extend_from_slice(payload);
    let len = usize::try_from(record_len(u64::from(words))).unwrap();
    r.resize(len, 0);
    r
}

/// One spike channel (source 6, 2 spikes of 3 samples, units 0 and 2), one analog channel
/// (source 7, 1 kHz, two records of 3 + 2 samples, contiguous or with a gap), one digital
/// channel (source 9, channel 17, two events), then the end record.
fn pl2(gap: bool) -> Vec<u8> {
    let mut header = vec![0u8; usize::try_from(PL2_FILE_HEADER_LEN).unwrap()];
    header[0] = 0xFE;
    put(&mut header, 10, b"PLEXON");
    put(&mut header, 0x1E0, b"TestWriter");
    for (k, v) in [11u32, 12, 13, 4, 8, 124, 3, 247, 0].iter().enumerate() {
        put(&mut header, 0x230 + 4 * k, &v.to_le_bytes());
    }
    put(&mut header, 0x258, &40_000f64.to_le_bytes());
    for (at, v) in [(0x260, 3u32), (0x264, 1), (0x26C, 1), (0x274, 1)] {
        put(&mut header, at, &v.to_le_bytes());
    }
    let mut spike = channel_header(0xD5, 6, 2592, "SPK01", 1, 40_000.0, 1e-6);
    put(&mut spike, 0x80, &3u32.to_le_bytes());
    put(&mut spike, 0xA0, &1u64.to_le_bytes()); // unit 0: 1 spike
    put(&mut spike, 0xB0, &1u64.to_le_bytes()); // unit 2: 1 spike
    let analog = channel_header(0xD4, 7, 512, "FP01", 1, 1000.0, 2e-6);
    let digital = channel_header(0xD6, 9, 368, "EVT17", 17, 0.0, 0.0);
    let headers_end = header.len() + spike.len() + analog.len() + digital.len();
    let mut data = Vec::new();
    let samples = |v: &[i16]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let mut fields = vec![];
    fields.extend_from_slice(&1u16.to_le_bytes());
    fields.extend_from_slice(&3u16.to_le_bytes());
    fields.extend_from_slice(&40u64.to_le_bytes());
    data.extend(record(0x42, 7, 3, &fields, &samples(&[1, 2, 3])));
    let mut fields = vec![];
    fields.extend_from_slice(&1u16.to_le_bytes());
    fields.extend_from_slice(&2u16.to_le_bytes());
    fields.extend_from_slice(&(if gap { 400u64 } else { 160 }).to_le_bytes());
    data.extend(record(0x42, 7, 2, &fields, &samples(&[4, 5])));
    let mut fields = vec![];
    fields.extend_from_slice(&1u16.to_le_bytes());
    fields.extend_from_slice(&3u16.to_le_bytes());
    fields.extend_from_slice(&2u32.to_le_bytes());
    let mut payload = Vec::new();
    for ts in [100u64, 900] {
        payload.extend_from_slice(&ts.to_le_bytes());
    }
    for u in [0u16, 2] {
        payload.extend_from_slice(&u.to_le_bytes());
    }
    payload.extend(samples(&[-1, -2, -3, 4, 5, 6]));
    data.extend(record(0x31, 6, 2 * 8, &fields, &payload));
    let mut fields = vec![];
    fields.extend_from_slice(&17u16.to_le_bytes());
    fields.extend_from_slice(&2u16.to_le_bytes());
    let mut payload = Vec::new();
    for ts in [500u64, 700] {
        payload.extend_from_slice(&ts.to_le_bytes());
    }
    for v in [1u16, 0xFF01] {
        payload.extend_from_slice(&v.to_le_bytes());
    }
    data.extend(record(0x5A, 9, 10, &fields, &payload));
    let mut end = vec![0u8; 20];
    end[8..16].copy_from_slice(&1000u64.to_le_bytes());
    data.extend(record(0x59, 0, 10, &[], &end));
    let data_start = headers_end;
    let footer = data_start + data.len();
    put(&mut header, 0x20, &(headers_end as u64).to_le_bytes());
    put(&mut header, 0x28, &(data_start as u64).to_le_bytes());
    put(&mut header, 0x30, &(footer as u64).to_le_bytes());
    put(&mut header, 0x38, &(footer as u64).to_le_bytes());
    put(&mut header, 0x48, &1000u64.to_le_bytes());
    let mut out = header;
    out.extend(spike);
    out.extend(analog);
    out.extend(digital);
    out.extend(data);
    out
}

#[test]
fn reads_analog_spikes_and_events() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.pl2");
    std::fs::write(&path, pl2(false)).unwrap();
    let head = std::fs::read(&path).unwrap();
    assert!(PlexonReader.sniff(&head, &path).is_some());
    let mut ds = PlexonReader.open(&path).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("PL2"));
    let t = &info.traces[0];
    assert_eq!((t.sample_count, t.sweep_count), (5, 1));
    assert_eq!(t.channels[0].unit.as_deref(), Some("V"));
    assert_eq!(t.start_s, Some(40.0 / 40_000.0));
    assert_eq!(
        ds.read_trace(0, 0, 1, 10).unwrap().channels[0],
        [2.0, 3.0, 4.0, 5.0].map(|v| v * 2e-6)
    );
    let spikes = ds.read_table(0, 0, 10).unwrap();
    assert_eq!(spikes.columns[1], [100.0, 900.0]);
    assert_eq!(spikes.columns[4], [0.0, 2.0]);
    assert_eq!(spikes.columns[5], [-1e-6, 4e-6]);
    assert_eq!(spikes.columns[7], [-3e-6, 6e-6]);
    let events = ds.read_table(1, 1, 10).unwrap();
    assert_eq!(events.columns[3], [17.0]);
    assert_eq!(events.columns[4], [65_281.0]);
    let report = ds.check().unwrap();
    assert!(
        report.ok && report.findings.is_empty(),
        "{:?}",
        report.findings
    );
}

#[test]
fn a_gap_starts_a_sweep() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("x.pl2");
    std::fs::write(&p, pl2(true)).unwrap();
    let ds = PlexonReader.open(&p).unwrap();
    assert_eq!(ds.info().unwrap().traces[0].sweep_count, 2);
}

#[test]
fn truncation_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut b = pl2(false);
    let cut = b.len() - 100;
    b.truncate(cut);
    let p = dir.path().join("x.pl2");
    std::fs::write(&p, &b).unwrap();
    let mut ds = PlexonReader.open(&p).unwrap();
    let r = ds.check().unwrap();
    assert!(!r.ok);
    assert!(
        r.findings
            .iter()
            .any(|f| f.severity == Severity::Error && f.code == "truncated")
    );
    std::fs::write(&p, &pl2(false)[..2000]).unwrap();
    assert!(matches!(
        PlexonReader.open(&p),
        Err(openreadout_core::Error::Corrupt { .. })
    ));
}

#[test]
fn pl2_from_bytes_without_a_local_file() {
    let input = openreadout_core::source::Input::from_bytes("memory.pl2", pl2(false));
    let mut ds = PlexonReader.open_input(&input).unwrap();
    assert!(!ds.info().unwrap().traces.is_empty());
    assert!(!ds.read_trace(0, 0, 0, 5).unwrap().channels.is_empty());
    assert!(ds.check().unwrap().ok);
}
