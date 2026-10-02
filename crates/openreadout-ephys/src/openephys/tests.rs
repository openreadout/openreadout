//! Synthetic-directory tests of the Open Ephys reader.

use openreadout_core::reader::Dataset;

use super::dataset::OpenEphysDataset;
use super::legacy::RECORD_MARKER;

fn legacy_file(name: &str, runs: &[(i64, u16, i16)]) -> Vec<u8> {
    let mut h = format!(
        "header.format = 'Open Ephys Data Format';\nheader.version = 0.4;\nheader.channel = '{name}';\nheader.sampleRate = 30000;\nheader.bitVolts = 0.5;\n"
    )
    .into_bytes();
    h.resize(1024, b' ');
    for &(ts, rec, v) in runs {
        h.extend(ts.to_le_bytes());
        h.extend(1024u16.to_le_bytes());
        h.extend(rec.to_le_bytes());
        for _ in 0..1024 {
            h.extend(v.to_be_bytes());
        }
        h.extend(RECORD_MARKER);
    }
    h
}

#[test]
fn legacy_runs_split_at_gaps_and_irregular_files_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    // two contiguous records, then a gap: two sweeps
    let recs = [(1000, 0, 2), (2024, 0, 4), (9000, 1, 6)];
    std::fs::write(d.join("100_CH1.continuous"), legacy_file("CH1", &recs)).unwrap();
    std::fs::write(d.join("100_CH2.continuous"), legacy_file("CH2", &recs)).unwrap();
    // irregular clock: every record a new run
    std::fs::write(
        d.join("100_CH3.continuous"),
        legacy_file("CH3", &[(0, 0, 1), (900, 0, 1), (2000, 0, 1)]),
    )
    .unwrap();
    let mut ds = OpenEphysDataset::open(d).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces.len(), 1);
    let t = &info.traces[0];
    assert_eq!(
        (t.sweep_count, t.sample_count, t.channels.len()),
        (2, 2048, 2)
    );
    assert_eq!(t.channels[0].unit.as_deref(), Some("uV"));
    let s0 = ds.read_trace(0, 0, 1020, 8).unwrap();
    assert_eq!(s0.channels[0], vec![1.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0]);
    let s1 = ds.read_trace(0, 1, 0, 2).unwrap();
    assert_eq!(s1.channels[1], vec![3.0, 3.0]);
    let r = ds.check().unwrap();
    assert!(r.findings.iter().any(|f| f.code == "unreadable_channel"));
    assert!(ds.read_trace(0, 2, 0, 1).is_err());
}

#[test]
fn binary_recording() {
    let dir = tempfile::tempdir().unwrap();
    let rec = dir.path().join("Record Node 101/experiment1/recording1");
    let cont = rec.join("continuous/Acq-100.Probe");
    std::fs::create_dir_all(&cont).unwrap();
    std::fs::write(
        rec.join("structure.oebin"),
        r#"{"GUI version": "0.6.7", "continuous": [{"folder_name": "Acq-100.Probe/", "sample_rate": 1000.0,
            "channels": [{"channel_name": "CH1", "bit_volts": 0.25, "units": ""},
                         {"channel_name": "ADC1", "bit_volts": 2.0, "units": ""}]}],
            "events": [], "spikes": []}"#,
    )
    .unwrap();
    let samples: Vec<u8> = [4i16, -8, 12, 16]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    std::fs::write(cont.join("continuous.dat"), samples).unwrap();
    let mut ds = OpenEphysDataset::open(dir.path()).unwrap();
    let info = ds.info().unwrap();
    let t = &info.traces[0];
    assert_eq!((t.sweep_count, t.sample_count), (1, 2));
    assert_eq!(t.channels[1].unit.as_deref(), Some("V"));
    let s = ds.read_trace(0, 0, 0, 10).unwrap();
    assert_eq!(s.channels, vec![vec![1.0, 3.0], vec![-16.0, 32.0]]);
    // a truncated continuous.dat is reported, not panicked on
    std::fs::write(cont.join("continuous.dat"), [1u8, 2, 3]).unwrap();
    let mut ds = OpenEphysDataset::open(dir.path()).unwrap();
    let _ = ds.info();
    assert!(
        ds.check()
            .unwrap()
            .findings
            .iter()
            .any(|f| f.code == "partial_sample")
    );
}

#[test]
fn only_recording_folders_are_claimed() {
    use openreadout_core::reader::FormatReader;
    let dir = tempfile::tempdir().unwrap();
    let rec = dir
        .path()
        .join("share/Record Node 101/experiment1/recording1");
    std::fs::create_dir_all(&rec).unwrap();
    std::fs::write(rec.join("structure.oebin"), "{}").unwrap();
    std::fs::write(dir.path().join("share/notes.pdf"), b"%PDF").unwrap();
    let r = super::OpenEphysReader;
    // the recording's own folders are claimed
    assert!(
        r.sniff(&[], &dir.path().join("share/Record Node 101"))
            .is_some()
    );
    assert!(r.sniff(&[], &rec).is_some());
    // a share that holds a recording among other files is not
    assert!(r.sniff(&[], &dir.path().join("share")).is_none());
    assert!(r.sniff(&[], dir.path()).is_none());
}
