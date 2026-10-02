//! Synthetic Bruker experiment directories and JCAMP-DX files: layouts the public corpus does not
//! cover (2rr submatrices, truncation, unsupported sample types, path resolution) and exact values.
#![allow(clippy::float_cmp, clippy::many_single_char_names)]

use std::fs;
use std::path::{Path, PathBuf};

use openreadout_core::model::Severity;
use openreadout_core::{Error, Registry};
use openreadout_nmr::{BrukerReader, JcampReader};

fn registry() -> Registry {
    Registry::new()
        .with(Box::new(BrukerReader))
        .with(Box::new(JcampReader))
}

fn params(pairs: &[(&str, &str)]) -> String {
    let mut s = String::from(
        "##TITLE= Parameter file, TopSpin 4.1.0\n##JCAMPDX= 5.0\n##DATATYPE= Parameter Values\n",
    );
    for (k, v) in pairs {
        s.push_str(&format!("##${k}= {v}\n"));
    }
    s.push_str("##END=\n");
    s
}

fn be_i32(vals: &[i32]) -> Vec<u8> {
    vals.iter().flat_map(|v| v.to_be_bytes()).collect()
}

fn le_i32(vals: &[i32]) -> Vec<u8> {
    vals.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// A 1D experiment: TD 8 big-endian int32 complex values, NC -2; pdata/1 with SI 4, NC_proc 1.
fn one_d(root: &Path) -> PathBuf {
    let exp = root.join("study").join("1");
    fs::create_dir_all(exp.join("pdata/1")).unwrap();
    fs::write(
        exp.join("acqus"),
        params(&[
            ("TD", "8"),
            ("DTYPA", "0"),
            ("BYTORDA", "1"),
            ("AQ_mod", "3"),
            ("NC", "-2"),
            ("SW_h", "1000"),
            ("NUC1", "<1H>"),
            ("SFO1", "400.13"),
            ("NS", "16"),
            ("PULPROG", "<zg30>"),
            ("SOLVENT", "<CDCl3>"),
            ("TE", "298.1"),
            ("DATE", "1700000000"),
            ("GRPDLY", "-1"),
            ("DSPFVS", "10"),
            ("DECIM", "6"),
            ("PROBHD", "<5 mm PABBO BB/\n19F-1H/D Z-GRD>"),
        ]),
    )
    .unwrap();
    fs::write(exp.join("fid"), be_i32(&[4, -4, 8, 12, 16, 20, -24, 28])).unwrap();
    fs::write(
        exp.join("pdata/1/procs"),
        params(&[
            ("SI", "4"),
            ("NC_proc", "1"),
            ("OFFSET", "10"),
            ("SW_p", "400"),
            ("SF", "100"),
            ("BYTORDP", "0"),
            ("DTYPP", "0"),
            ("AXNUC", "<1H>"),
        ]),
    )
    .unwrap();
    fs::write(exp.join("pdata/1/1r"), le_i32(&[1, 2, 3, 4])).unwrap();
    fs::write(exp.join("pdata/1/1i"), le_i32(&[-1, -2, -3, -4])).unwrap();
    exp
}

#[test]
fn bruker_1d_values_axis_and_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let exp = one_d(dir.path());
    let reg = registry();
    let (det, mut ds) = reg.open(&exp).unwrap();
    assert_eq!(det.format_id, "bruker-nmr");
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("TopSpin 4.1.0"));
    assert_eq!(info.traces.len(), 2);
    let fid = &info.traces[0];
    assert_eq!((fid.sample_count, fid.sweep_count), (4, 1));
    assert_eq!(fid.sample_rate_hz, 1000.0);
    assert_eq!(fid.channels.len(), 2);
    assert_eq!(fid.channels[0].scale, 0.25);
    assert_eq!(fid.extra["nucleus"], "1H");
    assert_eq!(fid.extra["pulse_program"], "zg30");
    assert_eq!(fid.extra["acquired_at"], "2023-11-14T22:13:20.000Z");
    assert_eq!(fid.extra["probe"], "5 mm PABBO BB/\n19F-1H/D Z-GRD");
    assert_eq!(fid.extra["group_delay_source"], "DSPFVS/DECIM table");
    assert_eq!(fid.extra["byte_order"], "big-endian");
    let t = ds.read_trace(0, 0, 0, 100).unwrap();
    assert_eq!(t.channels[0], vec![1.0, 2.0, 4.0, -6.0]);
    assert_eq!(t.channels[1], vec![-1.0, 3.0, 5.0, 7.0]);
    let part = ds.read_trace(0, 0, 1, 2).unwrap();
    assert_eq!(part.channels[0], vec![2.0, 4.0]);
    // processed: ×2^NC_proc, ppm axis from OFFSET/SW_p/SF/SI
    let spec = &info.traces[1];
    assert_eq!(spec.name.as_deref(), Some("pdata/1"));
    assert_eq!(spec.extra["axis"]["first"], 10.0);
    assert_eq!(spec.extra["axis"]["step"], -1.0);
    assert_eq!(spec.extra["axis"]["last"], 7.0);
    let p = ds.read_trace(1, 0, 0, 10).unwrap();
    assert_eq!(
        p.channels,
        vec![vec![2.0, 4.0, 6.0, 8.0], vec![-2.0, -4.0, -6.0, -8.0]]
    );
    let report = ds.check().unwrap();
    assert!(report.ok, "{:?}", report.findings);
    // a sweep out of range is a usage error
    assert!(matches!(ds.read_trace(0, 1, 0, 1), Err(Error::Usage(_))));
    // the listing names every file
    let names: Vec<String> = ds.entries().unwrap().into_iter().map(|e| e.name).collect();
    assert!(names.contains(&"fid".to_string()) && names.contains(&"pdata/1/1r".to_string()));
}

