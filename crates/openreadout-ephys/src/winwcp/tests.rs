//! Synthetic-file tests of the WinWCP reader.

use openreadout_core::reader::{Dataset, FormatReader};

use super::{WinWcpDataset, WinWcpReader};

/// Two channels (YO swapped), two records of one data sector each, VMax 10 V in record 0 and
/// 5 V in record 1.
fn mini_wcp() -> Vec<u8> {
    let head = "VER=9\r\nVERPROG=V5.3.7\r\nRTIME=21/05/2019 16:17:53\r\nNBH=1024\r\nADCMAX=32767\r\nNC=2\r\nNBA=1\r\nNBD=1\r\nAD=10\r\nNR=2\r\nDT=1,0E-004\r\nYO0=1\r\nYU0=mV\r\nYN0=Vm\r\nYG0=0,01\r\nYZ0=0\r\nYO1=0\r\nYU1=pA\r\nYN1=Im\r\nYG1=0,001\r\nYZ1=0\r\n";
    let mut b = head.as_bytes().to_vec();
    b.resize(1024, 0);
    for (k, vmax) in [(0u8, 10.0f32), (1, 5.0)] {
        let mut a = vec![0u8; 512];
        a[..8].copy_from_slice(b"ACCEPTED");
        a[8..12].copy_from_slice(b"TEST");
        a[12..16].copy_from_slice(&1.0f32.to_le_bytes());
        a[16..20].copy_from_slice(&(f32::from(k) * 5.0).to_le_bytes());
        a[20..24].copy_from_slice(&1e-4f32.to_le_bytes());
        a[24..28].copy_from_slice(&vmax.to_le_bytes());
        a[28..32].copy_from_slice(&vmax.to_le_bytes());
        b.extend(a);
        // 128 frames of (Im, Vm): Im = −i, Vm = i
        let mut d = Vec::new();
        for i in 0..128i16 {
            d.extend((-i).to_le_bytes());
            d.extend(i.to_le_bytes());
        }
        b.extend(d);
    }
    b
}

#[test]
fn reads_records_as_sweeps() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("m.wcp");
    let bytes = mini_wcp();
    assert!(WinWcpReader.sniff(&bytes[..64], &p).is_some());
    std::fs::write(&p, &bytes).unwrap();
    let mut ds = WinWcpDataset::open(&p).unwrap();
    let info = ds.info().unwrap();
    let t = &info.traces[0];
    assert_eq!((t.sample_count, t.sweep_count), (128, 2));
    assert_eq!(t.channels[0].name, "Vm");
    assert_eq!(t.channels[1].unit.as_deref(), Some("pA"));
    assert_eq!(t.extra["recorded_at"], "2019-05-21T16:17:53");
    assert_eq!(t.extra["sweep_starts_s"], serde_json::json!([0.0, 5.0]));
    let s0 = ds.read_trace(0, 0, 2, 3).unwrap();
    let g0 = 10.0 / 32767.0 / 0.01;
    assert!((s0.channels[0][0] - 2.0 * g0).abs() < 1e-9);
    assert!((s0.channels[1][0] + 2.0 * 10.0 / 32767.0 / 0.001).abs() < 1e-9);
    let s1 = ds.read_trace(0, 1, 2, 1).unwrap();
    assert!((s1.channels[0][0] - 2.0 * 5.0 / 32767.0 / 0.01).abs() < 1e-9);
    assert!(info.notes.iter().any(|n| n.contains("VMax")));
    assert!(ds.check().unwrap().ok);
}

#[test]
fn damaged_files_fail_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let full = mini_wcp();
    for cut in [0, 10, 500, 1024, 1100, 1600, 2000, full.len() - 1] {
        let p = dir.path().join(format!("c{cut}.wcp"));
        std::fs::write(&p, &full[..cut]).unwrap();
        if let Ok(mut ds) = WinWcpDataset::open(&p) {
            let _ = ds.info();
            let _ = ds.check();
            let _ = ds.read_trace(0, 0, 0, 1000);
            let _ = ds.read_trace(0, 1, 0, 1000);
        }
    }
    for (from, to) in [
        ("NC=2", "NC=0"),
        ("NBD=1", "NBD=99999999"),
        ("ADCMAX=32767", "ADCMAX=0"),
        ("YG0=0,01", "YG0=0"),
    ] {
        let s = String::from_utf8_lossy(&full[..1024]).replace(from, to);
        let mut b = s.into_bytes();
        b.resize(1024, 0);
        b.extend_from_slice(&full[1024..]);
        let p = dir.path().join("h.wcp");
        std::fs::write(&p, &b).unwrap();
        if let Ok(mut ds) = WinWcpDataset::open(&p) {
            let _ = ds.info();
            let _ = ds.check();
            let _ = ds.read_trace(0, 0, 0, 1000);
        }
    }
}
