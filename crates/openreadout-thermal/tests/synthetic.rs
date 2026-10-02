//! Synthetic NETZSCH `.ngb-*` files built from the record grammar: channels read back exactly,
//! and malformed containers end in clean errors (never a panic).

use std::path::Path;

use openreadout_core::{Error, FormatReader};
use openreadout_thermal::NgbReader;

const HEADER: &[u8] = &[0x18, 0xfc, 0xff, 0xff, 0x03, 0x80, 0x01];
const MIDDLE: &[u8] = &[0, 0, 1, 0, 0, 0, 0x0c, 0, 0x17, 0xfc, 0xff, 0xff];
const END_FIELD: &[u8] = &[1, 0, 0, 0, 2, 0, 1, 0, 0];

fn record(id: u16, dtype: u8, array: Option<u32>, payload: &[u8]) -> Vec<u8> {
    let mut r = HEADER.to_vec();
    r.extend(id.to_le_bytes());
    r.extend(MIDDLE);
    r.push(dtype);
    if let Some(n) = array {
        r.extend([0xa0, 0x01]);
        r.extend(n.to_le_bytes());
    } else {
        r.extend([0x80, 0x01]);
    }
    r.extend(payload);
    r.extend(END_FIELD);
    r
}

/// A table open: a reference record `01 80 02 00 00 80 <type> 00 00` under the category id.
fn open(category: u16, type_ref: u16) -> Vec<u8> {
    let mut p = vec![0x01, 0x80, 2, 0, 0, 0x80];
    p.extend(type_ref.to_le_bytes());
    p.extend([0, 0]);
    record(category, 0x1a, None, &p)
}

/// A stream member: header, a one-entry section directory, the section.
fn stream(id: u16, body: &[u8]) -> Vec<u8> {
    let mut b = vec![0u8; 0x50];
    b[2..17].copy_from_slice(b"Netzsch TA file");
    b[28..40].copy_from_slice(b"_db_format_1");
    let start = 0x50 + 14;
    b.extend([0xff, 0xff]);
    b.extend(id.to_le_bytes());
    b.extend(u32::try_from(start).unwrap().to_le_bytes());
    b.extend(u32::try_from(body.len()).unwrap().to_le_bytes());
    b.extend([0, 0]);
    b.extend(body);
    b
}

fn f64s(v: &[f64]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn f32s(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn ngb(truncate_data: bool) -> Vec<u8> {
    // stream 1: the sample descriptor (category 0x7530): name and mass
    let mut s1 = open(0x7530, 0x2AFB);
    let name = "PLA";
    let mut p = u32::try_from(name.len()).unwrap().to_le_bytes().to_vec();
    p.extend(name.as_bytes());
    s1.extend(record(0x0840, 0x1f, None, &p));
    s1.extend(record(0x0C9E, 0x05, None, &5.5f64.to_le_bytes()));
    // stream 2: time (f64, minutes) in two segments, then sample temperature (f32)
    let mut s2 = open(0x178C, 0x2B22);
    s2.extend(open(0x0001, 0x2B23));
    s2.extend(record(0x0F40, 0x05, Some(2), &f64s(&[0.0, 0.5])));
    s2.extend(open(0x0002, 0x2B23));
    s2.extend(record(0x0F40, 0x05, Some(1), &f64s(&[1.0])));
    s2.extend(open(0x178D, 0x2B22));
    s2.extend(open(0x0001, 0x2B23));
    let mut temps = record(0x0F3D, 0x04, Some(3), &f32s(&[25.0, 30.0, 35.5]));
    if truncate_data {
        temps.truncate(temps.len() - 12);
    }
    s2.extend(temps);
    openreadout_core::zip::zip_bytes(&[
        ("Streams/stream_1.table", &stream(1, &s1)),
        ("Streams/stream_2.table", &stream(2, &s2)),
    ])
    .unwrap()
}

fn write(dir: &Path, name: &str, data: &[u8]) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, data).unwrap();
    p
}

#[test]
fn channels_and_metadata() {
    let d = tempfile::tempdir().unwrap();
    let p = write(d.path(), "a.ngb-sd7", &ngb(false));
    let mut ds = NgbReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    let names: Vec<&str> = info.traces[0]
        .channels
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(names, ["time", "sample_temperature"]);
    let t = ds.read_trace(0, 0, 0, 9).unwrap();
    assert_eq!(t.channels[0], vec![0.0, 30.0, 60.0]);
    assert_eq!(t.channels[1], vec![25.0, 30.0, 35.5]);
    let e = ds.experiment().unwrap();
    assert_eq!(e.sample.unwrap().name.as_deref(), Some("PLA"));
    assert_eq!(e.method.unwrap().parameters["sample_mass"].value, 5.5);
}

