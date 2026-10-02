//! Synthetic VnmrJ directories and JEOL `.jdf` files (layouts the public corpus does not cover:
//! int16/int32 samples, several traces per block, truncation, float32 JDF data, unsupported
//! layouts), and the Bruker 3D processed fixture written by nmrglue
//! (`oracle/make_nmr_fixtures.py`).
#![allow(
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::many_single_char_names
)]

use std::fs;
use std::path::{Path, PathBuf};

use openreadout_core::model::Severity;
use openreadout_core::{Error, Registry};
use openreadout_nmr::{BrukerReader, JcampReader, JeolReader, VarianReader};

fn registry() -> Registry {
    Registry::new()
        .with(Box::new(BrukerReader))
        .with(Box::new(JcampReader))
        .with(Box::new(VarianReader))
        .with(Box::new(JeolReader))
}

fn codes(r: &openreadout_core::model::CheckReport, sev: Severity) -> Vec<String> {
    r.findings
        .iter()
        .filter(|f| f.severity == sev)
        .map(|f| f.code.clone())
        .collect()
}

// ------------------------------------------------------------------------------------------
// Varian / Agilent
// ------------------------------------------------------------------------------------------

fn real_param(name: &str, v: &str) -> String {
    format!("{name} 1 1 1e9 0 0 2 1 11 1 64\n1 {v} \n0 \n")
}

fn text_param(name: &str, v: &str) -> String {
    format!("{name} 2 2 8 0 0 2 1 11 1 64\n1 \"{v}\"\n0 \n")
}

/// A `fid` of `blocks` blocks of `traces` traces, each `points` values of `eb` bytes from `val`.
fn fid(
    blocks: i32,
    traces: i32,
    points: i32,
    eb: i32,
    status: i16,
    val: impl Fn(i32, i32, i32) -> i64,
) -> Vec<u8> {
    let mut b = Vec::new();
    let tb = points * eb;
    for v in [blocks, traces, points, eb, tb, traces * tb + 28] {
        b.extend_from_slice(&v.to_be_bytes());
    }
    b.extend_from_slice(&0i16.to_be_bytes());
    b.extend_from_slice(&status.to_be_bytes());
    b.extend_from_slice(&1i32.to_be_bytes());
    for blk in 0..blocks {
        // block header: scale, status, index, mode, ctcount, 4 floats
        b.extend_from_slice(&0i16.to_be_bytes());
        b.extend_from_slice(&status.to_be_bytes());
        b.extend_from_slice(&((blk + 1) as i16).to_be_bytes());
        b.extend_from_slice(&0i16.to_be_bytes());
        b.extend_from_slice(&4i32.to_be_bytes());
        b.extend_from_slice(&[0u8; 16]);
        for t in 0..traces {
            for p in 0..points {
                let v = val(blk, t, p);
                if eb == 2 {
                    b.extend_from_slice(&(v as i16).to_be_bytes());
                } else {
                    b.extend_from_slice(&(v as i32).to_be_bytes());
                }
            }
        }
    }
    b
}

fn varian_dir(root: &Path, name: &str, fid_bytes: &[u8], procpar: Option<&str>) -> PathBuf {
    let d = root.join(name);
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("fid"), fid_bytes).unwrap();
    if let Some(p) = procpar {
        fs::write(d.join("procpar"), p).unwrap();
    }
    fs::write(d.join("text"), "sample 7 in CDCl3\n").unwrap();
    d
}

fn procpar(np: i32, arraydim: i32) -> String {
    [
        real_param("np", &np.to_string()),
        real_param("sw", "8000"),
        real_param("sfrq", "399.8"),
        real_param("nt", "16"),
        real_param("arraydim", &arraydim.to_string()),
        text_param("tn", "H1"),
        text_param("seqfil", "s2pul"),
        text_param("solvent", "cdcl3"),
        text_param("samplename", "S-7"),
        text_param("operator_", "alice"),
        text_param("parver", "VnmrJ VERSION 4.2 REVISION A"),
        text_param("time_run", "20240102T030405"),
    ]
    .concat()
}

