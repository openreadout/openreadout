//! FCS 3.2: mixed data types per measurement (`$PnDATATYPE`), no `$MODE`, ISO date-times and
//! the new measurement keywords.

use openreadout_core::reader::FormatReader;
use openreadout_fcs::FcsReader;

#[test]
fn mixed_data_types_and_new_keywords() {
    // Time as 16-bit integer, FL1-A as float, FL2-A as double; little-endian.
    let kws: Vec<(&str, &str)> = vec![
        ("$BYTEORD", "1,2,3,4"),
        ("$DATATYPE", "F"),
        ("$PAR", "3"),
        ("$TOT", "2"),
        ("$NEXTDATA", "0"),
        ("$CYT", "Synthetic"),
        ("$BEGINDATETIME", "2025-03-07T19:58:46Z"),
        ("$ENDDATETIME", "2025-03-07T20:01:26Z"),
        ("$CARRIERTYPE", "96 U-Bottom"),
        ("$LOCATIONID", "B4"),
        ("$P1N", "Time"),
        ("$P1B", "16"),
        ("$P1E", "0,0"),
        ("$P1R", "65536"),
        ("$P1DATATYPE", "I"),
        ("$P1TYPE", "Time"),
        ("$P2N", "FL1-A"),
        ("$P2B", "32"),
        ("$P2E", "0,0"),
        ("$P2R", "262144"),
        ("$P2DET", "B1 (515)"),
        ("$P2FEATURE", "Area"),
        ("$P2TAG", "FITC"),
        ("$P2ANALYTE", "CD3"),
        ("$P3N", "FL2-A"),
        ("$P3B", "64"),
        ("$P3E", "0,0"),
        ("$P3R", "262144"),
        ("$P3DATATYPE", "D"),
    ];
    let mut data = Vec::new();
    for (t, a, b) in [(7u16, 1.5f32, 2.25f64), (9, -3.0, 1e10)] {
        data.extend_from_slice(&t.to_le_bytes());
        data.extend_from_slice(&a.to_le_bytes());
        data.extend_from_slice(&b.to_le_bytes());
    }
    let render = |begin: u64, end: u64| {
        let mut s = String::from("|");
        for (k, v) in kws
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .chain([
                ("$BEGINDATA".to_string(), format!("{begin:010}")),
                ("$ENDDATA".to_string(), format!("{end:010}")),
            ])
        {
            s.push_str(&k);
            s.push('|');
            s.push_str(&v);
            s.push('|');
        }
        s
    };
    let len = render(0, 0).len() as u64;
    let (tb, te) = (58u64, 57 + len);
    let (db, de) = (te + 1, te + data.len() as u64);
    let mut out = format!("FCS3.2    {tb:>8}{te:>8}{db:>8}{de:>8}{:>8}{:>8}", 0, 0).into_bytes();
    out.extend_from_slice(render(db, de).as_bytes());
    out.extend_from_slice(&data);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mixed.fcs");
    std::fs::write(&path, out).unwrap();

    let mut ds = FcsReader.open(&path).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("3.2"));
    let t = &info.tables[0];
    assert_eq!(t.columns[0].dtype, "uint16");
    assert_eq!(t.columns[1].dtype, "float32");
    assert_eq!(t.columns[2].dtype, "float64");
    assert_eq!(t.extra["acquisition_start"], "2025-03-07T19:58:46Z");
    assert_eq!(t.extra["location_id"], "B4");
    assert_eq!(t.columns[1].extra["detector"], "B1 (515)");
    assert_eq!(t.columns[1].extra["dye"], "FITC");
    assert_eq!(t.columns[1].extra["analyte"], "CD3");
    assert_eq!(t.columns[0].extra["parameter_type"], "Time");
    let rows = ds.read_table(0, 0, 2).unwrap();
    assert_eq!(rows.columns[0], vec![7.0, 9.0]);
    assert_eq!(rows.columns[1], vec![1.5, -3.0]);
    assert_eq!(rows.columns[2], vec![2.25, 1e10]);
    // No $MODE and no segment keywords: not an error in FCS 3.2.
    let report = ds.check().unwrap();
    assert!(
        !report.findings.iter().any(|f| f.code == "missing_keyword"),
        "{:?}",
        report.findings
    );
}
