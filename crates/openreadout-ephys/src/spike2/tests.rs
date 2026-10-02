//! Synthetic-file tests of the Spike2 reader.

use openreadout_core::reader::Dataset;

use super::dataset::Spike2Dataset;
use super::file::SMR_COPYRIGHT;

/// A version-6 `.smr`: channel 1 Adc (interval 10 ticks of 10 µs, scale 6553.6 so gain 1) with
/// two contiguous blocks and a third after a pause; channel 2 a marker channel.
fn mini_smr() -> Vec<u8> {
    let mut b = vec![0u8; 512 + 2 * 140];
    b[0..2].copy_from_slice(&6i16.to_le_bytes());
    b[2..12].copy_from_slice(SMR_COPYRIGHT);
    b[20..22].copy_from_slice(&10i16.to_le_bytes()); // us_per_time
    b[22..24].copy_from_slice(&1i16.to_le_bytes());
    b[30..32].copy_from_slice(&2i16.to_le_bytes());
    b[44..52].copy_from_slice(&1e-6f64.to_le_bytes());
    let adc: [(i32, Vec<i16>); 3] = [(0, vec![1, 2, 3]), (30, vec![4, 5]), (100, vec![6])];
    let mut at = b.len();
    let mut offs = Vec::new();
    for (_, s) in &adc {
        offs.push(at);
        at += 20 + 2 * s.len();
    }
    let mut blocks: Vec<Vec<u8>> = Vec::new();
    for (k, (start, s)) in adc.iter().enumerate() {
        let mut h = Vec::new();
        h.extend(if k == 0 { -1 } else { offs[k - 1] as i32 }.to_le_bytes());
        h.extend(offs.get(k + 1).map_or(-1, |&o| o as i32).to_le_bytes());
        h.extend(start.to_le_bytes());
        h.extend((start + 10 * (s.len() as i32 - 1)).to_le_bytes());
        h.extend(1i16.to_le_bytes());
        h.extend((s.len() as u16).to_le_bytes());
        for v in s {
            h.extend(v.to_le_bytes());
        }
        blocks.push(h);
    }
    let marker_at = at;
    let mut m = Vec::new();
    m.extend((-1i32).to_le_bytes());
    m.extend((-1i32).to_le_bytes());
    m.extend(5i32.to_le_bytes());
    m.extend(5i32.to_le_bytes());
    m.extend(2i16.to_le_bytes());
    m.extend(1u16.to_le_bytes());
    m.extend(5i32.to_le_bytes());
    m.extend([7u8, 0, 0, 0]);
    blocks.push(m);
    // channel 1: Adc "Vm" in mV
    let o = 512;
    b[o + 6..o + 10].copy_from_slice(&(offs[0] as i32).to_le_bytes());
    b[o + 10..o + 14].copy_from_slice(&(offs[2] as i32).to_le_bytes());
    b[o + 14..o + 16].copy_from_slice(&3u16.to_le_bytes());
    b[o + 102..o + 106].copy_from_slice(&10i32.to_le_bytes());
    b[o + 108] = 2;
    b[o + 109..o + 111].copy_from_slice(b"Vm");
    b[o + 122] = 1;
    b[o + 124..o + 128].copy_from_slice(&6553.6f32.to_le_bytes());
    b[o + 132] = 2;
    b[o + 133..o + 135].copy_from_slice(b"mV");
    // channel 2: marker
    let o = 512 + 140;
    b[o + 6..o + 10].copy_from_slice(&(marker_at as i32).to_le_bytes());
    b[o + 10..o + 14].copy_from_slice(&(marker_at as i32).to_le_bytes());
    b[o + 14..o + 16].copy_from_slice(&1u16.to_le_bytes());
    b[o + 122] = 5;
    for h in blocks {
        b.extend(h);
    }
    b
}

#[test]
fn reads_the_minimal_file() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("mini.smr");
    std::fs::write(&p, mini_smr()).unwrap();
    let mut ds = Spike2Dataset::open(&p).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces.len(), 1);
    let t = &info.traces[0];
    assert_eq!((t.sweep_count, t.sample_count), (2, 5));
    assert!((t.sample_rate_hz - 10_000.0).abs() < 1e-6);
    assert_eq!(t.channels[0].unit.as_deref(), Some("mV"));
    let s0 = ds.read_trace(0, 0, 1, 10).unwrap();
    assert_eq!(s0.channels[0].len(), 4);
    assert!((s0.channels[0][0] - 2.0).abs() < 1e-5);
    let s1 = ds.read_trace(0, 1, 0, 10).unwrap();
    assert!((s1.channels[0][0] - 6.0).abs() < 1e-5);
    let ev = ds.read_table(0, 0, 10).unwrap();
    assert_eq!(ev.columns[1], vec![5.0]);
    assert_eq!(ev.columns[2], vec![2.0]);
    assert_eq!(ev.columns[3], vec![7.0]);
    let r = ds.check().unwrap();
    assert!(r.ok, "{:?}", r.findings);
    assert!(r.findings.iter().any(|f| f.code == "pauses"));
}