#[test]
fn varian_int16_values_axis_and_metadata() {
    let dir = tempfile::tempdir().unwrap();
    // int16 (status: data bit only), 2 blocks x 1 trace x 4 values (2 complex points)
    let bytes = fid(2, 1, 4, 2, 0x1, |b, _, p| {
        i64::from(if p % 2 == 0 {
            -(p + 10 * b)
        } else {
            p + 10 * b
        })
    });
    let d = varian_dir(dir.path(), "one.fid", &bytes, Some(&procpar(4, 2)));
    let reg = registry();
    let (det, mut ds) = reg.open(&d).unwrap();
    assert_eq!(det.format_id, "varian-nmr");
    let info = ds.info().unwrap();
    assert_eq!(
        info.format_version.as_deref(),
        Some("VnmrJ VERSION 4.2 REVISION A")
    );
    let t = &info.traces[0];
    assert_eq!((t.sample_count, t.sweep_count), (2, 2));
    assert_eq!(t.sample_rate_hz, 8000.0);
    assert_eq!(t.channels[0].dtype, "int16");
    assert_eq!(t.extra["nucleus"], "1H");
    assert_eq!(t.extra["pulse_program"], "s2pul");
    assert_eq!(t.extra["acquired_at"], "2024-01-02T03:04:05");
    assert_eq!(t.extra["software"], "VnmrJ");
    let tr = ds.read_trace(0, 1, 0, 10).unwrap();
    assert_eq!(tr.channels[0], vec![-10.0, -12.0]);
    assert_eq!(tr.channels[1], vec![11.0, 13.0]);
    let tail = ds.read_trace(0, 0, 1, 10).unwrap();
    assert_eq!(tail.channels[0], vec![-2.0]);
    let r = ds.check().unwrap();
    assert!(r.ok, "{:?}", r.findings);
    let e = ds.experiment().unwrap();
    assert_eq!(e.sample.as_ref().unwrap().id.as_deref(), Some("S-7"));
    assert_eq!(
        e.sample.as_ref().unwrap().name.as_deref(),
        Some("sample 7 in CDCl3")
    );
    assert_eq!(e.acquisition.unwrap().operator.as_deref(), Some("alice"));
    // paths inside the directory resolve to it
    for leaf in ["fid", "procpar", "text"] {
        let (det, _) = reg.open(&d.join(leaf)).unwrap();
        assert_eq!(det.format_id, "varian-nmr", "{leaf}");
    }
}

#[test]
fn varian_int32_traces_per_block_and_study_folder() {
    let dir = tempfile::tempdir().unwrap();
    // int32 (0x4), 2 blocks x 2 traces x 2 values: sweep = block * 2 + trace
    let bytes = fid(2, 2, 2, 4, 0x5, |b, t, p| {
        i64::from(1000 * b + 100 * t + p) * 100_000
    });
    let d = varian_dir(dir.path(), "two.fid", &bytes, Some(&procpar(2, 4)));
    let reg = registry();
    let (_, mut ds) = reg.open(&d).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces[0].sweep_count, 4);
    assert_eq!(info.traces[0].channels[0].dtype, "int32");
    let tr = ds.read_trace(0, 3, 0, 1).unwrap();
    assert_eq!(tr.channels[0], vec![110_000_000.0]);
    assert_eq!(tr.channels[1], vec![110_100_000.0]);
    assert!(ds.check().unwrap().ok);
    // the folder above several .fid directories is detected but not opened
    let _ = varian_dir(dir.path(), "one.fid", &bytes, Some(&procpar(2, 4)));
    match reg.open(dir.path()) {
        Err(Error::Usage(m)) => assert!(m.contains("VnmrJ data directories"), "{m}"),
        Err(e) => panic!("unexpected error {e}"),
        Ok(_) => panic!("a study folder must not open"),
    }
}