#[test]
fn bruker_paths_resolve_to_the_experiment() {
    let dir = tempfile::tempdir().unwrap();
    let exp = one_d(dir.path());
    let reg = registry();
    for p in [
        exp.join("fid"),
        exp.join("acqus"),
        exp.join("pdata"),
        exp.join("pdata/1"),
        exp.join("pdata/1/1r"),
        exp.join("pdata/1/procs"),
    ] {
        let (det, ds) = reg
            .open(&p)
            .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        assert_eq!(det.format_id, "bruker-nmr");
        assert_eq!(ds.info().unwrap().traces.len(), 2, "{}", p.display());
    }
    // the study directory above the experiments: a usage error naming them
    match reg.open(exp.parent().unwrap()) {
        Err(Error::Usage(m)) => assert!(m.contains("1 experiments"), "{m}"),
        Err(e) => panic!("unexpected error {e}"),
        Ok(_) => panic!("a study directory must not open as one experiment"),
    }
}

#[test]
fn bruker_ser_rows_are_1024_byte_aligned() {
    let dir = tempfile::tempdir().unwrap();
    let exp = dir.path().join("2");
    fs::create_dir_all(&exp).unwrap();
    fs::write(
        exp.join("acqus"),
        params(&[
            ("TD", "10"),
            ("DTYPA", "2"),
            ("AQ_mod", "3"),
            ("NC", "0"),
            ("SW_h", "5000"),
        ]),
    )
    .unwrap();
    fs::write(
        exp.join("acqu2s"),
        params(&[("TD", "3"), ("NUC1", "<13C>"), ("FnMODE", "6")]),
    )
    .unwrap();
    let mut ser = Vec::new();
    for row in 0..3u32 {
        let mut r: Vec<u8> = (0..10)
            .flat_map(|i| (f64::from(row) * 100.0 + f64::from(i)).to_le_bytes())
            .collect();
        r.resize(1024, 0);
        ser.extend(r);
    }
    fs::write(exp.join("ser"), &ser).unwrap();
    let reg = registry();
    let (_, mut ds) = reg.open(&exp).unwrap();
    let info = ds.info().unwrap();
    let t = &info.traces[0];
    assert_eq!((t.sample_count, t.sweep_count), (5, 3));
    assert_eq!(t.extra["row_stride_bytes"], 1024);
    assert_eq!(
        t.extra["indirect_dimensions"][0]["encoding"],
        "Echo-Antiecho"
    );
    let r = ds.read_trace(0, 2, 0, 5).unwrap();
    assert_eq!(r.channels[0], vec![200.0, 202.0, 204.0, 206.0, 208.0]);
    assert_eq!(r.channels[1], vec![201.0, 203.0, 205.0, 207.0, 209.0]);
    assert!(ds.check().unwrap().ok);
    // cut the last row short: check reports truncation, reading it is a corrupt-file error
    fs::write(exp.join("ser"), &ser[..2048 + 40]).unwrap();
    let (_, mut ds) = reg.open(&exp).unwrap();
    let report = ds.check().unwrap();
    assert!(!report.ok);
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.code == "truncated" && f.severity == Severity::Error)
    );
    assert!(matches!(
        ds.read_trace(0, 2, 0, 5),
        Err(Error::Corrupt { .. })
    ));
    assert!(ds.read_trace(0, 1, 0, 5).is_ok());
}

