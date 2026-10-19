//! Synthetic-file tests of the MaxWell reader, with HDF5 files written by hdf5-pure.

use hdf5_pure::{CompoundTypeBuilder, FileBuilder, make_f64_type, make_i32_type};

use super::*;

fn mapping_bytes(rows: &[(i32, i32, f64, f64)]) -> (hdf5_pure::Datatype, Vec<u8>) {
    let dt = CompoundTypeBuilder::with_size(24)
        .field("channel", 0, make_i32_type())
        .field("electrode", 4, make_i32_type())
        .field("x", 8, make_f64_type())
        .field("y", 16, make_f64_type())
        .build()
        .unwrap();
    let mut raw = Vec::new();
    for (c, e, x, y) in rows {
        raw.extend_from_slice(&c.to_le_bytes());
        raw.extend_from_slice(&e.to_le_bytes());
        raw.extend_from_slice(&x.to_le_bytes());
        raw.extend_from_slice(&y.to_le_bytes());
    }
    (dt, raw)
}

/// Layout 20160704: four amplifier rows, channels 3 and 1 routed.
fn legacy() -> Vec<u8> {
    let mut b = FileBuilder::new();
    b.create_dataset("version")
        .with_strings(&["20160704"])
        .unwrap();
    b.create_dataset("message_0")
        .with_strings(&["Recorded with MaxLab (C) in sequence mode"])
        .unwrap();
    b.create_dataset("time")
        .with_strings(&["start: 2020-01-23 16:27:30;\nstop: 2020-01-23 16:30:30\n"])
        .unwrap();
    let (dt, raw) = mapping_bytes(&[(3, 900, 17.5, 35.0), (1, 12, 0.0, 17.5)]);
    b.create_dataset("mapping").with_compound_data(dt, raw, 2);
    let sig: Vec<u16> = (0..12).collect();
    b.create_dataset("sig")
        .with_u16_data(&sig)
        .with_shape(&[4, 3])
        .with_chunks(&[4, 2])
        .with_deflate(4);
    let mut s = b.create_group("settings");
    s.create_dataset("lsb").with_f64_data(&[6.29425e-6]);
    b.add_group(s.finish());
    b.finish().unwrap()
}

/// Layout 20190530: one well, rows hold channels 7 and 2.
fn current() -> Vec<u8> {
    let mut b = FileBuilder::new();
    b.create_dataset("version")
        .with_strings(&["20190530"])
        .unwrap();
    b.create_dataset("mxw_version")
        .with_strings(&["20.1.8"])
        .unwrap();
    let mut wp = b.create_group("wellplate");
    wp.create_dataset("version")
        .with_strings(&["MaxOne Single Well MEA"])
        .unwrap();
    b.add_group(wp.finish());
    let mut wells = b.create_group("wells");
    let w0 = wells.create_group("well000");
    wells.add_group(w0.finish());
    b.add_group(wells.finish());
    let mut store = b.create_group("data_store");
    let mut d = store.create_group("data0000");
    d.create_dataset("well_id").with_i32_data(&[0]);
    d.create_dataset("recording_id").with_i32_data(&[0]);
    d.create_dataset("start_time")
        .with_i64_data(&[1_616_517_845_839]);
    let mut set = d.create_group("settings");
    set.create_dataset("lsb").with_f64_data(&[1e-5]);
    set.create_dataset("sampling").with_f64_data(&[10_000.0]);
    let (dt, raw) = mapping_bytes(&[(2, 20, 1.0, 2.0), (7, 70, 3.0, 4.0)]);
    set.create_dataset("mapping").with_compound_data(dt, raw, 2);
    d.add_group(set.finish());
    let mut g = d.create_group("groups");
    let mut routed = g.create_group("routed");
    routed.create_dataset("channels").with_u16_data(&[7, 2]);
    routed
        .create_dataset("raw")
        .with_u16_data(&[100, 101, 102, 200, 201, 202])
        .with_shape(&[2, 3]);
    g.add_group(routed.finish());
    d.add_group(g.finish());
    store.add_group(d.finish());
    b.add_group(store.finish());
    b.finish().unwrap()
}

fn open(bytes: Vec<u8>) -> (tempfile::TempDir, MaxwellDataset) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("data.raw.h5");
    std::fs::write(&p, bytes).unwrap();
    let ds = MaxwellDataset::open(&p).unwrap();
    (dir, ds)
}

#[test]
fn legacy_layout_reads_routed_rows_by_channel() {
    let (_d, mut ds) = open(legacy());
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("20160704"));
    let t = &info.traces[0];
    assert_eq!(t.sample_rate_hz, 20_000.0);
    assert_eq!(t.extra["sample_rate_assumed"], true);
    assert_eq!(t.extra["recorded_at"], "2020-01-23T16:27:30");
    assert_eq!(t.channels[0].name, "ch1");
    assert_eq!(t.channels[1].extra["electrode"], 900);
    let uv = 6.29425e-6 * 1e6;
    let tr = ds.read_trace(0, 0, 1, 5).unwrap();
    assert_eq!(tr.channels[0], vec![4.0 * uv, 5.0 * uv]);
    assert_eq!(tr.channels[1], vec![10.0 * uv, 11.0 * uv]);
}

#[test]
fn current_layout_reads_rows_by_their_channel_list() {
    let (_d, mut ds) = open(current());
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("20190530"));
    let t = &info.traces[0];
    assert_eq!(t.name.as_deref(), Some("well000 rec0000"));
    assert_eq!(t.sample_rate_hz, 10_000.0);
    assert_eq!(t.extra["recorded_at"], "2021-03-23T16:44:05.839Z");
    assert_eq!(t.channels[0].name, "ch2");
    assert_eq!(t.channels[0].extra["electrode"], 20);
    let uv = 1e-5 * 1e6;
    let tr = ds.read_trace(0, 0, 0, 10).unwrap();
    assert_eq!(tr.channels[0], vec![200.0 * uv, 201.0 * uv, 202.0 * uv]);
    assert_eq!(tr.channels[1][0], 100.0 * uv);
}

#[test]
fn detection_and_malformed_files() {
    let b = legacy();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("x.raw.h5");
    std::fs::write(&p, &b).unwrap();
    assert!(MaxwellReader.sniff_input(&b, &Input::local(&p)).is_some());
    let c = current();
    std::fs::write(&p, &c).unwrap();
    assert!(MaxwellReader.sniff_input(&c, &Input::local(&p)).is_some());
    for cut in [8, 300, c.len() / 2, c.len() - 5] {
        std::fs::write(&p, &c[..cut]).unwrap();
        if let Ok(mut ds) = MaxwellDataset::open(&p) {
            let _ = ds.info();
            let _ = ds.read_trace(0, 0, 0, 10);
        }
    }
}
