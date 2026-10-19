//! Synthetic-file tests of the BRW reader, with HDF5 files written by hdf5-pure.

use hdf5_pure::{AttrValue, CompoundTypeBuilder, FileBuilder, make_i16_type};

use super::*;

fn v3(version: Option<i64>, inversion: f64) -> Vec<u8> {
    let mut b = FileBuilder::new();
    let mut info = b.create_group("3BRecInfo");
    let mut vars = info.create_group("3BRecVars");
    vars.create_dataset("BitDepth").with_u8_data(&[12]);
    vars.create_dataset("MaxVolt").with_f64_data(&[4125.0]);
    vars.create_dataset("MinVolt").with_f64_data(&[-4125.0]);
    vars.create_dataset("NRecFrames").with_i64_data(&[3]);
    vars.create_dataset("SamplingRate")
        .with_f64_data(&[17855.5]);
    vars.create_dataset("SignalInversion")
        .with_f64_data(&[inversion]);
    info.add_group(vars.finish());
    let mut streams = info.create_group("3BMeaStreams");
    let mut raw = streams.create_group("Raw");
    let dt = CompoundTypeBuilder::with_size(4)
        .field("Row", 0, make_i16_type())
        .field("Col", 2, make_i16_type())
        .build()
        .unwrap();
    let mut chs = Vec::new();
    for (r, c) in [(1i16, 2i16), (3, 4)] {
        chs.extend_from_slice(&r.to_le_bytes());
        chs.extend_from_slice(&c.to_le_bytes());
    }
    raw.create_dataset("Chs").with_compound_data(dt, chs, 2);
    streams.add_group(raw.finish());
    info.add_group(streams.finish());
    b.add_group(info.finish());
    let mut data = b.create_group("3BData");
    if let Some(v) = version {
        data.set_attr("Version", AttrValue::I64(v));
    }
    let samples: Vec<u16> = vec![0, 4096, 2048, 1, 100, 200];
    if version == Some(100) {
        data.create_dataset("Raw")
            .with_u16_data(&samples)
            .with_shape(&[3, 2]);
    } else {
        data.create_dataset("Raw").with_u16_data(&samples);
    }
    b.add_group(data.finish());
    b.finish().unwrap()
}

fn v4(sparse: bool) -> Vec<u8> {
    let mut b = FileBuilder::new();
    let settings = r#"{"ValueConverter":{"MinAnalogValue":-8000.0,"MaxAnalogValue":8000.0,"MaxDigitalValue":4095,"MinDigitalValue":0,"ScaleFactor":1.0},"TimeConverter":{"FrameRate":19753.775390625},"MeaSystem":{"FwVersion":{"Major":1,"Minor":7,"Build":0,"Revision":12}},"ExperimentDateTime":"2023-04-30T11:56:37.143122Z"}"#;
    b.create_dataset("ExperimentSettings")
        .with_vlen_strings(&[settings]);
    let mut w = b.create_group("Well_A1");
    w.create_dataset("StoredChIdxs").with_i32_data(&[5, 70, 99]);
    if sparse {
        w.create_dataset("EventsBasedSparseRaw")
            .with_u8_data(&[0; 16]);
    } else {
        w.create_dataset("Raw")
            .with_u16_data(&[0, 1, 2, 3, 4, 4095]);
    }
    b.add_group(w.finish());
    b.finish().unwrap()
}

fn open(bytes: Vec<u8>) -> Result<(tempfile::TempDir, BrwDataset)> {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("r.brw");
    std::fs::write(&p, bytes).unwrap();
    BrwDataset::open(&p).map(|ds| (dir, ds))
}

#[test]
fn v3_interleaved_and_matrix_layouts_read_alike() {
    let gain = 8250.0 / 4096.0;
    for v in [None, Some(101), Some(100)] {
        let (_d, mut ds) = open(v3(v, 1.0)).unwrap();
        let info = ds.info().unwrap();
        assert_eq!(info.format_version.as_deref(), Some("BRW 3.x"));
        let t = &info.traces[0];
        assert_eq!(t.sample_count, 3);
        assert_eq!(t.channels[1].name, "ch3-4");
        assert_eq!(t.sample_rate_hz, 17855.5);
        let tr = ds.read_trace(0, 0, 0, 10).unwrap();
        assert_eq!(
            tr.channels[0],
            vec![
                0.0 * gain - 4125.0,
                2048.0 * gain - 4125.0,
                100.0 * gain - 4125.0
            ]
        );
        assert_eq!(tr.channels[1][2], 200.0 * gain - 4125.0);
        let tr = ds.read_trace(0, 0, 1, 1).unwrap();
        assert_eq!(tr.channels[1], vec![gain - 4125.0]);
    }
}

#[test]
fn v3_signal_inversion() {
    let (_d, mut ds) = open(v3(Some(101), -1.0)).unwrap();
    let gain = 8250.0 / 4096.0;
    let tr = ds.read_trace(0, 0, 0, 1).unwrap();
    assert_eq!(tr.channels[1], vec![(4096.0 - 4096.0) * gain - 4125.0]);
    assert_eq!(tr.channels[0], vec![4096.0 * gain - 4125.0]);
}

#[test]
fn v4_wells_and_refusing_sparse_data() {
    let (_d, mut ds) = open(v4(false)).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("BRW 4.x"));
    let t = &info.traces[0];
    assert_eq!(t.name.as_deref(), Some("Well_A1"));
    assert_eq!(t.sample_count, 2);
    assert_eq!(t.channels[1].name, "ch70");
    assert_eq!(t.extra["firmware_version"], "1.7.0.12");
    assert_eq!(t.extra["recorded_at"], "2023-04-30T11:56:37.143122Z");
    let gain = 1.0 * 16000.0 / 4095.0;
    let tr = ds.read_trace(0, 0, 0, 10).unwrap();
    assert_eq!(
        tr.channels[2],
        vec![2.0 * gain - 8000.0, 4095.0 * gain - 8000.0]
    );
    let e = open(v4(true)).unwrap_err();
    assert_eq!(e.exit_code(), 6);
}

#[test]
fn detection_and_malformed_files() {
    let b = v3(Some(101), 1.0);
    assert!(BrwReader.sniff(&b, Path::new("x.brw")).is_some());
    assert!(BrwReader.sniff(b"not hdf5", Path::new("x.brw")).is_none());
    for cut in [8, 200, b.len() / 2, b.len() - 4] {
        if let Ok((_d, mut ds)) = open(b[..cut].to_vec()) {
            let _ = ds.info();
            let _ = ds.read_trace(0, 0, 0, 10);
        }
    }
}
