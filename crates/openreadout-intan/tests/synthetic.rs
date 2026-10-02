//! Synthetic RHD/RHS files built from Intan's published layout: block parts at their rates,
//! scaling, digital lines, stimulation decoding, truncation.

use std::path::PathBuf;

use openreadout_core::Error;
use openreadout_core::reader::FormatReader;
use openreadout_intan::{IntanReader, RHD_MAGIC, RHS_MAGIC};

fn qs(b: &mut Vec<u8>, s: &str) {
    let u: Vec<u16> = s.encode_utf16().collect();
    b.extend_from_slice(&((u.len() * 2) as u32).to_le_bytes());
    for x in u {
        b.extend_from_slice(&x.to_le_bytes());
    }
}
fn i16s(b: &mut Vec<u8>, v: &[i16]) {
    for x in v {
        b.extend_from_slice(&x.to_le_bytes());
    }
}
fn f32s(b: &mut Vec<u8>, v: &[f32]) {
    for x in v {
        b.extend_from_slice(&x.to_le_bytes());
    }
}

/// (native name, signal code, native order)
fn channel(b: &mut Vec<u8>, name: &str, code: i16, order: i16, rhs: bool) {
    qs(b, name);
    qs(b, "");
    i16s(b, &[order, order, code, 1, order]);
    if rhs {
        i16s(b, &[0]);
    }
    i16s(b, &[0, 0, 0, 0, 0]);
    f32s(b, &[1000.0, -30.0]);
}

/// RHD 1.5: 1 amplifier, 1 aux, 1 supply, 1 temperature sensor, 1 board ADC (mode 0), digital
/// inputs 2 and 5; N = 60; `blocks` data blocks.
fn rhd(blocks: usize) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&RHD_MAGIC.to_le_bytes());
    i16s(&mut b, &[1, 5]);
    f32s(&mut b, &[20000.0]);
    i16s(&mut b, &[1]);
    f32s(&mut b, &[1.0, 0.1, 7500.0, 1.0, 0.1, 7500.0]);
    i16s(&mut b, &[2]);
    f32s(&mut b, &[1000.0, 1000.0]);
    qs(&mut b, "a note");
    qs(&mut b, "");
    qs(&mut b, "");
    i16s(&mut b, &[1, 0]); // 1 temperature sensor, board mode 0
    i16s(&mut b, &[2]); // two groups
    qs(&mut b, "Port A");
    qs(&mut b, "A");
    i16s(&mut b, &[1, 3, 1]);
    channel(&mut b, "A-000", 0, 0, false);
    channel(&mut b, "A-AUX1", 1, 32, false);
    channel(&mut b, "A-VDD1", 2, 33, false);
    qs(&mut b, "Board");
    qs(&mut b, "B");
    i16s(&mut b, &[1, 3, 0]);
    channel(&mut b, "ADC-00", 3, 0, false);
    channel(&mut b, "DIN-02", 4, 2, false);
    channel(&mut b, "DIN-05", 4, 5, false);
    let n = 60usize;
    let mut t = 100i32;
    for blk in 0..blocks {
        for _ in 0..n {
            b.extend_from_slice(&t.to_le_bytes());
            t += 1;
        }
        for i in 0..n {
            b.extend_from_slice(&(32768u16 + (blk * n + i) as u16).to_le_bytes());
        }
        for i in 0..n / 4 {
            b.extend_from_slice(&((1000 + blk * 15 + i) as u16).to_le_bytes());
        }
        b.extend_from_slice(&45000u16.to_le_bytes());
        b.extend_from_slice(&3712i16.to_le_bytes());
        for i in 0..n {
            b.extend_from_slice(&(i as u16).to_le_bytes());
        }
        for i in 0..n {
            let w: u16 = if i % 2 == 0 { 0b0010_0100 } else { 0b0000_0100 } | 0x0200;
            b.extend_from_slice(&w.to_le_bytes());
        }
    }
    b
}

fn write(name: &str, b: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join(name);
    std::fs::write(&p, b).unwrap();
    (d, p)
}

#[test]
fn rhd_parts_rates_and_scaling() {
    let (_d, p) = write("a.rhd", &rhd(2));
    let mut ds = IntanReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    let names: Vec<_> = info
        .traces
        .iter()
        .map(|t| t.name.clone().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "amplifier",
            "auxiliary",
            "supply",
            "temperature",
            "board_adc",
            "digital_in"
        ]
    );
    let rates: Vec<f64> = info.traces.iter().map(|t| t.sample_rate_hz).collect();
    assert_eq!(
        rates,
        [
            20000.0,
            5000.0,
            20000.0 / 60.0,
            20000.0 / 60.0,
            20000.0,
            20000.0
        ]
    );
    assert_eq!(info.traces[0].sample_count, 120);
    assert_eq!(info.traces[0].start_s, Some(100.0 / 20000.0));
    let amp = ds.read_trace(0, 0, 58, 4).unwrap();
    assert_eq!(amp.channels[0].len(), 4);
    assert!((amp.channels[0][2] - (60.0 * 0.195 + (-6389.76 + 32768.0 * 0.195))).abs() < 1e-9);
    let aux = ds.read_trace(1, 0, 14, 2).unwrap();
    assert!((aux.channels[0][1] - 1015.0 * 0.000_037_4).abs() < 1e-12);
    let temp = ds.read_trace(3, 0, 0, 10).unwrap();
    assert_eq!(temp.channels[0].len(), 2);
    assert!((temp.channels[0][0] - 37.12).abs() < 1e-9);
    let dig = ds.read_trace(5, 0, 0, 4).unwrap();
    assert_eq!(dig.channels.len(), 2);
    assert_eq!(dig.channels[0], [1.0, 1.0, 1.0, 1.0]); // bit 2
    assert_eq!(dig.channels[1], [1.0, 0.0, 1.0, 0.0]); // bit 5; bit 9 is not an enabled line
    let r = ds.check().unwrap();
    assert!(r.ok, "{:?}", r.findings);
}