#[test]
fn bruker_truncated_fid_and_unknown_sample_type() {
    let dir = tempfile::tempdir().unwrap();
    let exp = one_d(dir.path());
    fs::write(exp.join("fid"), be_i32(&[1, 2, 3])).unwrap();
    let reg = registry();
    let (_, mut ds) = reg.open(&exp).unwrap();
    let report = ds.check().unwrap();
    assert!(!report.ok && report.findings.iter().any(|f| f.code == "truncated"));
    let e = ds.read_trace(0, 0, 0, 4).unwrap_err();
    assert_eq!(e.exit_code(), 4);
    // DTYPA 1 is not a known sample type: info works, reading is unsupported (exit 6)
    let acqus = fs::read_to_string(exp.join("acqus"))
        .unwrap()
        .replace("##$DTYPA= 0", "##$DTYPA= 1");
    fs::write(exp.join("acqus"), acqus).unwrap();
    let (_, mut ds) = reg.open(&exp).unwrap();
    assert!(ds.info().unwrap().notes.iter().any(|n| n.contains("DTYPA")));
    assert_eq!(ds.read_trace(0, 0, 0, 4).unwrap_err().exit_code(), 6);
}

#[test]
fn bruker_2rr_submatrices() {
    let dir = tempfile::tempdir().unwrap();
    let exp = dir.path().join("3");
    fs::create_dir_all(exp.join("pdata/1")).unwrap();
    fs::write(
        exp.join("acqus"),
        params(&[("TD", "4"), ("AQ_mod", "3"), ("SW_h", "100")]),
    )
    .unwrap();
    fs::write(exp.join("acqu2s"), params(&[("TD", "2")])).unwrap();
    fs::write(exp.join("ser"), vec![0u8; 2048]).unwrap();
    let p = exp.join("pdata/1");
    fs::write(
        p.join("procs"),
        params(&[
            ("SI", "4"),
            ("XDIM", "2"),
            ("NC_proc", "0"),
            ("OFFSET", "5"),
            ("SW_p", "8"),
            ("SF", "2"),
        ]),
    )
    .unwrap();
    fs::write(
        p.join("proc2s"),
        params(&[
            ("SI", "4"),
            ("XDIM", "2"),
            ("OFFSET", "100"),
            ("SW_p", "40"),
            ("SF", "10"),
        ]),
    )
    .unwrap();
    // logical 4x4 matrix m[r][c] = 10*r + c stored as four 2x2 submatrices, row-major
    let mut stored = Vec::new();
    for (br, bc) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
        for r in 0..2 {
            for c in 0..2 {
                stored.push(10 * (2 * br + r) + (2 * bc + c));
            }
        }
    }
    fs::write(p.join("2rr"), le_i32(&stored)).unwrap();
    let reg = registry();
    let (_, mut ds) = reg.open(&exp).unwrap();
    let info = ds.info().unwrap();
    let t = &info.traces[1];
    assert_eq!((t.sample_count, t.sweep_count), (4, 4));
    assert_eq!(t.extra["submatrix"], serde_json::json!([2, 2]));
    assert_eq!(t.extra["sweep_axis"]["axis"]["first"], 100.0);
    for r in 0..4u32 {
        let row = ds.read_trace(1, r, 0, 4).unwrap();
        let want: Vec<f64> = (0..4).map(|c| f64::from(10 * r + c)).collect();
        assert_eq!(row.channels[0], want, "row {r}");
    }
    assert_eq!(
        ds.read_trace(1, 3, 1, 2).unwrap().channels[0],
        vec![31.0, 32.0]
    );
    assert!(ds.check().unwrap().ok);
}

const JDX: &str = "##TITLE= demo\n##JCAMP-DX= 4.24\n##DATA TYPE= INFRARED SPECTRUM\n##XUNITS= 1/CM\n##YUNITS= ABSORBANCE\n##XFACTOR= 1.0\n##YFACTOR= 0.001\n##FIRSTX= 599.860\n##LASTX= 700.158\n##NPOINTS= 53\n##FIRSTY= 0\n##XYDATA= (X++(Y..Y))\n599.860@VKT%TLkj%J%KLJ%njKjL%kL%jJULJ%kLK1%lLMNPNPRLJ0QTOJ1P\n700.158A28 $$ checkpoint\n##END=\n";

