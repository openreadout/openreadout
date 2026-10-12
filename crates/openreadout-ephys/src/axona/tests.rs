//! Synthetic-session tests of the Axona reader.

use super::*;
use openreadout_core::FormatReader;
use openreadout_core::model::Severity;

fn set_text() -> String {
    let mut s = String::from(
        "trial_date Sunday, 4 Oct 2020\r\ntrial_time 11:07:07\r\nduration 1\r\nsw_version 1.2.2.16\r\nADC_fullscale_mv 1500\r\n",
    );
    for i in 0..64 {
        s += &format!("gain_ch_{i} {}\r\n", if i < 4 { 10000 } else { 20000 });
    }
    for t in 1..=16 {
        s += &format!("collectMask_{t} {}\r\n", u8::from(t == 1 || t == 3));
    }
    s += "rawRate 48000\r\n";
    s
}

/// `packets` packets; channel i, sample k holds i * 100 + k.
fn bin(packets: u32) -> Vec<u8> {
    let mut v = Vec::new();
    for p in 0..packets {
        v.extend_from_slice(if p % 2 == 0 { b"ADU1" } else { b"ADU2" });
        v.extend_from_slice(&(1000 + p).to_le_bytes());
        v.extend_from_slice(&[0; 24]);
        for sub in 0..3 {
            let mut frame = [0i16; 64];
            for i in 0..64 {
                frame[bin_channel_slot(i)] = (i * 100) as i16 + (p * 3 + sub) as i16;
            }
            for x in frame {
                v.extend_from_slice(&x.to_le_bytes());
            }
        }
        v.extend_from_slice(&[0; 16]);
    }
    v
}

fn tetrode(spikes: &[(i32, i8)]) -> Vec<u8> {
    let mut v = format!(
        "trial_date Sunday, 4 Oct 2020\r\nnum_chans 4\r\ntimebase 96000 hz\r\nbytes_per_timestamp 4\r\nsamples_per_spike 50\r\nbytes_per_sample 1\r\nnum_spikes {}\r\ndata_start",
        spikes.len()
    )
    .into_bytes();
    for (t, base) in spikes {
        for c in 0..4i8 {
            v.extend_from_slice(&t.to_be_bytes());
            v.extend((0..50).map(|i| (base + c * 10 + i as i8) as u8));
        }
    }
    v.extend_from_slice(b"\r\ndata_end\r\n");
    v
}

fn session(dir: &Path, packets: u32) -> PathBuf {
    std::fs::write(dir.join("s.set"), set_text()).unwrap();
    std::fs::write(dir.join("s.bin"), bin(packets)).unwrap();
    std::fs::write(dir.join("s.1"), tetrode(&[(960, 1), (1920, -5)])).unwrap();
    std::fs::write(dir.join("s.2"), tetrode(&[(480, 2)])).unwrap();
    std::fs::write(dir.join("s.eeg"), b"trial_date x\r\ndata_start").unwrap();
    dir.join("s.set")
}

#[test]
fn slots_follow_the_interleaved_blocks() {
    let slots: Vec<usize> = (0..64).map(bin_channel_slot).collect();
    assert_eq!(&slots[..9], &[32, 33, 34, 35, 36, 37, 38, 39, 0]);
    assert_eq!(slots[16], 40);
    assert_eq!(slots[63], 31);
    let mut sorted = slots.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, (0..64).collect::<Vec<_>>());
}

#[test]
fn reads_a_synthetic_session() {
    let d = tempfile::tempdir().unwrap();
    let p = session(d.path(), 4);
    let head = std::fs::read(&p).unwrap();
    assert_eq!(
        AxonaReader.sniff(&head, &p).unwrap().confidence,
        DetectConfidence::Definite
    );
    let mut ds = AxonaReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    let t = &info.traces[0];
    assert_eq!(t.sample_count, 12);
    assert_eq!(t.sample_rate_hz, 48000.0);
    let names: Vec<&str> = t.channels.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["1a", "1b", "1c", "1d", "3a", "3b", "3c", "3d"]);
    assert!((t.channels[0].scale - 1500.0 * 1000.0 / (10000.0 * 32768.0)).abs() < 1e-15);
    assert_eq!(t.extra["recorded_at"], "2020-10-04T11:07:07");
    let s = ds.read_trace(0, 0, 2, 5).unwrap();
    let raw: Vec<f64> = s.channels[4]
        .iter()
        .map(|v| (v / t.channels[4].scale).round())
        .collect();
    assert_eq!(raw, vec![802.0, 803.0, 804.0, 805.0, 806.0]);
    assert!(info.notes.iter().any(|n| n.contains("s.eeg")));
    let tab = ds.read_table(0, 1, 10).unwrap();
    assert_eq!(tab.columns[0], vec![0.02, 0.005]);
    assert_eq!(tab.columns[2], vec![1.0, 2.0]);
    assert_eq!(tab.columns[3][0], -5.0);
    assert_eq!(tab.columns[3 + 50 + 1][1], 2.0 + 10.0 + 1.0);
    let r = ds.check().unwrap();
    assert!(
        !r.findings.iter().any(|f| f.severity != Severity::Info),
        "{:?}",
        r.findings
    );
}

#[test]
fn opens_from_the_bin_and_refuses_without_set() {
    let d = tempfile::tempdir().unwrap();
    session(d.path(), 2);
    let b = d.path().join("s.bin");
    let head = std::fs::read(&b).unwrap();
    assert!(AxonaReader.sniff(&head, &b).is_some());
    assert!(AxonaReader.open(&b).is_ok());
    std::fs::remove_file(d.path().join("s.set")).unwrap();
    let Err(e) = AxonaReader.open(&b) else {
        panic!("opened without a .set");
    };
    assert_eq!(e.exit_code(), 6);
}

#[test]
fn damaged_sessions_give_clean_errors() {
    let d = tempfile::tempdir().unwrap();
    let p = session(d.path(), 3);
    let full_bin = bin(3);
    let full_t = tetrode(&[(960, 1), (1920, -5)]);
    for cut in (0..full_bin.len()).step_by(37) {
        std::fs::write(d.path().join("s.bin"), &full_bin[..cut]).unwrap();
        std::fs::write(d.path().join("s.1"), &full_t[..cut.min(full_t.len())]).unwrap();
        if let Ok(mut ds) = AxonaReader.open(&p) {
            let info = ds.info().unwrap();
            for t in &info.traces {
                let _ = ds.read_trace(t.index, 0, 0, 100);
            }
            for t in &info.tables {
                let _ = ds.read_table(t.index, 0, 100);
            }
            let _ = ds.check();
        }
    }
    let text = set_text();
    for cut in (0..text.len()).step_by(11) {
        std::fs::write(d.path().join("s.set"), &text[..cut]).unwrap();
        if let Ok(mut ds) = AxonaReader.open(&p) {
            let info = ds.info().unwrap();
            for t in &info.traces {
                let _ = ds.read_trace(t.index, 0, 0, 100);
            }
            let _ = ds.check();
        }
    }
}