#[test]
fn malformed_containers() {
    let d = tempfile::tempdir().unwrap();
    // a truncated data array: the data stream is refused as corrupt
    let p = write(d.path(), "b.ngb-ss3", &ngb(true));
    assert!(matches!(NgbReader.open(&p), Err(Error::Corrupt { .. })));
    // a zip without the streams
    let z = openreadout_core::zip::zip_bytes(&[("Streams/Props.xml", b"<x/>".as_slice())]).unwrap();
    let p = write(d.path(), "c.ngb-taa", &z);
    assert!(NgbReader.open(&p).is_err());
    // a cut archive
    let full = ngb(false);
    let p = write(d.path(), "d.ngb-ss3", &full[..full.len() / 2]);
    assert!(NgbReader.open(&p).is_err());
    // a stream whose directory runs past its end
    let mut s = stream(2, &[0u8; 10]);
    s[0x50 + 8..0x50 + 12].copy_from_slice(&1000u32.to_le_bytes());
    let z = openreadout_core::zip::zip_bytes(&[
        ("Streams/stream_1.table", &stream(1, &open(1, 0x2AFB))),
        ("Streams/stream_2.table", &s),
    ])
    .unwrap();
    let p = write(d.path(), "e.ngb-ss3", &z);
    assert!(matches!(NgbReader.open(&p), Err(Error::Corrupt { .. })));
}

/// A TA data file: UTF-16 header, form feed, signal count, float32 records, end record.
fn ta001(records: &[[f32; 3]], end_record: bool, stored_nsig: u8) -> Vec<u8> {
    let header = "CLOSED\r\nVERSION 2.0\r\nInstrument DSC Q20 V24.11 Build 124\r\nInstSerial 0020-1234\r\nOperator Lab\r\nSample Indium\r\nSize 5.5000 mg\r\nNsig 3\r\nSig1 Time (min)\r\nSig2 Temperature (°C)\r\nSig3 Heat Flow (mW)\r\nDate 2022-02-22\r\nTime 17:09:26\r\nOrgMethod 1: Ramp 10.00 °C/min to 200.00 °C\r\n\u{c}";
    let mut b = vec![0xff, 0xfe];
    for u in header.encode_utf16() {
        b.extend(u.to_le_bytes());
    }
    b.push(stored_nsig);
    for r in records {
        for v in r {
            b.extend(v.to_le_bytes());
        }
    }
    if end_record {
        for v in [-100f32, 20.0, 0.0] {
            b.extend(v.to_le_bytes());
        }
        b.extend([0u8; 24]);
    }
    b
}

#[test]
fn ta_records_and_header() {
    use openreadout_thermal::TaReader;
    let d = tempfile::tempdir().unwrap();
    let recs = [[0.5f32, 30.0, 1.5], [1.0, 35.0, -2.25]];
    let p = write(d.path(), "Data.001", &ta001(&recs, true, 3));
    let mut ds = TaReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    let names: Vec<&str> = info.traces[0]
        .channels
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(names, ["time", "temperature", "heat_flow"]);
    let t = ds.read_trace(0, 0, 0, 9).unwrap();
    assert_eq!(t.channels[0], vec![30.0, 60.0]);
    assert_eq!(t.channels[2], vec![1.5, -2.25]);
    let e = ds.experiment().unwrap();
    assert_eq!(
        e.instrument.as_ref().unwrap().model.as_deref(),
        Some("DSC Q20")
    );
    assert_eq!(
        e.acquisition.unwrap().started_at.as_deref(),
        Some("2022-02-22T17:09:26")
    );
    assert_eq!(e.method.unwrap().parameters["sample_mass"].value, 5.5);
    // no end record: read, with a warning
    let p = write(d.path(), "Data.002", &ta001(&recs, false, 3));
    let mut ds = TaReader.open(&p).unwrap();
    assert!(
        ds.check()
            .unwrap()
            .findings
            .iter()
            .any(|f| f.code == "no_end_record")
    );
    // a data block that disagrees with Nsig: refused
    let p = write(d.path(), "Data.003", &ta001(&recs, true, 4));
    assert!(matches!(TaReader.open(&p), Err(Error::Unsupported { .. })));
    // every truncation: an error or a clean read, never a panic
    let full = ta001(&recs, true, 3);
    for n in [2, 20, 200, full.len() - 30] {
        let p = write(d.path(), "Data.004", &full[..n]);
        let _ = TaReader.open(&p).map(|ds| ds.info());
    }
}