#[test]
fn varian_truncation_mismatch_and_missing_procpar() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = fid(3, 1, 8, 4, 0xc9, |_, _, p| {
        i64::from(f32::to_bits(p as f32))
    });
    // cut inside block 2
    let cut = &bytes[..32 + 60 + 60 + 10];
    let d = varian_dir(dir.path(), "cut.fid", cut, Some(&procpar(6, 3)));
    let reg = registry();
    let (_, mut ds) = reg.open(&d).unwrap();
    let info = ds.info().unwrap();
    assert!(
        info.notes.iter().any(|n| n.contains("2 of 3 blocks")),
        "{:?}",
        info.notes
    );
    assert_eq!(
        ds.read_trace(0, 1, 0, 8).unwrap().channels[0],
        vec![0.0, 2.0, 4.0, 6.0]
    );
    match ds.read_trace(0, 2, 0, 8) {
        Err(e @ Error::Corrupt { .. }) => assert_eq!(e.exit_code(), 4),
        other => panic!("expected a corrupt-file error, got {other:?}"),
    }
    let r = ds.check().unwrap();
    assert!(!r.ok);
    assert!(codes(&r, Severity::Error).contains(&"truncated".to_string()));
    assert!(codes(&r, Severity::Warning).contains(&"np_mismatch".to_string()));
    // fid without procpar: the layout alone
    let bare = varian_dir(dir.path(), "bare.fid", &bytes, None);
    let (_, mut ds) = reg.open(&bare).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces[0].sample_rate_hz, 0.0);
    assert_eq!(info.traces[0].sweep_count, 3);
    assert!(
        codes(&ds.check().unwrap(), Severity::Warning).contains(&"missing_parameters".to_string())
    );
}

#[test]
fn varian_bad_header_is_not_varian() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = fid(1, 1, 4, 4, 0xc9, |_, _, _| 0);
    bytes[23] ^= 0x10; // block size no longer adds up
    let d = varian_dir(dir.path(), "bad.fid", &bytes, Some(&procpar(4, 1)));
    assert!(registry().open(&d).is_err());
}

// ------------------------------------------------------------------------------------------
// JEOL
// ------------------------------------------------------------------------------------------

struct Jdf {
    dims: u8,
    format: u8,
    float32: bool,
    types: [u8; 2],
    points: [u32; 2],
    stop: [u32; 2],
    unit: [(u8, u8); 2],
    axis: [(f64, f64); 2],
    listed_y: Option<Vec<f64>>,
}

fn param(name: &str, vt: i32, scaler: i16, unit: (u8, u8), value: &[u8]) -> Vec<u8> {
    let mut r = vec![0u8; 64];
    r[4..6].copy_from_slice(&scaler.to_le_bytes());
    r[6] = unit.0;
    r[7] = unit.1;
    r[16..16 + value.len()].copy_from_slice(value);
    r[32..36].copy_from_slice(&vt.to_le_bytes());
    let n = name.as_bytes();
    r[36..36 + n.len()].copy_from_slice(n);
    for b in &mut r[36 + n.len()..64] {
        *b = b' ';
    }
    r
}

fn text16(s: &str) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    v.resize(16, b' ');
    v
}