#[test]
fn jcamp_difdup_table_and_axis() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("demo.jdx");
    fs::write(&p, JDX).unwrap();
    let reg = registry();
    let (det, mut ds) = reg.open(&p).unwrap();
    assert_eq!(det.format_id, "jcamp-dx");
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("4.24"));
    let t = &info.traces[0];
    assert_eq!(t.sample_count, 53);
    assert_eq!(t.channels[0].scale, 0.001);
    assert_eq!(t.extra["axis"]["quantity"], "wavenumber");
    let v = ds.read_trace(0, 0, 0, 100).unwrap();
    assert_eq!(v.channels[0].len(), 53);
    assert_eq!(v.channels[0][4], 0.002);
    assert_eq!(*v.channels[0].last().unwrap(), 0.128);
    assert!(ds.check().unwrap().ok);
}

#[test]
fn jcamp_ntuples_peak_table_and_truncation() {
    let nt = "##TITLE= fid\n##JCAMP-DX= 5.00\n##DATA TYPE= NMR FID\n##DATA CLASS= NTUPLES\n##.OBSERVE FREQUENCY= 100.4\n##NTUPLES= NMR FID\n##VAR_NAME= TIME, FID/REAL, FID/IMAG, PAGE NUMBER\n##SYMBOL= X, R, I, N\n##VAR_TYPE= INDEPENDENT, DEPENDENT, DEPENDENT, PAGE\n##VAR_FORM= AFFN, ASDF, ASDF, AFFN\n##VAR_DIM= 4, 4, 4, 2\n##UNITS= SECONDS, ARBITRARY UNITS, ARBITRARY UNITS,\n##FIRST= 0.0, 1, 5,\n##LAST= 0.3, 4, 8,\n##FACTOR= 0.1, 2, 1,\n##PAGE= N=1\n##DATA TABLE= (X++(R..R)), XYDATA\n0 1 2 3 4\n##PAGE= N=2\n##DATA TABLE= (X++(I..I)), XYDATA\n0 5 6 7 8\n##END NTUPLES= NMR FID\n##END=\n";
    let pk = "##TITLE= peaks\n##JCAMP-DX= 5.00\n##DATA TYPE= MASS SPECTRUM\n##DATA CLASS= PEAK TABLE\n##XUNITS= M/Z\n##YUNITS= RELATIVE ABUNDANCE\n##NPOINTS= 3\n##PEAK TABLE= (XY..XY)\n50, 5.84\n51, 9.55; 52,4.19\n##END=\n";
    let dir = tempfile::tempdir().unwrap();
    let reg = registry();
    let a = dir.path().join("fid.dx");
    fs::write(&a, nt).unwrap();
    let (_, mut ds) = reg.open(&a).unwrap();
    let info = ds.info().unwrap();
    let t = &info.traces[0];
    assert_eq!(
        t.channels
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["FID/REAL", "FID/IMAG"]
    );
    assert!((t.sample_rate_hz - 10.0).abs() < 1e-9);
    let v = ds.read_trace(0, 0, 0, 10).unwrap();
    assert_eq!(
        v.channels,
        vec![vec![2.0, 4.0, 6.0, 8.0], vec![5.0, 6.0, 7.0, 8.0]]
    );
    let b = dir.path().join("peaks.jdx");
    fs::write(&b, pk).unwrap();
    let (_, mut ds) = reg.open(&b).unwrap();
    let v = ds.read_trace(0, 0, 0, 10).unwrap();
    assert_eq!(
        v.channels,
        vec![vec![50.0, 51.0, 52.0], vec![5.84, 9.55, 4.19]]
    );
    assert!(ds.check().unwrap().ok);
    // cut inside the table: no ##END=, fewer points than declared
    let c = dir.path().join("cut.jdx");
    fs::write(&c, &JDX[..JDX.find("700.158").unwrap()]).unwrap();
    let (_, mut ds) = reg.open(&c).unwrap();
    let report = ds.check().unwrap();
    assert!(!report.ok);
    assert!(report.findings.iter().any(|f| f.code == "truncated"));
}

#[test]
fn jcamp_garbage_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("x.jdx");
    fs::write(&p, "not jcamp at all\n").unwrap();
    // extension-only detection, then no ##TITLE= block: a corrupt-file error (exit 4)
    match registry().open(&p) {
        Err(e) => assert_eq!(e.exit_code(), 4, "{e}"),
        Ok(_) => panic!("garbage opened as JCAMP-DX"),
    }
}
