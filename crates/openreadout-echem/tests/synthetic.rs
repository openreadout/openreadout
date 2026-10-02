//! Synthetic EC-Lab and Gamry files: layouts read back exactly, malformed files end in clean
//! errors (never a panic).

use std::path::Path;

use openreadout_core::{Error, FormatReader};
use openreadout_echem::{GamryReader, MprReader, MptReader, NdaReader, NdaxReader};

fn write(dir: &Path, name: &str, data: &[u8]) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, data).unwrap();
    p
}

/// A module with the older 57-byte header.
fn module(short: &str, version: u32, body: &[u8]) -> Vec<u8> {
    let mut m = b"MODULE".to_vec();
    let mut s = short.as_bytes().to_vec();
    s.resize(10, b' ');
    m.extend_from_slice(&s);
    m.extend_from_slice(&[b' '; 25]);
    m.extend_from_slice(&(body.len() as u32).to_le_bytes());
    m.extend_from_slice(&version.to_le_bytes());
    m.extend_from_slice(b"03/02/21");
    m.extend_from_slice(body);
    m
}

/// A version-3 `.mpr`: ids (flags 1, 3), 4 time f64, 6 Ewe f32; two records.
fn mpr(ids: &[u16], records: &[u8], points: u32) -> Vec<u8> {
    let mut f = b"BIO-LOGIC MODULAR FILE\x1a".to_vec();
    f.resize(0x34, b' ');
    f.extend_from_slice(&module("VMP Set", 0, &[0x0B, 0, 0]));
    let mut body = points.to_le_bytes().to_vec();
    body.push(ids.len() as u8);
    for i in ids {
        body.extend_from_slice(&i.to_le_bytes());
    }
    body.resize(406, 0);
    body.extend_from_slice(records);
    f.extend_from_slice(&module("VMP data", 3, &body));
    f
}

fn rec(flags: u8, t: f64, exp: f32) -> Vec<u8> {
    let mut r = vec![flags];
    r.extend_from_slice(&t.to_le_bytes());
    r.extend_from_slice(&exp.to_le_bytes());
    r
}

#[test]
fn mpr_layout_and_refusals() {
    let dir = tempfile::tempdir().unwrap();
    let mut recs = rec(0x03, 0.0, 0.25);
    recs.extend(rec(0x0B, 60.0, 0.5));
    let p = write(dir.path(), "a.mpr", &mpr(&[1, 3, 4, 6], &recs, 2));
    let mut ds = MprReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    let names: Vec<&str> = info.traces[0]
        .channels
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(names, ["time", "mode", "error", "ewe"]);
    let t = ds.read_trace(0, 0, 0, 9).unwrap();
    assert_eq!(t.channels[0], vec![0.0, 60.0]);
    assert_eq!(t.channels[1], vec![3.0, 3.0]);
    assert_eq!(t.channels[2], vec![0.0, 1.0]);
    assert_eq!(t.channels[3], vec![0.25, 0.5]);
    assert_eq!(info.traces[0].extra["technique"], "Open Circuit Voltage");
    // an unknown column id: refused
    let p = write(dir.path(), "b.mpr", &mpr(&[1, 4, 9999], &recs, 2));
    assert!(matches!(MprReader.open(&p), Err(Error::Unsupported { .. })));
    // a data module whose length disagrees with the records: corrupt
    let p = write(dir.path(), "c.mpr", &mpr(&[1, 3, 4, 6], &recs[..20], 2));
    assert!(matches!(MprReader.open(&p), Err(Error::Corrupt { .. })));
    // every truncation of a valid file is an error
    let full = mpr(&[1, 3, 4, 6], &recs, 2);
    for n in [22, 0x34, 0x40, 0x80, 200, full.len() - 1] {
        let p = write(dir.path(), "t.mpr", &full[..n]);
        assert!(MprReader.open(&p).is_err(), "cut at {n}");
    }
}

#[test]
fn mpt_header_comma_and_missing_columns() {
    let dir = tempfile::tempdir().unwrap();
    let t = "EC-Lab ASCII FILE\nNb header lines : 6\n\nOpen Circuit Voltage\nAcquisition started on : 01/16/2020 14:13:34.000\nmode\ttime/s\tEwe/V\t\tEwe-Ece/V\n3\t0,0\t2,5E-001\n3\t6,0E+001\t2,6E-001\n";
    let p = write(dir.path(), "a.mpt", t.as_bytes());
    let mut ds = MptReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    let names: Vec<&str> = info.traces[0]
        .channels
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(names, ["time", "mode", "ewe"]);
    let tr = ds.read_trace(0, 0, 0, 9).unwrap();
    assert_eq!(tr.channels[0], vec![0.0, 60.0]);
    assert_eq!(tr.channels[2], vec![0.25, 0.26]);
    let exp = ds.experiment().unwrap();
    assert_eq!(
        exp.acquisition.unwrap().started_at.as_deref(),
        Some("2020-01-16T14:13:34")
    );
    // a value that is not a number
    let p = write(dir.path(), "b.mpt", b"mode\ttime/s\tEwe/V\n3\tx\t1\n");
    assert!(matches!(MptReader.open(&p), Err(Error::Corrupt { .. })));
    // a header count past the end
    let p = write(
        dir.path(),
        "c.mpt",
        b"EC-Lab ASCII FILE\nNb header lines : 50\n",
    );
    assert!(MptReader.open(&p).is_err());
}

