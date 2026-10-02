//! Synthetic BES3T and ESP/WinEPR data sets: layouts read back exactly, and malformed files end in
//! clean errors (never a panic).

use std::path::Path;

use openreadout_core::{Error, FormatReader};
use openreadout_epr::{Bes3tReader, EspReader};

fn dsc(extra: &str, xpts: usize, ypts: usize, ikkf: &str, fmt: &str, bseq: &str) -> String {
    format!(
        "#DESC\t1.2 * DESCRIPTOR INFORMATION\nDSRC\tEXP\nBSEQ\t{bseq}\nIKKF\t{ikkf}\nXTYP\tIDX\nYTYP\t{}\nZTYP\tNODATA\nIRFMT\t{fmt}\nXPTS\t{xpts}\n{}XMIN\t3300.0\nXWID\t100.0\nTITL\t'test'\nXNAM\t'Field'\nXUNI\t'G'\n{extra}#SPL\t1.2\nOPER\txuser\nDATE\t11/18/21\nTIME\t16:22:52\nEXPT\tCW\nMWFQ\t9.4e9\nMWPW\t0.002\nB0MA\t0.0001\nAVGS\t4\n#DSL\t1.0\n.DVC\tacqStart, 1.0\n#MHL\t1.0\nsomething\n",
        if ypts > 1 { "IDX" } else { "NODATA" },
        if ypts > 1 {
            format!("YPTS\t{ypts}\nYMIN\t0\nYWID\t10\nYNAM\t'Power'\nYUNI\t'mW'\n")
        } else {
            String::new()
        }
    )
}

fn write(dir: &Path, name: &str, data: &[u8]) {
    std::fs::write(dir.join(name), data).unwrap();
}

#[test]
fn bes3t_real_and_complex_2d() {
    let dir = tempfile::tempdir().unwrap();
    // 1D real, big-endian float64
    write(
        dir.path(),
        "a.DSC",
        dsc("", 3, 1, "REAL", "D", "BIG").as_bytes(),
    );
    let v: Vec<u8> = [1.5f64, -2.0, 4.25]
        .iter()
        .flat_map(|x| x.to_be_bytes())
        .collect();
    write(dir.path(), "a.DTA", &v);
    for member in ["a.DSC", "a.DTA"] {
        let mut ds = Bes3tReader.open(&dir.path().join(member)).unwrap();
        let info = ds.info().unwrap();
        assert_eq!(info.traces[0].channels[0].name, "intensity");
        let t = ds.read_trace(0, 0, 0, u64::MAX).unwrap();
        assert_eq!(t.channels[0], vec![1.5, -2.0, 4.25]);
        assert_eq!(info.traces[0].extra["axis"]["step"], 50.0);
        let exp = ds.experiment().unwrap();
        let params = &exp.method.as_ref().unwrap().parameters;
        assert_eq!(params["microwave_frequency"].value, 9.4);
        assert_eq!(params["modulation_amplitude"].value, 1);
        assert_eq!(
            exp.acquisition.as_ref().unwrap().started_at.as_deref(),
            Some("2021-11-18T16:22:52")
        );
        assert_eq!(ds.member_files().len(), 2);
    }
    // 2D complex, little-endian float32: 2 x-points, 2 sweeps
    write(
        dir.path(),
        "b.DSC",
        dsc("", 2, 2, "CPLX", "F", "LIT").as_bytes(),
    );
    let v: Vec<u8> = [1f32, 10., 2., 20., 3., 30., 4., 40.]
        .iter()
        .flat_map(|x| x.to_le_bytes())
        .collect();
    write(dir.path(), "b.DTA", &v);
    let mut ds = Bes3tReader.open(&dir.path().join("b.DSC")).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces[0].sweep_count, 2);
    let t = ds.read_trace(0, 1, 0, u64::MAX).unwrap();
    assert_eq!(t.channels, vec![vec![3.0, 4.0], vec![30.0, 40.0]]);
    let tab = ds.read_table(0, 0, 10).unwrap();
    assert_eq!(tab.columns[1], vec![0.0, 10.0]);
}

