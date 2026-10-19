//! Synthetic-file tests of the MCS HDF5 reader, with HDF5 files written by hdf5-pure.

use hdf5_pure::{
    AttrValue, CharacterSet, CompoundTypeBuilder, Datatype, FileBuilder, StringPadding,
    make_i32_type, make_i64_type,
};

use super::*;

fn text_type(n: u32) -> Datatype {
    Datatype::String {
        size: n,
        padding: StringPadding::NullPad,
        charset: CharacterSet::Ascii,
    }
}

/// InfoChannel records with fixed-length strings (the files use variable-length ones).
/// ChannelID, RowIndex, Label, Unit, Exponent, ADZero, Tick, ConversionFactor.
type Row<'a> = (i32, i32, &'a str, &'a str, i32, i32, i64, i64);

fn info_channel(rows: &[Row<'_>]) -> (Datatype, Vec<u8>) {
    let dt = CompoundTypeBuilder::with_size(64)
        .field("ChannelID", 0, make_i32_type())
        .field("RowIndex", 4, make_i32_type())
        .field("Label", 8, text_type(8))
        .field("Unit", 16, text_type(8))
        .field("Exponent", 24, make_i32_type())
        .field("ADZero", 28, make_i32_type())
        .field("Tick", 32, make_i64_type())
        .field("ConversionFactor", 40, make_i64_type())
        .build()
        .unwrap();
    let mut raw = Vec::new();
    for (id, row, label, unit, exp, zero, tick, cf) in rows {
        let mut r = vec![0u8; 64];
        r[0..4].copy_from_slice(&id.to_le_bytes());
        r[4..8].copy_from_slice(&row.to_le_bytes());
        r[8..8 + label.len()].copy_from_slice(label.as_bytes());
        r[16..16 + unit.len()].copy_from_slice(unit.as_bytes());
        r[24..28].copy_from_slice(&exp.to_le_bytes());
        r[28..32].copy_from_slice(&zero.to_le_bytes());
        r[32..40].copy_from_slice(&tick.to_le_bytes());
        r[40..48].copy_from_slice(&cf.to_le_bytes());
        raw.extend_from_slice(&r);
    }
    (dt, raw)
}

fn file(segments: &[[i64; 3]]) -> Vec<u8> {
    let mut b = FileBuilder::new();
    b.set_attr("McsHdf5ProtocolType", AttrValue::String("RawData".into()));
    b.set_attr("McsHdf5ProtocolVersion", AttrValue::I32(3));
    b.set_attr(
        "GeneratingApplicationName",
        AttrValue::String("Multi Channel DataManager".into()),
    );
    b.set_attr(
        "GeneratingApplicationVersion",
        AttrValue::String("1.14".into()),
    );
    let mut data = b.create_group("Data");
    data.set_attr("ProgramName", AttrValue::String("MC_Rack".into()));
    data.set_attr("DateInTicks", AttrValue::I64(635_404_978_551_720_981));
    let mut rec = data.create_group("Recording_0");
    let mut analog = rec.create_group("AnalogStream");
    let mut s = analog.create_group("Stream_0");
    s.set_attr("Label", AttrValue::String("Electrode Raw Data".into()));
    s.set_attr("DataSubType", AttrValue::String("Electrode".into()));
    // two channels stored in swapped rows; 6 samples each
    let (dt, raw) = info_channel(&[
        (0, 1, "E1", "V", -12, 10, 50, 1_000),
        (1, 0, "E2", "V", -12, 0, 50, 2_000),
    ]);
    s.create_dataset("InfoChannel")
        .with_compound_data(dt, raw, 2);
    let samples: Vec<i32> = vec![1, 2, 3, 4, 5, 6, 10, 20, 30, 40, 50, 60];
    s.create_dataset("ChannelData")
        .with_i32_data(&samples)
        .with_shape(&[2, 6])
        .with_chunks(&[2, 4])
        .with_shuffle()
        .with_deflate(4);
    let ts: Vec<i64> = segments.iter().flatten().copied().collect();
    s.create_dataset("ChannelDataTimeStamps")
        .with_i64_data(&ts)
        .with_shape(&[segments.len() as u64, 3]);
    analog.add_group(s.finish());
    rec.add_group(analog.finish());
    let mut ev = rec.create_group("EventStream");
    let mut es = ev.create_group("Stream_0");
    es.set_attr("Label", AttrValue::String("trig".into()));
    es.create_dataset("EventEntity_0")
        .with_i64_data(&[1_000, 500_000, 20, 30])
        .with_shape(&[2, 2]);
    ev.add_group(es.finish());
    rec.add_group(ev.finish());
    data.add_group(rec.finish());
    b.add_group(data.finish());
    b.finish().unwrap()
}

fn open(bytes: Vec<u8>) -> (tempfile::TempDir, McsHdf5Dataset) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("rec.h5");
    std::fs::write(&p, bytes).unwrap();
    let ds = McsHdf5Dataset::open(&p).unwrap();
    (dir, ds)
}