#[test]
fn gamry_tables_and_malformed() {
    let dir = tempfile::tempdir().unwrap();
    let t = "EXPLAIN\nTAG\tEISPOT\nTITLE\tLABEL\tEIS\tTest\nDATE\tLABEL\t6/5/2020\tDate\nTIME\tLABEL\t15:56:12\tTime\nPSTAT\tPSTAT\tIFC5000-04580\tPotentiostat\nNOTES\tNOTES\t1\tNotes\n\tfirst run\nZCURVE\tTABLE\t2\n\tPt\tTime\tFreq\tZreal\tZimag\n\t#\ts\tHz\tohm\tohm\n\t0\t1\t1000\t10,5\t-2\n\t1\t2\t100\t11\t-3\n";
    let p = write(dir.path(), "a.DTA", t.as_bytes());
    let mut ds = GamryReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces[0].name.as_deref(), Some("impedance"));
    let tr = ds.read_trace(0, 0, 0, 9).unwrap();
    assert_eq!(tr.channels[0], vec![1.0, 2.0]);
    assert_eq!(tr.channels[3], vec![10.5, 11.0]);
    let exp = ds.experiment().unwrap();
    assert_eq!(
        exp.instrument.unwrap().serial.as_deref(),
        Some("IFC5000-04580")
    );
    // declared and found rows differ: a warning
    let bad = t.replace("ZCURVE\tTABLE\t2", "ZCURVE\tTABLE\t5");
    let p = write(dir.path(), "b.DTA", bad.as_bytes());
    let mut ds = GamryReader.open(&p).unwrap();
    assert!(
        ds.check()
            .unwrap()
            .findings
            .iter()
            .any(|f| f.code == "count_mismatch")
    );
    // no table
    let p = write(dir.path(), "c.DTA", b"EXPLAIN\nTAG\tCV\n");
    assert!(matches!(GamryReader.open(&p), Err(Error::Corrupt { .. })));
}

/// A version-29 `.nda`: header, data section named at 0x40, an `0xAA` record and two
/// measurements (range -100: current and capacities in units of 1e-3 mA).
fn nda29(range: i32) -> Vec<u8> {
    let mut b = vec![0u8; 0x200];
    b[..14].copy_from_slice(b"NEWARE20200325");
    b[14] = 29;
    b[152..156].copy_from_slice(&2500u32.to_le_bytes());
    let mut recs = Vec::new();
    let mut other = vec![0u8; 86];
    other[0] = 0xAA;
    recs.extend(other);
    for (idx, t_ms, uv, cur) in [(1u32, 0u64, 38_000i32, -500i32), (2, 1000, 37_990, -500)] {
        let mut r = vec![0u8; 86];
        r[0] = 0x55;
        r[2..6].copy_from_slice(&idx.to_le_bytes());
        r[10..12].copy_from_slice(&1u16.to_le_bytes());
        r[12] = 2; // cc_discharge
        r[14..22].copy_from_slice(&t_ms.to_le_bytes());
        r[22..26].copy_from_slice(&uv.to_le_bytes());
        r[26..30].copy_from_slice(&cur.to_le_bytes());
        r[46..54].copy_from_slice(&(3600i64 * i64::from(idx)).to_le_bytes());
        r[70..72].copy_from_slice(&2020u16.to_le_bytes());
        r[72..77].copy_from_slice(&[3, 25, 11, 16, 50]);
        r[78..82].copy_from_slice(&range.to_le_bytes());
        recs.extend(r);
    }
    let start = u32::try_from(b.len()).unwrap();
    b[0x40..0x44].copy_from_slice(&start.to_le_bytes());
    b[0x44..0x48].copy_from_slice(&u32::try_from(recs.len()).unwrap().to_le_bytes());
    b.extend(recs);
    b
}

