//! Synthetic-recording tests of the BrainVision reader.

use openreadout_core::reader::{Dataset, FormatReader};

use super::{BrainVisionDataset, BrainVisionReader, identification_version};

fn vhdr(extra_common: &str, binary: &str, orientation: &str) -> String {
    format!(
        "Brain Vision Data Exchange Header File Version 1.0\r\n; Data created by the Vision Recorder\r\n\r\n\
         [Common Infos]\r\nCodepage=UTF-8\r\nDataFile=rec.eeg\r\nMarkerFile=rec.vmrk\r\nDataFormat=BINARY\r\n\
         DataOrientation={orientation}\r\nNumberOfChannels=2\r\nSamplingInterval=2000\r\n{extra_common}\r\n\
         [Binary Infos]\r\nBinaryFormat={binary}\r\n\r\n[Channel Infos]\r\nCh1=Fp1,,0.5,µV\r\nCh2=A\\1B,REF,2\r\n\r\n\
         [Coordinates]\r\nCh1=1,-90,-72\r\n\r\n[Comment]\r\nA m p l i f i e r  S e t u p\r\nNumber of channels: 2\r\n"
    )
}

const VMRK: &str = "Brain Vision Data Exchange Marker File, Version 1.0\r\n\r\n[Common Infos]\r\nCodepage=UTF-8\r\nDataFile=rec.eeg\r\n\r\n\
    [Marker Infos]\r\nMk1=New Segment,,1,1,0,20200124040556123456\r\nMk2=Stimulus,S  1,3,1,0\r\nMk10=Response,R\\1x,4,2,1\r\n";

fn write(dir: &std::path::Path, header: &str, data: &[u8]) -> std::path::PathBuf {
    std::fs::write(dir.join("rec.vhdr"), header).unwrap();
    std::fs::write(dir.join("rec.vmrk"), VMRK).unwrap();
    std::fs::write(dir.join("rec.eeg"), data).unwrap();
    dir.join("rec.vhdr")
}

#[test]
fn multiplexed_int16_with_markers() {
    let dir = tempfile::tempdir().unwrap();
    // 4 frames of (ch1, ch2)
    let data: Vec<u8> = [1i16, -1, 2, -2, 3, -3, 4, -4]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let p = write(dir.path(), &vhdr("", "INT_16", "MULTIPLEXED"), &data);
    let head = std::fs::read(&p).unwrap();
    assert!(BrainVisionReader.sniff(&head, &p).is_some());
    let mut ds = BrainVisionDataset::open(&p).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("1"));
    let t = &info.traces[0];
    assert_eq!(
        (t.sample_rate_hz, t.sample_count, t.sweep_count),
        (500.0, 4, 1)
    );
    assert_eq!(t.channels[1].name, "A,B");
    assert_eq!(t.channels[1].unit.as_deref(), Some("µV"));
    assert_eq!(t.extra["recorded_at"], "2020-01-24T04:05:56.123456");
    assert_eq!(t.extra["application"], "BrainVision Recorder");
    let tr = ds.read_trace(0, 0, 1, 2).unwrap();
    assert_eq!(tr.channels[0], vec![1.0, 1.5]);
    assert_eq!(tr.channels[1], vec![-4.0, -6.0]);
    let tab = &info.tables[0];
    assert_eq!(tab.row_count, 3);
    let rows = ds.read_table(0, 0, 10).unwrap();
    assert_eq!(rows.columns[0], vec![0.0, 0.004, 0.006]);
    assert_eq!(rows.columns[2], vec![0.0, 2.0, 3.0]);
    assert_eq!(rows.columns[3], vec![1.0, 1.0, 2.0]);
    assert_eq!(rows.columns[4], vec![0.0, 0.0, 1.0]);
    assert_eq!(
        tab.columns[6].extra["categories"],
        serde_json::json!(["", "S  1", "R,x"])
    );
    assert!(ds.check().unwrap().ok);
    // the marker and data files open the recording too
    assert!(BrainVisionDataset::open(&dir.path().join("rec.vmrk")).is_ok());
    assert!(BrainVisionDataset::open(&dir.path().join("rec.eeg")).is_ok());
}