#[test]
fn reads_channels_from_their_rows_with_the_documented_scaling() {
    let (_d, mut ds) = open(file(&[[100, 0, 5]]));
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("RawData 3"));
    let t = &info.traces[0];
    assert_eq!(t.name.as_deref(), Some("Electrode Raw Data"));
    assert_eq!(t.sample_rate_hz, 20_000.0);
    assert_eq!(t.sample_count, 6);
    assert_eq!(t.start_s, Some(1e-4));
    assert_eq!(t.channels[0].name, "E1");
    assert_eq!(t.channels[0].unit.as_deref(), Some("V"));
    assert_eq!(t.extra["recorded_at"], "2014-07-09T10:17:35.172098");
    assert_eq!(t.extra["writer"], "Multi Channel DataManager");
    let tr = ds.read_trace(0, 0, 0, 100).unwrap();
    // E1 is row 1, zero 10, step 1000e-12
    let step = 1_000.0 * 1e-12;
    assert_eq!(tr.channels[0][0], (10.0 - 10.0) * step);
    assert_eq!(tr.channels[0][5], (60.0 - 10.0) * step);
    assert_eq!(tr.channels[1][2], 3.0 * (2_000.0 * 1e-12));
    let tr = ds.read_trace(0, 0, 4, 100).unwrap();
    assert_eq!(tr.channels[1].len(), 2);
    let table = &info.tables[0];
    assert_eq!(table.name.as_deref(), Some("events"));
    assert_eq!(table.row_count, 2);
    let rows = ds.read_table(0, 0, 10).unwrap();
    assert_eq!(rows.columns[0], vec![0.001, 0.5]);
    assert_eq!(rows.columns[1], vec![2e-5, 3e-5]);
}

#[test]
fn segments_are_sweeps() {
    let (_d, mut ds) = open(file(&[[0, 0, 2], [1_000, 3, 5]]));
    let t = &ds.info().unwrap().traces[0];
    assert_eq!(t.sweep_count, 2);
    assert_eq!(t.extra["sweep_starts_s"], json!([0.0, 0.001]));
    let tr = ds.read_trace(0, 1, 0, 10).unwrap();
    let step = 2_000.0 * 1e-12;
    assert_eq!(tr.channels[1], vec![4.0 * step, 5.0 * step, 6.0 * step]);
}

#[test]
fn bad_segments_are_findings_and_detection_needs_the_protocol() {
    let (_d, mut ds) = open(file(&[[0, 0, 9]]));
    assert_eq!(ds.mcs().analog[0].segments.len(), 0);
    let r = ds.check().unwrap();
    assert!(r.findings.iter().any(|f| f.code == "bad_segment"));
    let b = file(&[[0, 0, 5]]);
    let head = &b[..b.len().min(65_536)];
    let found = McsHdf5Reader.sniff(head, Path::new("x.h5")).is_some();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("y.h5");
    std::fs::write(&p, &b).unwrap();
    let probed = McsHdf5Reader.sniff_input(head, &Input::local(&p));
    assert!(found || probed.is_some());
    // another HDF5 file is not claimed
    let mut other = FileBuilder::new();
    other.create_dataset("x").with_i32_data(&[1]);
    let o = other.finish().unwrap();
    std::fs::write(&p, &o).unwrap();
    assert!(McsHdf5Reader.sniff_input(&o, &Input::local(&p)).is_none());
}

#[test]
fn malformed_files_give_errors_not_panics() {
    let good = file(&[[0, 0, 5]]);
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("m.h5");
    for cut in [8, 100, 600, good.len() / 2, good.len() - 10] {
        std::fs::write(&p, &good[..cut]).unwrap();
        if let Ok(mut ds) = McsHdf5Dataset::open(&p) {
            let _ = ds.info();
            let _ = ds.read_trace(0, 0, 0, 10);
            let _ = ds.check();
        }
    }
}