#[test]
fn nda29_records_and_refusals() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "a.nda", &nda29(-100));
    let mut ds = NdaReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    let names: Vec<&str> = info.traces[0]
        .channels
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(&names[..4], ["time", "step_time", "voltage", "current"]);
    let t = ds.read_trace(0, 0, 0, 9).unwrap();
    assert_eq!(t.channels[0], vec![0.0, 1.0]);
    assert_eq!(t.channels[2], vec![3.8, 3.799]);
    assert_eq!(t.channels[3], vec![-0.5, -0.5]);
    // discharge capacity: 3600 × 1e-3 / 3600 per index
    assert_eq!(t.channels[5], vec![0.001, 0.002]);
    let exp = ds.experiment().unwrap();
    assert_eq!(
        exp.acquisition.unwrap().started_at.as_deref(),
        Some("2020-03-25T11:16:50")
    );
    // a current range with no known scale: refused
    let p = write(dir.path(), "b.nda", &nda29(7));
    assert!(matches!(NdaReader.open(&p), Err(Error::Unsupported { .. })));
    // every truncation: an error, never a panic
    let full = nda29(-100);
    for n in [6, 15, 0x44, 0x200, full.len() - 1] {
        let p = write(dir.path(), "t.nda", &full[..n]);
        assert!(NdaReader.open(&p).is_err(), "cut at {n}");
    }
    // another version: refused
    let mut v = full.clone();
    v[14] = 31;
    let p = write(dir.path(), "v.nda", &v);
    assert!(matches!(NdaReader.open(&p), Err(Error::Unsupported { .. })));
}

/// A BTS 9.1 `.nda` (version 130): 52-byte `0x55` records from 1024, then a `0x81` footer.
fn nda91() -> Vec<u8> {
    let mut b = vec![0u8; 1024];
    b[..14].copy_from_slice(b"NEWARE20250527");
    b[14] = 130;
    for (idx, t, v, cap) in [
        (1u32, 0u32, 3.5f32, 0f32),
        (2, 10, 3.6, 36.0),
        (3, 20, 3.7, 72.0),
    ] {
        let mut r = vec![0u8; 52];
        r[0] = 0x55;
        r[2] = 2; // step 2
        r[3] = 1; // cc_charge
        r[8..12].copy_from_slice(&idx.to_le_bytes());
        r[12..16].copy_from_slice(&t.to_le_bytes());
        r[20..24].copy_from_slice(&100f32.to_le_bytes());
        r[24..28].copy_from_slice(&v.to_le_bytes());
        r[28..32].copy_from_slice(&cap.to_le_bytes());
        r[44..48].copy_from_slice(&(1_766_393_064u32 + t).to_le_bytes());
        b.extend(r);
    }
    let mut f = vec![0u8; 52];
    f[0] = 0x81;
    b.extend(f);
    b
}

#[test]
fn nda130_bts91_test_time() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "a.nda", &nda91());
    let mut ds = NdaReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    // the file stores the test time: no step_time channel
    assert!(
        info.traces[0]
            .channels
            .iter()
            .all(|c| c.name != "step_time")
    );
    let t = ds.read_trace(0, 0, 0, 9).unwrap();
    assert_eq!(t.channels[0], vec![0.0, 10.0, 20.0]);
    // charge capacity: 36 mA·s = 0.01 mA·h
    let cap = info.traces[0]
        .channels
        .iter()
        .position(|c| c.name == "charge_capacity")
        .unwrap();
    assert!((t.channels[cap][1] - 0.01).abs() < 1e-9);
    let exp = ds.experiment().unwrap();
    assert_eq!(
        exp.acquisition.unwrap().started_at.as_deref(),
        Some("2025-12-22T08:44:24.000Z")
    );
    // no footer: a warning
    let full = nda91();
    let p = write(dir.path(), "b.nda", &full[..full.len() - 52]);
    let mut ds = NdaReader.open(&p).unwrap();
    assert!(
        ds.check()
            .unwrap()
            .findings
            .iter()
            .any(|f| f.code == "no_footer")
    );
}

/// One `.ndc` file: header (file type, version) and pages of records.
fn ndc(kind: u8, version: u8, rec: usize, records: &[Vec<u8>]) -> Vec<u8> {
    let mut b = vec![0u8; 4096];
    b[0] = kind;
    b[2] = version;
    let per_page = (4096 - 132 - 4) / rec;
    for chunk in records.chunks(per_page.max(1)) {
        let mut page = vec![0u8; 4096];
        page[..2].copy_from_slice(&(u16::from(kind) + 1).to_le_bytes());
        page[2..4].copy_from_slice(&u16::try_from(chunk.len()).unwrap().to_le_bytes());
        for k in 0..chunk.len() {
            page[4 + k / 8] |= 1 << (k % 8);
        }
        for (k, r) in chunk.iter().enumerate() {
            page[132 + k * rec..132 + (k + 1) * rec].copy_from_slice(r);
        }
        let crc = crc32(&page[..4092]);
        page[4092..].copy_from_slice(&crc.to_le_bytes());
        b.extend(page);
    }
    b
}