#[test]
fn short_smrx_is_corrupt_and_smr_survives_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("x.smrx");
    std::fs::write(&p, b"S64pl\0\x01\x01\xff\xff\0\0\0\0\0\0").unwrap();
    let e = Spike2Dataset::open(&p).unwrap_err();
    assert_eq!(e.exit_code(), 4);
    let full = mini_smr();
    for cut in [0, 10, 100, 511, 600, 800, full.len() - 3] {
        let p = dir.path().join(format!("c{cut}.smr"));
        std::fs::write(&p, &full[..cut]).unwrap();
        if let Ok(mut ds) = Spike2Dataset::open(&p) {
            let _ = ds.info();
            let _ = ds.check();
            let _ = ds.read_trace(0, 0, 0, 100);
            let _ = ds.read_table(0, 0, 100);
        }
    }
}

/// A minimal `.smrx`: header block with two channel records and a string table; channel 1 Adc
/// (interval 5 ticks of 1 µs, scale 6553.6 so gain 1) with one index block and a data block of
/// two runs (the second after a pause) plus a block of an older generation; channel 2 rising
/// events. Layout as in `docs/formats/ced-spike2.md`.
fn mini_smrx() -> Vec<u8> {
    let mut b = vec![0u8; 0x10000];
    b[..6].copy_from_slice(b"S64pl\0");
    b[6] = 1;
    b[7] = 1;
    b[16..24].copy_from_slice(b"S2091349");
    b[24..32].copy_from_slice(&[0, 5, 32, 8, 9, 10, 0xe8, 0x07]);
    b[32..40].copy_from_slice(&1e-6f64.to_le_bytes());
    let (table, rec) = (2048u32, 272u32);
    let strings = table + 2 * rec;
    for (k, v) in [(44, table), (48, table), (52, strings), (56, 2), (60, rec)] {
        b[k..k + 4].copy_from_slice(&v.to_le_bytes());
    }
    b[64..68].copy_from_slice(&3u32.to_le_bytes()); // file comment: string 3
    // strings: 1 "wave", 2 "mV", 3 "note", 4 "spikes"
    let mut st = Vec::new();
    st.extend(0u32.to_le_bytes());
    st.extend(4u32.to_le_bytes());
    for name in ["wave", "mV", "note", "spikes"] {
        st.extend(1u32.to_le_bytes());
        let mut padded = name.as_bytes().to_vec();
        padded.push(0);
        while padded.len() % 4 != 0 {
            padded.push(0);
        }
        st.extend(padded);
    }
    b[strings as usize..strings as usize + st.len()].copy_from_slice(&st);
    let put64 =
        |b: &mut Vec<u8>, at: usize, v: u64| b[at..at + 8].copy_from_slice(&v.to_le_bytes());
    // channel 1 (Adc) record
    let c1 = table as usize;
    put64(&mut b, c1, 0x10000); // root index
    put64(&mut b, c1 + 0x10, 1); // live data blocks
    b[c1 + 0x20..c1 + 0x24].copy_from_slice(&2u32.to_le_bytes());
    b[c1 + 0x2c..c1 + 0x2e].copy_from_slice(&1u16.to_le_bytes()); // generation 1
    b[c1 + 0x2e] = 1; // Adc
    b[c1 + 0x30..c1 + 0x34].copy_from_slice(&2i32.to_le_bytes());
    b[c1 + 0x34..c1 + 0x38].copy_from_slice(&1u32.to_le_bytes());
    b[c1 + 0x38..c1 + 0x3c].copy_from_slice(&2u32.to_le_bytes());
    put64(&mut b, c1 + 0x40, 5); // interval
    b[c1 + 0x48..c1 + 0x50].copy_from_slice(&2e5f64.to_le_bytes());
    b[c1 + 0x50..c1 + 0x58].copy_from_slice(&6553.6f64.to_le_bytes());
    b[c1 + 0x58..c1 + 0x60].copy_from_slice(&0.5f64.to_le_bytes());
    // channel 2 (events) record
    let c2 = (table + rec) as usize;
    put64(&mut b, c2, 0x11000);
    put64(&mut b, c2 + 0x10, 1);
    b[c2 + 0x20..c2 + 0x24].copy_from_slice(&8u32.to_le_bytes());
    b[c2 + 0x2e] = 3;
    b[c2 + 0x34..c2 + 0x38].copy_from_slice(&4u32.to_le_bytes());
    b.resize(0x40000, 0);
    // index of channel 1: a stale block first, then the live one
    let i1 = 0x10000usize;
    put64(&mut b, i1, 0x100);
    b[i1 + 12..i1 + 16].copy_from_slice(&2u32.to_le_bytes());
    put64(&mut b, i1 + 16, 0);
    put64(&mut b, i1 + 24, 0x30000);
    put64(&mut b, i1 + 32, 0);
    put64(&mut b, i1 + 40, 0x20000);
    // index of channel 2
    let i2 = 0x11000usize;
    put64(&mut b, i2, 0x100);
    b[i2 + 8..i2 + 10].copy_from_slice(&1u16.to_le_bytes());
    b[i2 + 12..i2 + 16].copy_from_slice(&1u32.to_le_bytes());
    put64(&mut b, i2 + 16, 7);
    put64(&mut b, i2 + 24, 0x31000);
    // live Adc block: runs (0, [1,2,3]) and (100, [4,5])
    let live = 0x20000usize;
    put64(&mut b, live, 0x10001);
    b[live + 10..live + 12].copy_from_slice(&1u16.to_le_bytes());
    b[live + 12..live + 16].copy_from_slice(&2u32.to_le_bytes());
    let mut pos = live + 16;
    for (start, vals) in [(0u64, vec![1i16, 2, 3]), (100, vec![4, 5])] {
        put64(&mut b, pos, start);
        put64(&mut b, pos + 8, vals.len() as u64);
        pos += 16;
        for v in vals {
            b[pos..pos + 2].copy_from_slice(&v.to_le_bytes());
            pos += 2;
        }
    }
    // stale block of generation 0 (must be skipped)
    let stale = 0x30000usize;
    put64(&mut b, stale, 0x10000);
    b[stale + 12..stale + 16].copy_from_slice(&1u32.to_le_bytes());
    put64(&mut b, stale + 16, 0);
    put64(&mut b, stale + 24, 1);
    b[stale + 32..stale + 34].copy_from_slice(&999i16.to_le_bytes());
    // event block of channel 2 (in the last 64 KiB, as a smaller block)
    let events = 0x31000usize;
    put64(&mut b, events, 0x11000);
    b[events + 8..events + 10].copy_from_slice(&1u16.to_le_bytes());
    b[events + 12..events + 16].copy_from_slice(&2u32.to_le_bytes());
    put64(&mut b, events + 16, 7);
    put64(&mut b, events + 24, 250);
    b.truncate(0x32000);
    put64(&mut b, 1000, 0x32000);
    put64(&mut b, 1016, 250);
    b
}

