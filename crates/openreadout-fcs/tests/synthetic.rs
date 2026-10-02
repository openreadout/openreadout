//! Synthetic FCS files built from the FCS 3.1 layout rules, covering features the public corpus
//! does not (ASCII data, CRC values, histogram mode, bit-packed widths, $COMP, delimiter escapes).

use std::path::PathBuf;

use openreadout_core::model::Severity;
use openreadout_core::reader::FormatReader;
use openreadout_core::{Dataset, Error};
use openreadout_fcs::{Crc16, FcsReader};

struct Built {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

fn esc(s: &str, d: char) -> String {
    s.replace(d, &format!("{d}{d}"))
}

/// One data set: HEADER at 0, TEXT at 58+, DATA right after TEXT. Offsets in TEXT are written
/// with fixed-width leading zeros so the TEXT length does not depend on them.
fn data_set(version: &str, kws: &[(&str, String)], data: &[u8], delim: char) -> Vec<u8> {
    let render = |begin: u64, end: u64| {
        let mut t = String::new();
        t.push(delim);
        let mut all: Vec<(String, String)> = kws
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect();
        if version != "FCS2.0" {
            all.push(("$BEGINDATA".into(), format!("{begin:010}")));
            all.push(("$ENDDATA".into(), format!("{end:010}")));
        }
        for (k, v) in all {
            t.push_str(&esc(&k, delim));
            t.push(delim);
            t.push_str(&esc(&v, delim));
            t.push(delim);
        }
        t
    };
    let text_len = render(0, 0).len() as u64;
    let text_begin = 58u64;
    let text_end = text_begin + text_len - 1;
    let data_begin = text_end + 1;
    let data_end = data_begin + data.len() as u64 - 1;
    let text = render(data_begin, data_end);
    let mut out = format!(
        "{version}    {text_begin:>8}{text_end:>8}{data_begin:>8}{data_end:>8}{:>8}{:>8}",
        0, 0
    )
    .into_bytes();
    assert_eq!(out.len(), 58);
    out.extend_from_slice(text.as_bytes());
    out.extend_from_slice(data);
    out
}

fn write(bytes: &[u8]) -> Built {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.fcs");
    std::fs::write(&path, bytes).unwrap();
    Built { _dir: dir, path }
}

fn common(par: u32, tot: u64, dt: &str, byteord: &str) -> Vec<(&'static str, String)> {
    vec![
        ("$BYTEORD", byteord.into()),
        ("$DATATYPE", dt.into()),
        ("$MODE", "L".into()),
        ("$PAR", par.to_string()),
        ("$TOT", tot.to_string()),
        ("$NEXTDATA", "0".into()),
        ("$BEGINANALYSIS", "0".into()),
        ("$ENDANALYSIS", "0".into()),
        ("$BEGINSTEXT", "0".into()),
        ("$ENDSTEXT", "0".into()),
    ]
}

fn open(b: &Built) -> Box<dyn Dataset> {
    FcsReader.open(&b.path).unwrap()
}

#[test]
fn integer_masks_spillover_escapes_and_crc() {
    let mut kw = common(2, 3, "I", "1,2,3,4");
    kw.extend([
        ("$P1N", "FSC-A".to_string()),
        ("$P1S", "forward/scatter".to_string()),
        ("$P1B", "16".into()),
        ("$P1R", "1024".into()),
        ("$P1E", "0,0".into()),
        ("$P1V", "450.5".into()),
        ("$P1L", "488,561".into()),
        ("$P2N", "FL1-A".into()),
        ("$P2B", "16".into()),
        ("$P2R", "65536".into()),
        ("$P2E", "4,1".into()),
        ("$P2G", "2".into()),
        ("$SPILLOVER", "2,FSC-A,FL1-A,1,0.1,0.03,1".into()),
        ("$CYT", "TestCytometer".into()),
        ("$DATE", "05-MAR-2024".into()),
        ("$BTIM", "23:59:58.50".into()),
        ("$ETIM", "00:00:03".into()),
        ("CREATOR", "SynthWriter 1.0".into()),
        ("LASER1NAME", "Blue".into()),
        ("P1DISPLAY", "LIN".into()),
    ]);
    // three events; parameter 1 has junk in bits above 1023
    let mut data = Vec::new();
    for (a, b) in [(0xFC05u16, 7u16), (1023, 65535), (0x0400, 0)] {
        data.extend_from_slice(&a.to_le_bytes());
        data.extend_from_slice(&b.to_le_bytes());
    }
    let mut bytes = data_set("FCS3.1", &kw, &data, '/');
    let mut crc = Crc16::new();
    crc.update(&bytes);
    bytes.extend_from_slice(format!("{:08}", crc.finish()).as_bytes());
    let b = write(&bytes);
    let mut ds = open(&b);
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("3.1"));
    let t = &info.tables[0];
    assert_eq!(t.row_count, 3);
    assert_eq!(t.columns[0].label.as_deref(), Some("forward/scatter"));
    assert_eq!(t.columns[0].dtype, "uint16");
    assert_eq!(t.columns[0].range, Some([0.0, 1023.0]));
    assert_eq!(t.columns[0].extra["bit_mask"], 1023);
    assert_eq!(t.columns[0].extra["detector_voltage"], 450.5);
    assert_eq!(
        t.columns[0].extra["excitation_wavelength_nm"],
        serde_json::json!([488.0, 561.0])
    );
    assert_eq!(t.columns[1].extra["amplification"]["decades"], 4.0);
    assert_eq!(t.columns[1].extra["gain"], 2.0);
    assert_eq!(t.extra["instrument"]["model"], "TestCytometer");
    assert_eq!(t.extra["software"], "SynthWriter 1.0");
    assert_eq!(t.extra["acquisition_start"], "2024-03-05T23:59:58.50");
    assert_eq!(t.extra["acquisition_end"], "2024-03-06T00:00:03");
    assert_eq!(
        t.extra["spillover"]["matrix"],
        serde_json::json!([[1.0, 0.1], [0.03, 1.0]])
    );
    assert_eq!(t.extra["vendor_keywords"]["LASERn"]["LASER1NAME"], "Blue");
    assert_eq!(t.extra["vendor_keywords"]["Pn"]["P1DISPLAY"], "LIN");
    let tab = ds.read_table(0, 0, 10).unwrap();
    assert_eq!(tab.columns[0], vec![5.0, 1023.0, 0.0]);
    assert_eq!(tab.columns[1], vec![7.0, 65535.0, 0.0]);
    let tail = ds.read_table(0, 2, 10).unwrap();
    assert_eq!(tail.columns[0], vec![0.0]);
    let r = ds.check().unwrap();
    assert!(r.ok, "{:?}", r.findings);
    assert!(
        r.findings.iter().any(|f| f.code == "crc_ok"),
        "{:?}",
        r.findings
    );
    // corrupt the stored CRC: reported, but only as a warning
    let n = bytes.len();
    bytes[n - 8..].copy_from_slice(b"00000001");
    let b2 = write(&bytes);
    let r = open(&b2).check().unwrap();
    assert!(
        r.findings
            .iter()
            .any(|f| f.code == "crc_mismatch" && f.severity == Severity::Warning)
    );
    let entries = ds.entries().unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["HEADER", "TEXT", "DATA", "CRC"]);
}