/// A little-endian JDF file; `data` holds every section's values in stored order.
fn jdf(j: &Jdf, data: &[f64]) -> Vec<u8> {
    let mut h = vec![0u8; 1360];
    h[..8].copy_from_slice(b"JEOL.NMR");
    h[8] = 1; // little-endian
    h[9] = 1;
    h[10..12].copy_from_slice(&2u16.to_be_bytes());
    h[12] = j.dims;
    h[14] = (u8::from(j.float32) << 6) | j.format;
    h[15] = 25;
    for k in 0..usize::from(j.dims) {
        h[24 + k] = j.types[k];
        h[32 + 2 * k] = j.unit[k].0;
        h[33 + 2 * k] = j.unit[k].1;
        h[176 + 4 * k..180 + 4 * k].copy_from_slice(&j.points[k].to_be_bytes());
        h[240 + 4 * k..244 + 4 * k].copy_from_slice(&j.stop[k].to_be_bytes());
        h[272 + 8 * k..280 + 8 * k].copy_from_slice(&j.axis[k].0.to_be_bytes());
        h[336 + 8 * k..344 + 8 * k].copy_from_slice(&j.axis[k].1.to_be_bytes());
    }
    h[48..54].copy_from_slice(b"title7");
    h[400..402].copy_from_slice(&((34u16 << 9) | (3 << 5) | 9).to_be_bytes()); // 2024-03-09
    h[552..555].copy_from_slice(b"bob");
    h[808..814].copy_from_slice(b"Proton");
    let params = vec![
        param("X_SWEEP", 2, 0, (1, 13), &5000f64.to_le_bytes()),
        param("X_FREQ", 2, 0, (1, 13), &400_000_000f64.to_le_bytes()),
        param("X_DOMAIN", 0, 0, (0, 0), &text16("Proton")),
        param("SCANS", 1, 0, (0, 0), &8i32.to_le_bytes()),
        param("temp_get", 2, 0, (1, 4), &25f64.to_le_bytes()),
        param("sample_id", 0, 0, (0, 0), &text16("S-9")),
        param(
            "ACTUAL_START_TIME",
            1,
            0,
            (0, 0),
            &1_000_000_000i32.to_le_bytes(),
        ),
    ];
    let mut sec = Vec::new();
    for v in [64u32, 0, params.len() as u32 - 1, 64 * params.len() as u32] {
        sec.extend_from_slice(&v.to_le_bytes());
    }
    for p in &params {
        sec.extend_from_slice(p);
    }
    let param_start = 1360u32;
    let mut list = Vec::new();
    if let Some(ys) = &j.listed_y {
        h[172] = 0x03; // y listed
        for y in ys {
            list.extend_from_slice(&y.to_be_bytes());
        }
    }
    let list_start = param_start + sec.len() as u32;
    let data_start = list_start + list.len() as u32;
    h[1212..1216].copy_from_slice(&param_start.to_be_bytes());
    h[1216..1220].copy_from_slice(&(sec.len() as u32).to_be_bytes());
    for k in 0..8 {
        h[1220 + 4 * k..1224 + 4 * k].copy_from_slice(&list_start.to_be_bytes());
    }
    h[1256..1260].copy_from_slice(&(list.len() as u32).to_be_bytes());
    h[1284..1288].copy_from_slice(&data_start.to_be_bytes());
    let mut body = Vec::new();
    for v in data {
        if j.float32 {
            body.extend_from_slice(&(*v as f32).to_le_bytes());
        } else {
            body.extend_from_slice(&v.to_le_bytes());
        }
    }
    h[1292..1296].copy_from_slice(&(body.len() as u32).to_be_bytes());
    let total = u64::from(data_start) + body.len() as u64;
    h[1324..1328].copy_from_slice(&(total as u32).to_be_bytes());
    [h, sec, list, body].concat()
}

#[test]
fn jeol_1d_float32_values_and_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let j = Jdf {
        dims: 1,
        format: 1,
        float32: true,
        types: [3, 0],
        points: [8, 1],
        stop: [5, 0],
        unit: [(1, 28), (0, 0)],
        axis: [(0.0, 7.0 / 5000.0), (0.0, 0.0)],
        listed_y: None,
    };
    let data: Vec<f64> = (0..16).map(f64::from).collect();
    let p = dir.path().join("one.jdf");
    fs::write(&p, jdf(&j, &data)).unwrap();
    let reg = registry();
    let (det, mut ds) = reg.open(&p).unwrap();
    assert_eq!(det.format_id, "jeol-jdf");
    let info = ds.info().unwrap();
    let t = &info.traces[0];
    assert_eq!((t.sample_count, t.sweep_count), (6, 1));
    assert_eq!(t.sample_rate_hz, 5000.0);
    assert_eq!(t.channels[0].dtype, "float32");
    assert_eq!(t.extra["nucleus"], "1H");
    assert_eq!(t.extra["spectrometer_frequency_mhz"], 400.0);
    assert_eq!(t.extra["temperature_k"], 298.15);
    assert_eq!(t.extra["created_on"], "2024-03-09");
    assert_eq!(t.extra["acquired_at"], "2021-09-09T01:46:40.000Z");
    let tr = ds.read_trace(0, 0, 0, 100).unwrap();
    assert_eq!(tr.channels[0], vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
    assert_eq!(tr.channels[1], vec![8.0, 9.0, 10.0, 11.0, 12.0, 13.0]);
    assert!(ds.check().unwrap().ok);
    let e = ds.experiment().unwrap();
    assert_eq!(e.sample.unwrap().id.as_deref(), Some("S-9"));
    assert_eq!(e.acquisition.unwrap().operator.as_deref(), Some("bob"));
}