#[test]
fn reads_a_minimal_smrx() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("m.smrx");
    std::fs::write(&p, mini_smrx()).unwrap();
    let mut ds = Spike2Dataset::open(&p).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("smrx 1.1"));
    assert_eq!(info.traces.len(), 1);
    let t = &info.traces[0];
    assert_eq!(t.name.as_deref(), Some("wave"));
    assert_eq!(t.channels[0].unit.as_deref(), Some("mV"));
    assert_eq!(t.sweep_count, 2);
    assert!((t.sample_rate_hz - 2e5).abs() < 1e-6);
    assert_eq!(t.extra["recorded_at"], "2024-10-09T08:32:05.000");
    assert_eq!(t.extra["file_comments"], serde_json::json!(["note"]));
    let s0 = ds.read_trace(0, 0, 0, 10).unwrap();
    assert_eq!(s0.channels[0], vec![1.5, 2.5, 3.5]);
    let s1 = ds.read_trace(0, 1, 0, 10).unwrap();
    assert_eq!(s1.channels[0], vec![4.5, 5.5]);
    let ev = ds.read_table(0, 0, 10).unwrap();
    assert_eq!(ev.columns[1], vec![7.0, 250.0]);
    assert_eq!(ev.columns[2], vec![2.0, 2.0]);
    let r = ds.check().unwrap();
    assert!(r.ok, "{:?}", r.findings);
}

#[test]
fn damaged_smrx_fails_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let full = mini_smrx();
    for cut in [
        0,
        7,
        100,
        1031,
        2100,
        2600,
        0x10000,
        0x10010,
        0x11008,
        0x20010,
        0x20030,
        0x31018,
        full.len() - 1,
    ] {
        let p = dir.path().join(format!("c{cut}.smrx"));
        std::fs::write(&p, &full[..cut.min(full.len())]).unwrap();
        if let Ok(mut ds) = Spike2Dataset::open(&p) {
            let _ = ds.info();
            let _ = ds.check();
            let _ = ds.read_trace(0, 0, 0, 100);
            let _ = ds.read_table(0, 0, 100);
        }
    }
    // corrupted fields: loops, huge counts, bad offsets
    for (at, v) in [
        (0x10000 + 24, 0x10000u64), // index points at itself
        (0x10000 + 12, u64::from(u32::MAX)),
        (0x20000 + 16 + 8, u64::MAX), // run length
        (996, 5000),                  // header block count
        (56, 100_000),                // channel slots
        (2048 + 0x40, 0),             // zero interval
        (0x31000 + 12, 1 << 30),      // event count
    ] {
        let mut b = full.clone();
        b[at..at + 8].copy_from_slice(&v.to_le_bytes());
        let p = dir.path().join(format!("f{at}.smrx"));
        std::fs::write(&p, &b).unwrap();
        if let Ok(mut ds) = Spike2Dataset::open(&p) {
            let _ = ds.info();
            let _ = ds.check();
            let _ = ds.read_trace(0, 0, 0, 100);
            let _ = ds.read_table(0, 0, 100);
        }
    }
}