#[test]
fn rhd_truncated_block() {
    let mut b = rhd(3);
    b.truncate(b.len() - 10);
    let (_d, p) = write("b.rhd", &b);
    let mut ds = IntanReader.open(&p).unwrap();
    assert_eq!(ds.info().unwrap().traces[0].sample_count, 120);
    let r = ds.check().unwrap();
    assert!(!r.ok);
    assert!(r.findings.iter().any(|f| f.code == "truncated"));
}

#[test]
fn rhd_time_index_gap() {
    let mut b = rhd(2);
    // overwrite the first time index of block 2
    let (_d0, p0) = write("c0.rhd", &b);
    let hl = IntanReader.open(&p0).unwrap().vendor_metadata().unwrap()["header_len"]
        .as_u64()
        .unwrap() as usize;
    let block = (b.len() - hl) / 2;
    b[hl + block..hl + block + 4].copy_from_slice(&9999i32.to_le_bytes());
    let (_d, p) = write("c.rhd", &b);
    let r = IntanReader.open(&p).unwrap().check().unwrap();
    assert!(r.findings.iter().any(|f| f.code == "time_index_gap"));
}

#[test]
fn rhs_stimulation() {
    let mut b = Vec::new();
    b.extend_from_slice(&RHS_MAGIC.to_le_bytes());
    i16s(&mut b, &[3, 3]);
    f32s(&mut b, &[30000.0]);
    i16s(&mut b, &[0]);
    f32s(
        &mut b,
        &[1.0, 0.1, 1000.0, 7500.0, 1.0, 0.1, 1000.0, 7500.0],
    );
    i16s(&mut b, &[0]);
    f32s(&mut b, &[1000.0, 1000.0]);
    i16s(&mut b, &[0, 0]);
    f32s(&mut b, &[1e-6, 1e-6, 0.0]);
    qs(&mut b, "");
    qs(&mut b, "");
    qs(&mut b, "");
    i16s(&mut b, &[0, 14]); // no DC data, board mode 14
    qs(&mut b, "n/a");
    i16s(&mut b, &[1]);
    qs(&mut b, "Port A");
    qs(&mut b, "A");
    i16s(&mut b, &[1, 1, 1]);
    channel(&mut b, "A-000", 0, 0, true);
    let n = 128usize;
    for _ in 0..1 {
        for i in 0..n {
            b.extend_from_slice(&(i as i32).to_le_bytes());
        }
        for _ in 0..n {
            b.extend_from_slice(&32768u16.to_le_bytes());
        }
        for i in 0..n {
            let w: u16 = match i {
                0 => 0x0105,
                1 => 0x0005 | 0x8000,
                2 => 0x0100,
                _ => 0,
            };
            b.extend_from_slice(&w.to_le_bytes());
        }
    }
    let (_d, p) = write("s.rhs", &b);
    let mut ds = IntanReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces[1].name.as_deref(), Some("stimulation"));
    let st = ds.read_trace(1, 0, 0, 3).unwrap();
    let step = f64::from(1e-6f32);
    assert_eq!(st.channels[0], [-5.0 * step, 5.0 * step, 0.0]);
    assert!(st.channels[0][2].is_sign_positive());
    assert!(ds.check().unwrap().ok);
}

#[test]
fn bad_magic_is_corrupt() {
    let (_d, p) = write("x.rhd", &[1, 2, 3, 4, 5, 6, 7, 8]);
    match IntanReader.open(&p) {
        Err(e @ Error::Corrupt { .. }) => assert_eq!(e.exit_code(), 4),
        Err(e) => panic!("unexpected {e}"),
        Ok(_) => panic!("accepted garbage"),
    }
    let mut b = rhd(1);
    b.truncate(40);
    let (_d2, p2) = write("y.rhd", &b);
    assert!(matches!(IntanReader.open(&p2), Err(Error::Corrupt { .. })));
}