#[test]
fn bes3t_malformed() {
    let dir = tempfile::tempdir().unwrap();
    // data shorter than described: corrupt
    write(
        dir.path(),
        "c.DSC",
        dsc("", 4, 1, "REAL", "D", "BIG").as_bytes(),
    );
    write(dir.path(), "c.DTA", &[0u8; 8]);
    assert!(matches!(
        Bes3tReader.open(&dir.path().join("c.DSC")),
        Err(Error::Corrupt { .. })
    ));
    // ASCII data: refused
    write(
        dir.path(),
        "e.DSC",
        dsc("", 1, 1, "REAL", "A", "BIG").as_bytes(),
    );
    write(dir.path(), "e.DTA", b"1.0");
    assert!(matches!(
        Bes3tReader.open(&dir.path().join("e.DSC")),
        Err(Error::Unsupported { .. })
    ));
    // missing data file: an error, not a panic
    write(
        dir.path(),
        "f.DSC",
        dsc("", 1, 1, "REAL", "D", "BIG").as_bytes(),
    );
    assert!(Bes3tReader.open(&dir.path().join("f.DSC")).is_err());
    // garbage descriptor
    write(dir.path(), "g.DSC", b"\x00\x01\x02");
    write(dir.path(), "g.DTA", b"");
    assert!(Bes3tReader.open(&dir.path().join("g.DSC")).is_err());
    // absurd point counts do not allocate
    write(
        dir.path(),
        "h.DSC",
        dsc("", 999_999_999_999, 1, "REAL", "D", "BIG").as_bytes(),
    );
    write(dir.path(), "h.DTA", &[0u8; 16]);
    assert!(Bes3tReader.open(&dir.path().join("h.DSC")).is_err());
}

#[test]
fn esp_winepr_and_malformed() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "s.par",
        b"DOS  Format\r\nANZ 4\r\nJSS 0\r\nGST 3300.0\r\nGSI 30.0\r\nRES 4\r\nHCF 3315\r\nMF  9.478\r\nJDA 06/18/2009\r\nJTM 12:28\r\n",
    );
    let v: Vec<u8> = [1f32, 2., 3., 4.]
        .iter()
        .flat_map(|x| x.to_le_bytes())
        .collect();
    write(dir.path(), "s.spc", &v);
    let mut ds = EspReader.open(&dir.path().join("s.spc")).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces[0].extra["axis"]["first"], 3300.0);
    assert_eq!(info.traces[0].extra["axis"]["step"], 10.0);
    let t = ds.read_trace(0, 0, 0, u64::MAX).unwrap();
    assert_eq!(t.channels[0], vec![1.0, 2.0, 3.0, 4.0]);
    // ESP (no DOS key): big-endian int32
    write(dir.path(), "e.par", b"JSS 0\nRES 2\nHCF 3400\nHSW 100\n");
    let v: Vec<u8> = [7i32, -8].iter().flat_map(|x| x.to_be_bytes()).collect();
    write(dir.path(), "e.spc", &v);
    let mut ds = EspReader.open(&dir.path().join("e.par")).unwrap();
    assert_eq!(
        ds.read_trace(0, 0, 0, 9).unwrap().channels[0],
        vec![7.0, -8.0]
    );
    // short data
    write(
        dir.path(),
        "t.par",
        b"DOS  Format\nRES 100\nHCF 3400\nHSW 100\n",
    );
    write(dir.path(), "t.spc", &[0u8; 8]);
    assert!(matches!(
        EspReader.open(&dir.path().join("t.par")),
        Err(Error::Corrupt { .. })
    ));
    // not a parameter file
    write(dir.path(), "u.par", b"hello world\n");
    write(dir.path(), "u.spc", &[0u8; 8]);
    assert!(EspReader.open(&dir.path().join("u.par")).is_err());
}