#[test]
fn big_endian_double_and_truncation() {
    let mut kw = common(1, 2, "D", "4,3,2,1");
    kw.extend([
        ("$P1N", "Time".to_string()),
        ("$P1B", "64".into()),
        ("$P1R", "1000".into()),
        ("$P1E", "0,0".into()),
    ]);
    let mut data = Vec::new();
    data.extend_from_slice(&1.25f64.to_be_bytes());
    data.extend_from_slice(&(-3.5f64).to_be_bytes());
    let bytes = data_set("FCS3.0", &kw, &data, '\\');
    let b = write(&bytes);
    let mut ds = open(&b);
    assert_eq!(ds.read_table(0, 0, 5).unwrap().columns[0], vec![1.25, -3.5]);
    assert!(ds.check().unwrap().ok);
    // cut the last 4 bytes: info still works, check fails with `truncated`, reads are corrupt (exit 4)
    let t = write(&bytes[..bytes.len() - 4]);
    let mut ds = open(&t);
    assert_eq!(ds.info().unwrap().tables[0].row_count, 2);
    let r = ds.check().unwrap();
    assert!(!r.ok);
    assert!(
        r.findings.iter().any(|f| f.code == "truncated"),
        "{:?}",
        r.findings
    );
    let e = ds.read_table(0, 0, 2).unwrap_err();
    assert_eq!(e.exit_code(), 4);
    // the first event is still readable
    assert_eq!(ds.read_table(0, 0, 1).unwrap().columns[0], vec![1.25]);
}

#[test]
fn fcs20_ascii_fixed_and_free_format() {
    let mut kw = common(2, 2, "A", "1,2,3,4");
    kw.extend([
        ("$P1N", "A".to_string()),
        ("$P1B", "4".into()),
        ("$P1R", "10000".into()),
        ("$P2N", "B".into()),
        ("$P2B", "3".into()),
        ("$P2R", "1000".into()),
    ]);
    let b = write(&data_set("FCS2.0", &kw, b"0012 34 999  7", '/'));
    let mut ds = open(&b);
    let t = ds.read_table(0, 0, 2).unwrap();
    assert_eq!(t.columns, vec![vec![12.0, 999.0], vec![34.0, 7.0]]);
    assert_eq!(ds.info().unwrap().tables[0].columns[0].dtype, "float64");

    let mut kw = common(2, 3, "A", "1,2,3,4");
    kw.extend([
        ("$P1N", "A".to_string()),
        ("$P1B", "*".into()),
        ("$P2N", "B".into()),
        ("$P2B", "*".into()),
    ]);
    let b = write(&data_set("FCS3.0", &kw, b"1,3,, ,3\r\n4\t5  6\n", '|'));
    let mut ds = open(&b);
    let t = ds.read_table(0, 1, 5).unwrap();
    assert_eq!(t.columns, vec![vec![3.0, 5.0], vec![4.0, 6.0]]);
}