/// A split-layout directory: `info.rhd` (the RHD 1.5 header above, no data blocks), `time.dat`
/// and either one file per signal type or one file per channel, 4 samples each.
fn split_dir(per_channel: bool, truncate_amp: bool) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("info.rhd"), rhd(0)).unwrap();
    std::fs::write(d.path().join("settings.xml"), b"<xml/>").unwrap();
    let mut time = Vec::new();
    for t in 0..4i32 {
        time.extend_from_slice(&(t + 7).to_le_bytes());
    }
    std::fs::write(d.path().join("time.dat"), time).unwrap();
    let words = |v: &[u16]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let mut amp = words(&[(-2i16).cast_unsigned(), 0, 1, 2]);
    if truncate_amp {
        amp.pop();
    }
    let files: Vec<(&str, Vec<u8>)> = if per_channel {
        vec![
            ("amp-A-000.dat", amp),
            ("aux-A-AUX1.dat", words(&[10, 11, 12, 13])),
            ("vdd-A-VDD1.dat", words(&[45000; 4])),
            ("board-ADC-00.dat", words(&[1, 2, 3, 4])),
            ("board-DIN-02.dat", words(&[1, 0, 1, 0])),
            ("board-DIN-05.dat", words(&[0, 0, 1, 1])),
        ]
    } else {
        vec![
            ("amplifier.dat", amp),
            ("auxiliary.dat", words(&[10, 11, 12, 13])),
            ("supply.dat", words(&[45000; 4])),
            ("analogin.dat", words(&[1, 2, 3, 4])),
            ("digitalin.dat", words(&[0b100, 0, 0b10_0100, 0b10_0000])),
        ]
    };
    for (n, b) in files {
        std::fs::write(d.path().join(n), b).unwrap();
    }
    d
}

#[test]
#[allow(clippy::float_cmp)]
fn split_layouts_read_the_same_values() {
    for per_channel in [false, true] {
        let d = split_dir(per_channel, false);
        let det = IntanReader.sniff(&[], d.path()).unwrap();
        assert_eq!(det.format_id, "intan");
        for path in [d.path().to_path_buf(), d.path().join("info.rhd")] {
            let mut ds = IntanReader.open(&path).unwrap();
            let info = ds.info().unwrap();
            let names: Vec<_> = info
                .traces
                .iter()
                .map(|t| t.name.clone().unwrap())
                .collect();
            assert_eq!(
                names,
                [
                    "amplifier",
                    "auxiliary",
                    "supply",
                    "board_adc",
                    "digital_in"
                ]
            );
            assert!(
                info.traces
                    .iter()
                    .all(|t| t.sample_count == 4 && t.sample_rate_hz == 20000.0)
            );
            assert_eq!(info.traces[0].start_s, Some(7.0 / 20000.0));
            let amp = ds.read_trace(0, 0, 0, 10).unwrap();
            assert_eq!(amp.channels[0], [-2.0 * 0.195, 0.0, 0.195, 2.0 * 0.195]);
            let din = ds.read_trace(4, 0, 0, 10).unwrap();
            assert_eq!(
                din.channels,
                [vec![1.0, 0.0, 1.0, 0.0], vec![0.0, 0.0, 1.0, 1.0]]
            );
            let aux = ds.read_trace(1, 0, 2, 10).unwrap();
            assert_eq!(aux.channels[0], [12.0 * 0.000_037_4, 13.0 * 0.000_037_4]);
            assert!(ds.check().unwrap().ok, "per_channel={per_channel}");
        }
    }
}

#[test]
fn split_truncated_file_fails_check() {
    for per_channel in [false, true] {
        let d = split_dir(per_channel, true);
        let mut ds = IntanReader.open(d.path()).unwrap();
        let r = ds.check().unwrap();
        assert!(!r.ok);
        assert!(r.findings.iter().any(|f| f.code == "truncated"));
    }
}

#[test]
fn a_directory_with_foreign_files_is_not_claimed() {
    let d = split_dir(false, false);
    std::fs::write(d.path().join("photo.jpg"), b"x").unwrap();
    assert!(IntanReader.sniff(&[], d.path()).is_none());
}

#[test]
fn split_sessions_read_from_memory_after_local_files_are_removed() {
    use openreadout_core::source::{Fs, Input, MemFs, MemSource};
    use std::sync::Arc;
    for per_channel in [false, true] {
        let dir = split_dir(per_channel, false);
        let path = dir.path().to_path_buf();
        let mut fs = MemFs::new();
        for entry in std::fs::read_dir(&path).unwrap() {
            let p = entry.unwrap().path();
            fs.insert(
                &p,
                Arc::new(MemSource::new(
                    p.display().to_string(),
                    std::fs::read(&p).unwrap(),
                )),
            );
        }
        let fs = Fs::new(Arc::new(fs));
        drop(dir);
        for p in [path.clone(), path.join("info.rhd")] {
            let input = Input::new(p, fs.clone());
            assert!(
                IntanReader
                    .sniff_input(&[], &Input::new(&path, fs.clone()))
                    .is_some()
            );
            let mut ds = IntanReader.open_input(&input).unwrap();
            assert_eq!(
                ds.read_trace(0, 0, 0, 10).unwrap().channels[0],
                [-2.0 * 0.195, 0.0, 0.195, 2.0 * 0.195]
            );
            assert!(ds.check().unwrap().ok);
        }
    }
}
