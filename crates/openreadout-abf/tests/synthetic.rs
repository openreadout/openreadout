//! Synthetic ABF 1 and ABF 2 files built from the layout in docs/formats/abf.md, covering
//! scaling, variable-length sweeps, truncation and malformed headers.

use std::path::PathBuf;

use openreadout_abf::AbfReader;
use openreadout_core::model::Severity;
use openreadout_core::reader::FormatReader;
use openreadout_core::{Dataset, Error};

struct Built {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

fn write(bytes: &[u8]) -> Built {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.abf");
    std::fs::write(&path, bytes).unwrap();
    Built { _dir: dir, path }
}

fn put_u32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_i32(b: &mut [u8], at: usize, v: i32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_i16(b: &mut [u8], at: usize, v: i16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}
fn put_f32(b: &mut [u8], at: usize, v: f32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_i64(b: &mut [u8], at: usize, v: i64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

/// Section-map slot by our name order.
fn slot(name: &str) -> usize {
    76 + 16
        * openreadout_abf::SECTION_NAMES
            .iter()
            .position(|n| *n == name)
            .unwrap()
}

/// ABF 2: 2 channels (pA, mV), int16, `sweeps` sweeps of `per_sweep` samples, rate 10 kHz.
/// Channel 0 scale: 1 / 0.001 / 1 / 1 × 10 / 32768; channel 1 has offsets and a telegraph gain.
/// Optional synch array lengths (multiplexed samples) make sweeps variable.
fn abf2(sweeps: u32, per_sweep: u32, synch: Option<&[i32]>) -> Vec<u8> {
    let nch = 2u32;
    let total = synch.map_or(sweeps * per_sweep * nch, |s| s.iter().sum::<i32>() as u32);
    let mut b = vec![0u8; 512 * 6];
    b[0..4].copy_from_slice(b"ABF2");
    b[4..8].copy_from_slice(&[0, 0, 6, 2]); // 2.6.0.0
    put_u32(&mut b, 8, 512);
    put_u32(&mut b, 12, sweeps);
    put_u32(&mut b, 16, 20_200_229);
    put_u32(&mut b, 20, 3_723_004); // 01:02:03.004
    put_i16(&mut b, 30, 0);
    b[56..60].copy_from_slice(&[4, 3, 2, 1]);
    put_u32(&mut b, 60, 1);
    put_u32(&mut b, 72, 2);
    // protocol at block 1
    let s = slot("protocol");
    put_u32(&mut b, s, 1);
    put_u32(&mut b, s + 4, 512);
    put_i64(&mut b, s + 8, 1);
    let p = 512;
    put_i16(&mut b, p, 5);
    put_f32(&mut b, p + 2, 100.0);
    put_f32(&mut b, p + 14, 0.0);
    put_f32(&mut b, p + 110, 10.0);
    put_i32(&mut b, p + 118, 32768);
    put_i32(&mut b, p + 132, 7);
    // ADC at block 2, 128-byte entries
    let s = slot("adc");
    put_u32(&mut b, s, 2);
    put_u32(&mut b, s + 4, 128);
    put_i64(&mut b, s + 8, 2);
    for c in 0..2usize {
        let a = 1024 + 128 * c;
        put_i16(&mut b, a, c as i16);
        put_f32(&mut b, a + 28, 1.0);
        put_f32(&mut b, a + 40, if c == 0 { 0.001 } else { 0.01 });
        put_f32(&mut b, a + 44, if c == 0 { 0.0 } else { 2.5 });
        put_f32(&mut b, a + 48, 1.0);
        put_f32(&mut b, a + 52, if c == 0 { 0.0 } else { 0.5 });
        put_f32(&mut b, a + 6, 5.0);
        put_i16(&mut b, a + 2, i16::from(c == 1)); // telegraph on channel 1
        put_i32(&mut b, a + 74, 3 + 2 * c as i32);
        put_i32(&mut b, a + 78, 4 + 2 * c as i32);
    }
    // strings at block 3
    let mut strings = b"SSCH\x01\x00\x00\x00\x07\x00\x00\x00".to_vec();
    strings.extend_from_slice(&[0u8; 28]);
    strings
        .extend_from_slice(b"Clampex\0C:\\protocols\\steps.pro\0IN 0\0pA\0IN 1\0mV\0a comment\0");
    let s = slot("strings");
    put_u32(&mut b, s, 3);
    put_u32(&mut b, s + 4, strings.len() as u32);
    put_i64(&mut b, s + 8, 7);
    b[1536..1536 + strings.len()].copy_from_slice(&strings);
    // synch array at block 4
    if let Some(lens) = synch {
        let s = slot("synch_array");
        put_u32(&mut b, s, 4);
        put_u32(&mut b, s + 4, 8);
        put_i64(&mut b, s + 8, lens.len() as i64);
        for (i, l) in lens.iter().enumerate() {
            put_i32(&mut b, 2048 + 8 * i, 1000 * i as i32);
            put_i32(&mut b, 2048 + 8 * i + 4, *l);
        }
    }
    // data at block 6
    let s = slot("data");
    put_u32(&mut b, s, 6);
    put_u32(&mut b, s + 4, 2);
    put_i64(&mut b, s + 8, i64::from(total));
    for i in 0..total {
        let v = (i as i16).wrapping_mul(3) - 100;
        b.extend_from_slice(&v.to_le_bytes());
    }
    b
}

fn open_err(path: &std::path::Path) -> Error {
    match AbfReader.open(path) {
        Err(e) => e,
        Ok(_) => panic!("expected {} to be rejected", path.display()),
    }
}

fn open(bytes: &[u8]) -> (Built, Box<dyn Dataset>) {
    let f = write(bytes);
    let ds = AbfReader.open(&f.path).unwrap();
    (f, ds)
}

#[test]
fn abf2_info_and_scaling() {
    let (_f, mut ds) = open(&abf2(3, 4, None));
    let info = ds.info().unwrap();
    assert_eq!(info.format.id, "abf");
    assert_eq!(info.format_version.as_deref(), Some("2.6.0.0"));
    let t = &info.traces[0];
    assert_eq!(t.sweep_count, 3);
    assert_eq!(t.sample_count, 4);
    assert!((t.sample_rate_hz - 10_000.0).abs() < 1e-9);
    assert_eq!(t.channels.len(), 2);
    assert_eq!(t.channels[0].name, "IN 0");
    assert_eq!(t.channels[0].unit.as_deref(), Some("pA"));
    assert_eq!(t.channels[1].unit.as_deref(), Some("mV"));
    assert_eq!(t.name.as_deref(), Some("steps"));
    assert_eq!(t.extra["creator"], "Clampex");
    assert_eq!(t.extra["creator_version"], "1.2.3.4");
    assert_eq!(t.extra["comment"], "a comment");
    assert_eq!(t.extra["created_at"], "2020-02-29T01:02:03.004");
    // sweep 1 starts at interleaved sample 8 → raw values 3*8-100 .. for channel 0
    let tr = ds.read_trace(0, 1, 0, 100).unwrap();
    assert_eq!(tr.channels[0].len(), 4);
    let g0 = 1.0 / f64::from(0.001f32) / 1.0 / 1.0 * 10.0 / 32768.0;
    let raw0 = f64::from(3 * 8 - 100);
    assert!((tr.channels[0][0] - raw0 * g0).abs() < 1e-12);
    let g1 = 1.0 / f64::from(0.01f32) / 1.0 / 1.0 / 5.0 * 10.0 / 32768.0;
    let raw1 = f64::from(3 * 9 - 100);
    assert!((tr.channels[1][0] - (raw1 * g1 + 2.5 - 0.5)).abs() < 1e-12);
    // windowed read
    let w = ds.read_trace(0, 2, 3, 10).unwrap();
    assert_eq!(w.channels[0].len(), 1);
    assert_eq!(w.first_sample, 3);
    assert!(ds.read_trace(0, 3, 0, 1).is_err());
    assert!(ds.read_trace(1, 0, 0, 1).is_err());
    let r = ds.check().unwrap();
    assert!(r.ok, "{:?}", r.findings);
    let ls = ds.entries().unwrap();
    assert!(ls.iter().any(|e| e.name == "strings"));
    assert_eq!(ls.iter().filter(|e| e.kind == "sweep").count(), 3);
}

#[test]
fn abf2_variable_sweeps() {
    // multiplexed lengths 6, 10, 4 → 3, 5, 2 samples per channel
    let (_f, mut ds) = open(&abf2(3, 0, Some(&[6, 10, 4])));
    let info = ds.info().unwrap();
    let t = &info.traces[0];
    assert_eq!(t.sample_count, 5);
    assert_eq!(t.extra["sweep_sample_counts"], serde_json::json!([3, 5, 2]));
    let lens: Vec<usize> = (0..3)
        .map(|s| ds.read_trace(0, s, 0, u64::MAX).unwrap().channels[0].len())
        .collect();
    assert_eq!(lens, vec![3, 5, 2]);
    // sweep 2 starts at multiplexed sample 16
    let tr = ds.read_trace(0, 2, 0, 1).unwrap();
    let g0 = 1.0 / f64::from(0.001f32) * 10.0 / 32768.0;
    assert!((tr.channels[0][0] - f64::from(3 * 16 - 100) * g0).abs() < 1e-12);
    assert!(ds.check().unwrap().ok);
}

#[test]
fn abf2_truncated_data_is_corrupt() {
    let mut b = abf2(3, 4, None);
    b.truncate(b.len() - 10);
    let (_f, mut ds) = open(&b);
    let r = ds.check().unwrap();
    assert!(!r.ok);
    assert!(r.findings.iter().any(|f| f.code == "truncated"));
    assert!(matches!(
        ds.read_trace(0, 2, 0, 100),
        Err(Error::Corrupt { .. })
    ));
    // earlier sweeps still read
    assert_eq!(ds.read_trace(0, 0, 0, 100).unwrap().channels[0].len(), 4);
}

#[test]
fn abf2_header_only_is_rejected() {
    let b = abf2(3, 4, None);
    let f = write(&b[..200]);
    let e = open_err(&f.path);
    assert!(matches!(e, Error::Corrupt { .. }), "{e}");
    assert_eq!(e.exit_code(), 4);
}

#[test]
fn garbage_signature_is_rejected() {
    let mut b = abf2(1, 4, None);
    b[3] = b'X';
    let f = write(&b);
    let e = open_err(&f.path);
    assert!(matches!(e, Error::Corrupt { .. }));
}

#[test]
fn abf2_zero_channels_is_rejected() {
    let mut b = abf2(1, 4, None);
    put_i64(&mut b, slot("adc") + 8, 0);
    let f = write(&b);
    assert!(matches!(open_err(&f.path), Error::Corrupt { .. }));
}

/// ABF 1.83: extended 6144-byte header, 1 channel on physical ADC 3, gap-free, 20 µs interval.
fn abf1(samples: i32) -> Vec<u8> {
    let mut b = vec![0u8; 6144];
    b[0..4].copy_from_slice(b"ABF ");
    put_f32(&mut b, 4, 1.83);
    put_i16(&mut b, 8, 3);
    put_i32(&mut b, 10, samples);
    put_i32(&mut b, 16, 7); // chunks; gap-free → 1 sweep
    put_i32(&mut b, 20, 990_101);
    put_i32(&mut b, 24, 61);
    put_i32(&mut b, 40, 12);
    put_i16(&mut b, 100, 0);
    put_i16(&mut b, 120, 1);
    put_f32(&mut b, 122, 20.0);
    put_f32(&mut b, 244, 10.0);
    put_i32(&mut b, 252, 32768);
    b[294..301].copy_from_slice(b"Clampex");
    put_i16(&mut b, 366, 250);
    put_i16(&mut b, 410, 3);
    b[442 + 30..442 + 34].copy_from_slice(b"Vm 3");
    b[602 + 24..602 + 26].copy_from_slice(b"mV");
    for (base, v) in [
        (730, 2.0f32),
        (922, 0.1),
        (986, 1.0),
        (1050, 1.0),
        (1114, 0.0),
    ] {
        put_f32(&mut b, base + 12, v);
    }
    put_i16(&mut b, 4512 + 6, 1);
    put_f32(&mut b, 4576 + 12, 4.0);
    b[4898..4912].copy_from_slice(b"C:\\p\\gapf.pro\0");
    for i in 0..samples {
        b.extend_from_slice(&((i * 7) as i16).to_le_bytes());
    }
    b
}

#[test]
fn abf1_extended_header() {
    let (_f, mut ds) = open(&abf1(50));
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("1.83"));
    let t = &info.traces[0];
    assert_eq!(t.sweep_count, 1);
    assert_eq!(t.sample_count, 50);
    assert!((t.sample_rate_hz - 50_000.0).abs() < 1e-9);
    assert_eq!(t.channels[0].name, "Vm 3");
    assert_eq!(t.channels[0].unit.as_deref(), Some("mV"));
    assert_eq!(t.channels[0].extra["adc_number"], 3);
    assert_eq!(t.name.as_deref(), Some("gapf"));
    assert_eq!(t.extra["created_at"], "1999-01-01T00:01:01.250");
    let g = 1.0 / f64::from(0.1f32) / 1.0 / 2.0 / 4.0 * 10.0 / 32768.0;
    let tr = ds.read_trace(0, 0, 10, 2).unwrap();
    assert!((tr.channels[0][0] - (70.0 * g + 1.0)).abs() < 1e-12);
    assert!(ds.check().unwrap().ok);
}

#[test]
fn abf1_truncated() {
    let mut b = abf1(50);
    b.truncate(6144 + 40);
    let (_f, mut ds) = open(&b);
    let r = ds.check().unwrap();
    assert!(!r.ok);
    let f = r.findings.iter().find(|f| f.code == "truncated").unwrap();
    assert_eq!(f.severity, Severity::Error);
    assert!(matches!(
        ds.read_trace(0, 0, 0, 50),
        Err(Error::Corrupt { .. })
    ));
    assert_eq!(ds.read_trace(0, 0, 0, 20).unwrap().channels[0].len(), 20);
}

#[test]
fn sniff_by_signature_and_extension() {
    let f = write(&abf1(4));
    let det = AbfReader.sniff(b"ABF2\0\0\0\x02", &f.path).unwrap();
    assert_eq!(
        det.confidence,
        openreadout_core::model::DetectConfidence::Definite
    );
    let other = std::path::Path::new("x.abf");
    assert_eq!(
        AbfReader.sniff(b"nope", other).unwrap().confidence,
        openreadout_core::model::DetectConfidence::ExtensionOnly
    );
    assert!(
        AbfReader
            .sniff(b"nope", std::path::Path::new("x.bin"))
            .is_none()
    );
}