/// CRC-32 (IEEE), bitwise.
fn crc32(b: &[u8]) -> u32 {
    let mut c = !0u32;
    for &x in b {
        c ^= u32::from(x);
        for _ in 0..8 {
            c = if c & 1 == 1 {
                (c >> 1) ^ 0xEDB8_8320
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// A version-14 `.ndax`: four records, two of them logged, one temperature channel.
fn ndax14(corrupt_page: bool) -> Vec<u8> {
    let data: Vec<Vec<u8>> = [(3.0f32, 0.1f32), (3.1, 0.1), (3.2, 0.1), (3.3, 0.1)]
        .iter()
        .map(|(v, a)| [v.to_le_bytes(), a.to_le_bytes()].concat())
        .collect();
    let mut step = vec![0u8; 37];
    step[4..8].copy_from_slice(&1i32.to_le_bytes());
    step[24] = 1; // cc_charge
    let run = |index: i32, t_ms: i32, dt_ms: i32, cap_ah: f32| {
        let mut r = vec![0u8; 55];
        r[..4].copy_from_slice(&t_ms.to_le_bytes());
        r[5..9].copy_from_slice(&cap_ah.to_le_bytes());
        r[29..33].copy_from_slice(&dt_ms.to_le_bytes());
        r[33..37].copy_from_slice(&1_731_933_825i32.to_le_bytes());
        r[37..41].copy_from_slice(&1i32.to_le_bytes());
        r[41..45].copy_from_slice(&index.to_le_bytes());
        r
    };
    let runs = vec![run(1, 0, 1000, 0.0), run(3, 2000, 1000, 0.002)];
    let temps: Vec<Vec<u8>> = [25.0f32, 25.5, 26.0, 26.5]
        .iter()
        .map(|t| t.to_le_bytes().to_vec())
        .collect();
    let mut data_ndc = ndc(1, 14, 8, &data);
    if corrupt_page {
        data_ndc[4096 + 140] ^= 0xFF;
    }
    let test_info = r#"<?xml version="1.0" encoding="GB2312"?><root><config><TestInfo DevID="27" UnitID="0" ChlID="81" StepName="demo.xml" Barcode="B1"><Aux1 AuxID="1" RealChlID="9" ChlType="103"/></TestInfo></config></root>"#;
    openreadout_core::zip::zip_bytes(&[
        ("TestInfo.xml", test_info.as_bytes()),
        ("data.ndc", &data_ndc),
        ("data_step.ndc", &ndc(7, 14, 37, &[step])),
        ("data_runInfo.ndc", &ndc(18, 14, 55, &runs)),
        ("data_AUX_9_3_1.ndc", &ndc(5, 14, 4, &temps)),
    ])
    .unwrap()
}

#[test]
fn ndax14_logged_records_and_crc() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "a.ndax", &ndax14(false));
    let mut ds = NdaxReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    let ch = |n: &str| {
        info.traces[0]
            .channels
            .iter()
            .position(|c| c.name == n)
            .unwrap()
    };
    let t = ds.read_trace(0, 0, 0, 9).unwrap();
    assert_eq!(t.channels[ch("step_time")], vec![0.0, 1.0, 2.0, 3.0]);
    assert!(
        t.channels[ch("current")]
            .iter()
            .all(|v| (v - 100.0).abs() < 1e-4)
    );
    let cap = &t.channels[ch("charge_capacity")];
    assert!(cap[0] == 0.0 && cap[1].is_nan() && (cap[2] - 2.0).abs() < 1e-6 && cap[3].is_nan());
    assert_eq!(
        t.channels[ch("aux_temperature_1")],
        vec![25.0, 25.5, 26.0, 26.5]
    );
    assert_eq!(t.channels[ch("cycle")], vec![1.0; 4]);
    let exp = ds.experiment().unwrap();
    assert_eq!(exp.method.unwrap().name.as_deref(), Some("demo"));
    assert_eq!(exp.sample.unwrap().barcode.as_deref(), Some("B1"));
    // a page whose CRC does not match: corrupt
    let p = write(dir.path(), "b.ndax", &ndax14(true));
    assert!(matches!(NdaxReader.open(&p), Err(Error::Corrupt { .. })));
    // a truncated zip
    let z = ndax14(false);
    let p = write(dir.path(), "c.ndax", &z[..z.len() / 2]);
    assert!(NdaxReader.open(&p).is_err());
}