/// Stored index of (row y, column x) in a 64 x 32 section of 32 x 32 submatrices.
fn sub_index(y: u32, x: u32) -> usize {
    (((y / 32) * 2 + x / 32) * 1024 + (y % 32) * 32 + x % 32) as usize
}

#[test]
fn jeol_2d_complex_submatrices_listed_axis_and_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let ys: Vec<f64> = (0..32).map(|y| f64::from(y) * 0.5).collect();
    let j = Jdf {
        dims: 2,
        format: 2,
        float32: false,
        types: [3, 3],
        points: [64, 32],
        stop: [59, 29],
        unit: [(1, 28), (0x11, 28)],
        axis: [(0.0, 63.0 / 5000.0), (0.0, 15.5)],
        listed_y: Some(ys),
    };
    let section = 64 * 32;
    let mut data = vec![0.0; 4 * section];
    for s in 0..4u32 {
        for y in 0..32u32 {
            for x in 0..64u32 {
                data[s as usize * section + sub_index(y, x)] = f64::from(s * 100_000 + y * 100 + x);
            }
        }
    }
    let bytes = jdf(&j, &data);
    let p = dir.path().join("two.jdf");
    fs::write(&p, &bytes).unwrap();
    let reg = registry();
    let (_, mut ds) = reg.open(&p).unwrap();
    let info = ds.info().unwrap();
    let t = &info.traces[0];
    assert_eq!((t.sample_count, t.sweep_count), (60, 60));
    // sweep 2k: sections 0/1 of row k; sweep 2k+1: sections 2/3
    let tr = ds.read_trace(0, 7, 30, 4).unwrap();
    assert_eq!(
        tr.channels[0],
        vec![200_330.0, 200_331.0, 200_332.0, 200_333.0]
    );
    assert_eq!(
        tr.channels[1],
        vec![300_330.0, 300_331.0, 300_332.0, 300_333.0]
    );
    // the listed Y axis is a table (valid rows only), values in seconds
    assert_eq!(info.tables.len(), 1);
    assert_eq!(info.tables[0].row_count, 30);
    let tab = ds.read_table(0, 2, 2).unwrap();
    assert_eq!(tab.columns[0], vec![2.0, 3.0]);
    assert_eq!(tab.columns[1], vec![0.001, 0.0015]);
    let ind = &t.extra["indirect_dimensions"][0]["axis"];
    assert_eq!(ind["listed"], true);
    assert!(ds.check().unwrap().ok);
    // truncated: open works, check fails with exit-4 finding, reads past the end fail
    let cut = dir.path().join("cut.jdf");
    fs::write(&cut, &bytes[..bytes.len() - 2 * section * 8]).unwrap();
    let (_, mut ds) = reg.open(&cut).unwrap();
    let r = ds.check().unwrap();
    assert!(codes(&r, Severity::Error).contains(&"truncated".to_string()));
    assert!(ds.read_trace(0, 0, 0, 60).is_ok());
    match ds.read_trace(0, 1, 0, 60) {
        Err(e @ Error::Corrupt { .. }) => assert_eq!(e.exit_code(), 4),
        other => panic!("expected a corrupt-file error, got {other:?}"),
    }
}