#[test]
fn chained_data_sets_follow_relative_nextdata() {
    let mk = |tot: u64, v: u8, next: u64| {
        let mut kw = common(1, tot, "I", "1,2,3,4");
        kw.retain(|(k, _)| *k != "$NEXTDATA");
        kw.push(("$NEXTDATA", format!("{next:08}")));
        kw.extend([
            ("$P1N", "X".to_string()),
            ("$P1B", "8".into()),
            ("$P1R", "256".into()),
            ("$P1E", "0,0".into()),
        ]);
        data_set("FCS3.1", &kw, &vec![v; tot as usize], '/')
    };
    let first_len = mk(2, 1, 0).len() as u64;
    let mut bytes = mk(2, 1, first_len);
    bytes.extend(mk(3, 9, 0));
    let b = write(&bytes);
    let mut ds = open(&b);
    let info = ds.info().unwrap();
    assert_eq!(info.tables.len(), 2);
    assert_eq!(info.tables[1].row_count, 3);
    assert_eq!(info.tables[1].extra["data_set_offset"], first_len);
    assert_eq!(ds.read_table(1, 0, 10).unwrap().columns[0], vec![9.0; 3]);
    assert!(matches!(ds.read_table(2, 0, 1), Err(Error::Usage(_))));
}

#[test]
fn unsupported_features_exit_6() {
    let base = |mode: &str, bits: &str| {
        let mut kw = common(1, 4, "I", "1,2,3,4");
        kw.retain(|(k, _)| *k != "$MODE");
        kw.push(("$MODE", mode.to_string()));
        kw.extend([
            ("$P1N", "X".to_string()),
            ("$P1B", bits.to_string()),
            ("$P1R", "1024".into()),
            ("$P1E", "0,0".into()),
        ]);
        write(&data_set("FCS3.0", &kw, &[0u8; 8], '/'))
    };
    let hist = base("U", "16");
    let e = open(&hist).read_table(0, 0, 1).unwrap_err();
    assert_eq!(e.exit_code(), 6, "{e}");
    let packed = base("L", "10");
    let e = open(&packed).read_table(0, 0, 1).unwrap_err();
    assert_eq!(e.exit_code(), 6, "{e}");
    // unknown version: detected, but opening is an unsupported feature
    let mut v1 = std::fs::read(&hist.path).unwrap();
    v1[..6].copy_from_slice(b"FCS1.0");
    let old = write(&v1);
    let e = FcsReader.open(&old.path).map(|_| ()).unwrap_err();
    assert_eq!(e.exit_code(), 6);
}

#[test]
fn comp_matrix_without_names_and_offset_off_by_one() {
    let mut kw = common(2, 1, "F", "1,2,3,4");
    kw.extend([
        ("$P1N", "FL1".to_string()),
        ("$P1B", "32".into()),
        ("$P1R", "262144".into()),
        ("$P1E", "0,0".into()),
        ("$P2N", "FL2".into()),
        ("$P2B", "32".into()),
        ("$P2R", "262144".into()),
        ("$P2E", "0,0".into()),
        ("$COMP", "2,1,0.5,0.25,1".into()),
    ]);
    let mut data = Vec::new();
    data.extend_from_slice(&0.1f32.to_le_bytes());
    data.extend_from_slice(&2.0f32.to_le_bytes());
    data.push(b' '); // writer claimed one extra byte
    let mut bytes = data_set("FCS3.0", &kw, &data, '/');
    bytes.extend_from_slice(b"00000000");
    let b = write(&bytes);
    let mut ds = open(&b);
    let info = ds.info().unwrap();
    assert_eq!(info.tables[0].extra["spillover"]["keyword"], "$COMP");
    assert_eq!(info.tables[0].columns[0].dtype, "float32");
    assert_eq!(
        ds.read_table(0, 0, 1).unwrap().columns,
        vec![vec![f64::from(0.1f32)], vec![2.0]]
    );
    let r = ds.check().unwrap();
    assert!(r.ok);
    let codes: Vec<&str> = r.findings.iter().map(|f| f.code.as_str()).collect();
    assert!(codes.contains(&"data_end_off_by_one"), "{codes:?}");
}

#[test]
fn garbage_is_rejected_cleanly() {
    for bytes in [
        &b""[..],
        b"FCS3.1",
        b"FCS3.1    garbage garbage garbage garbage garbage garbage",
        b"oi21j0\n\n\n\n",
    ] {
        let b = write(bytes);
        let e = FcsReader.open(&b.path).map(|_| ()).unwrap_err();
        assert_eq!(e.exit_code(), 4, "{bytes:?}: {e}");
    }
    // TEXT range past the end of the file
    let b = write(b"FCS3.1         256    9999       0       0       0       0");
    assert_eq!(
        FcsReader.open(&b.path).map(|_| ()).unwrap_err().exit_code(),
        4
    );
}
