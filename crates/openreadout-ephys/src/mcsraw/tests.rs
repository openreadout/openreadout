//! Synthetic-file tests of the MC_DataTool reader.

use super::*;
use openreadout_core::FormatReader;

fn file(streams: &str, frames: u16) -> Vec<u8> {
    let mut v = format!(
        "MC_DataTool binary conversion\r\nVersion 2.6.10\r\nMC_REC file = \"C:\\x.mcd\"\r\nSample rate = 10000\r\nADC zero = 32768\r\nEl = 0.5\u{b5}V/AD\r\nStreams = {streams}\r\nEOH\r\n"
    )
    .into_bytes();
    // the header is Windows-1252: µ is one byte
    if let Some(i) = v.windows(2).position(|w| w == [0xC2, 0xB5]) {
        v.remove(i);
    }
    let n = streams.split(';').count() as u16;
    for f in 0..frames {
        for c in 0..n {
            v.extend_from_slice(&(32768 + f * 10 + c).to_le_bytes());
        }
    }
    v
}

#[test]
fn reads_a_synthetic_export() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("x.raw");
    std::fs::write(&p, file("El_12;El_13", 5)).unwrap();
    let head = std::fs::read(&p).unwrap();
    assert!(McsRawReader.sniff(&head, &p).is_some());
    let mut ds = McsRawReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    let t = &info.traces[0];
    assert_eq!(t.sample_count, 5);
    assert_eq!(t.sample_rate_hz, 10000.0);
    assert_eq!(t.channels[1].name, "El_13");
    assert_eq!(t.channels[0].unit.as_deref(), Some("µV"));
    assert_eq!(info.format_version.as_deref(), Some("2.6.10"));
    let s = ds.read_trace(0, 0, 1, 10).unwrap();
    assert_eq!(s.channels[0], vec![5.0, 10.0, 15.0, 20.0]);
    assert_eq!(s.channels[1][0], 5.5);
    assert!(ds.check().unwrap().findings.is_empty());
}

#[test]
fn refuses_what_it_cannot_scale() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("x.raw");
    std::fs::write(&p, file("El_12;An_1", 2)).unwrap();
    let Err(e) = McsRawReader.open(&p) else {
        panic!("opened an analog stream");
    };
    assert_eq!(e.exit_code(), 6);
    std::fs::write(&p, b"MC_DataTool binary conversion\r\nno end").unwrap();
    let Err(e) = McsRawReader.open(&p) else {
        panic!("opened a file without EOH");
    };
    assert_eq!(e.exit_code(), 4);
}

#[test]
fn damaged_files_give_clean_errors() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("x.raw");
    let full = file("El_12;El_13;El_14", 7);
    for cut in 0..full.len() {
        std::fs::write(&p, &full[..cut]).unwrap();
        if let Ok(mut ds) = McsRawReader.open(&p) {
            let info = ds.info().unwrap();
            let _ = ds.read_trace(0, 0, 0, info.traces[0].sample_count + 3);
            let _ = ds.check();
        }
    }
}