#[test]
fn vectorized_float_and_segments() {
    let dir = tempfile::tempdir().unwrap();
    // channel 1: 0..6, channel 2: 10..16, as float32; 2 segments of 3
    let mut data = Vec::new();
    for v in (0..6).chain(10..16) {
        data.extend((v as f32).to_le_bytes());
    }
    let common = "DataPoints=6\r\nSegmentationType=MARKERBASED\r\nSegmentDataPoints=3";
    let p = write(
        dir.path(),
        &vhdr(common, "IEEE_FLOAT_32", "VECTORIZED"),
        &data,
    );
    let mut ds = BrainVisionDataset::open(&p).unwrap();
    let t = &ds.info().unwrap().traces[0];
    assert_eq!((t.sweep_count, t.sample_count), (2, 3));
    assert_eq!(t.extra["sweep_starts_s"], serde_json::json!([0.0, 0.006]));
    let tr = ds.read_trace(0, 1, 0, 10).unwrap();
    assert_eq!(tr.channels[0], vec![1.5, 2.0, 2.5]);
    assert_eq!(tr.channels[1], vec![26.0, 28.0, 30.0]);
}

#[test]
fn ascii_multiplexed() {
    let dir = tempfile::tempdir().unwrap();
    let header = vhdr("", "INT_16", "MULTIPLEXED")
        .replace("DataFormat=BINARY", "DataFormat=ASCII")
        .replace(
            "[Binary Infos]\r\nBinaryFormat=INT_16",
            "[ASCII Infos]\r\nDecimalSymbol=,\r\nSkipLines=1\r\nSkipColumns=0",
        );
    let p = write(dir.path(), &header, b"Fp1 A\r\n1,5 2\r\n-3 4,25\r\n");
    let mut ds = BrainVisionDataset::open(&p).unwrap();
    assert_eq!(ds.info().unwrap().traces[0].sample_count, 2);
    let tr = ds.read_trace(0, 0, 0, 10).unwrap();
    assert_eq!(tr.channels[0], vec![0.75, -1.5]);
    assert_eq!(tr.channels[1], vec![4.0, 8.5]);
}

#[test]
fn identification_lines() {
    assert_eq!(
        identification_version(
            "Brain Vision Data Exchange Header File Version 1.0",
            "header"
        ),
        Some(1)
    );
    assert_eq!(
        identification_version(
            "BrainVision Data Exchange Header File Version 2.0",
            "header"
        ),
        Some(2)
    );
    assert_eq!(
        identification_version(
            "BrainVision Data Exchange Marker File, Version 2.0",
            "marker"
        ),
        Some(2)
    );
    assert_eq!(
        identification_version(
            "BrainVision Data Exchange Marker File, Version 2.0",
            "header"
        ),
        None
    );
    assert_eq!(identification_version("[Common Infos]", "header"), None);
}

#[test]
fn damaged_recordings_fail_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let data: Vec<u8> = [1i16, -1, 2].iter().flat_map(|v| v.to_le_bytes()).collect();
    let p = write(dir.path(), &vhdr("", "INT_16", "MULTIPLEXED"), &data);
    let mut ds = BrainVisionDataset::open(&p).unwrap();
    assert_eq!(ds.info().unwrap().traces[0].sample_count, 1);
    assert!(
        ds.check()
            .unwrap()
            .findings
            .iter()
            .any(|f| f.code == "partial_frame")
    );
    for (from, to) in [
        ("NumberOfChannels=2", "NumberOfChannels=x"),
        ("SamplingInterval=2000", "SamplingInterval=0"),
        ("BinaryFormat=INT_16", "BinaryFormat=INT_8"),
        ("Brain Vision Data Exchange", "Something else"),
    ] {
        std::fs::write(&p, vhdr("", "INT_16", "MULTIPLEXED").replace(from, to)).unwrap();
        assert!(BrainVisionDataset::open(&p).is_err(), "{to}");
    }
    std::fs::remove_file(dir.path().join("rec.eeg")).unwrap();
    std::fs::write(&p, vhdr("", "INT_16", "MULTIPLEXED")).unwrap();
    assert!(BrainVisionDataset::open(&p).is_err());
}