#[test]
fn jeol_unsupported_layout_and_foreign_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let j = Jdf {
        dims: 2,
        format: 12, // small_two_d
        float32: false,
        types: [1, 1],
        points: [8, 8],
        stop: [7, 7],
        unit: [(1, 26), (1, 26)],
        axis: [(10.0, 0.0), (10.0, 0.0)],
        listed_y: None,
    };
    let p = dir.path().join("small.jdf");
    fs::write(&p, jdf(&j, &[0.0; 64])).unwrap();
    let reg = registry();
    let (_, mut ds) = reg.open(&p).unwrap();
    assert!(
        ds.info()
            .unwrap()
            .notes
            .iter()
            .any(|n| n.contains("unsupported"))
    );
    match ds.read_trace(0, 0, 0, 1) {
        Err(e @ Error::Unsupported { .. }) => assert_eq!(e.exit_code(), 6),
        other => panic!("expected unsupported, got {other:?}"),
    }
    assert!(!ds.check().unwrap().ok);
    let junk = dir.path().join("junk.jdf");
    fs::write(&junk, b"not a JEOL file at all").unwrap();
    assert!(reg.open(&junk).is_err());
    let short = dir.path().join("short.jdf");
    fs::write(&short, b"JEOL.NMR\x01\x01").unwrap();
    match reg.open(&short) {
        Err(e @ Error::Corrupt { .. }) => assert_eq!(e.exit_code(), 4),
        Err(e) => panic!("unexpected error {e}"),
        Ok(_) => panic!("a 10-byte file must not open"),
    }
}

// ------------------------------------------------------------------------------------------
// Bruker 3D processed data and nuslist
// ------------------------------------------------------------------------------------------

#[test]
fn bruker_3d_processed_components_written_by_nmrglue() {
    let exp = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bruker_3d_pdata/1");
    let reg = registry();
    let (det, mut ds) = reg.open(&exp).unwrap();
    assert_eq!(det.format_id, "bruker-nmr");
    let info = ds.info().unwrap();
    let t = info
        .traces
        .iter()
        .find(|t| t.name.as_deref() == Some("pdata/1"))
        .unwrap();
    // SI 8 (direct) x 4 (F2) x 4 (F1): 16 sweeps, F2 fastest
    assert_eq!((t.sample_count, t.sweep_count), (8, 16));
    let names: Vec<&str> = t.channels.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["real", "rri"]);
    assert_eq!(t.extra["shape"], serde_json::json!([4, 4, 8]));
    let idx = t.index;
    for k in 0..4u32 {
        for j in 0..4u32 {
            let tr = ds.read_trace(idx, k * 4 + j, 0, 8).unwrap();
            let want: Vec<f64> = (0..8)
                .map(|i| f64::from(10_000 * k + 100 * j + i))
                .collect();
            assert_eq!(tr.channels[0], want, "row k={k} j={j}");
            let neg: Vec<f64> = want.iter().map(|v| -v).collect();
            assert_eq!(tr.channels[1], neg);
        }
    }
    assert!(ds.check().unwrap().ok);
}

#[test]
fn bruker_nuslist_table_and_bad_nuslist() {
    let dir = tempfile::tempdir().unwrap();
    let exp = dir.path().join("5");
    fs::create_dir_all(&exp).unwrap();
    let params = |pairs: &[(&str, &str)]| {
        let mut s = String::from("##TITLE= Parameter file, TopSpin 4.1.0\n");
        for (k, v) in pairs {
            s.push_str(&format!("##${k}= {v}\n"));
        }
        s + "##END=\n"
    };
    fs::write(
        exp.join("acqus"),
        params(&[
            ("TD", "4"),
            ("AQ_mod", "3"),
            ("SW_h", "100"),
            ("FnTYPE", "2"),
        ]),
    )
    .unwrap();
    fs::write(exp.join("acqu2s"), params(&[("TD", "6")])).unwrap();
    fs::write(exp.join("ser"), vec![0u8; 1024 * 6]).unwrap();
    fs::write(exp.join("nuslist"), "0\n5\n2\n").unwrap();
    let reg = registry();
    let (_, mut ds) = reg.open(&exp).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.tables.len(), 1);
    assert_eq!(info.tables[0].columns[0].name, "index_1");
    assert_eq!(ds.read_table(0, 1, 5).unwrap().columns[0], vec![5.0, 2.0]);
    assert!(ds.check().unwrap().ok);
    fs::write(exp.join("nuslist"), "0 1\n5\n").unwrap();
    let (_, mut ds) = reg.open(&exp).unwrap();
    assert!(ds.info().unwrap().tables.is_empty());
    assert!(
        codes(&ds.check().unwrap(), Severity::Warning).contains(&"nuslist_unreadable".to_string())
    );
}
