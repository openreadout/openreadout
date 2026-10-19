//! Synthetic-dataset tests of the EEGLAB reader, with MAT-files written here.

use std::io::Write;

use openreadout_core::reader::{Dataset, FormatReader};

use super::{EeglabDataset, EeglabReader};

fn pad8(b: &mut Vec<u8>) {
    while !b.len().is_multiple_of(8) {
        b.push(0);
    }
}

fn element(ty: u32, data: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(ty.to_le_bytes());
    b.extend((data.len() as u32).to_le_bytes());
    b.extend(data);
    pad8(&mut b);
    b
}

fn matrix(class: u8, dims: &[i32], name: &str, body: &[u8]) -> Vec<u8> {
    let mut b = element(
        6,
        &[&u32::from(class).to_le_bytes()[..], &0u32.to_le_bytes()[..]].concat(),
    );
    b.extend(element(
        5,
        &dims
            .iter()
            .flat_map(|d| d.to_le_bytes())
            .collect::<Vec<_>>(),
    ));
    b.extend(element(1, name.as_bytes()));
    b.extend(body);
    element(14, &b)
}

fn double(name: &str, dims: &[i32], v: &[f64]) -> Vec<u8> {
    matrix(
        6,
        dims,
        name,
        &element(
            9,
            &v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<_>>(),
        ),
    )
}

fn single(name: &str, dims: &[i32], v: &[f32]) -> Vec<u8> {
    matrix(
        7,
        dims,
        name,
        &element(
            7,
            &v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<_>>(),
        ),
    )
}

fn scalar(name: &str, v: f64) -> Vec<u8> {
    // stored narrower than the class, as MATLAB does for small integers
    if v.fract() == 0.0 && (0.0..256.0).contains(&v) {
        return matrix(6, &[1, 1], name, &element(2, &[v as u8]));
    }
    double(name, &[1, 1], &[v])
}

fn chars(name: &str, s: &str) -> Vec<u8> {
    let u: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
    matrix(
        4,
        &[1, s.encode_utf16().count() as i32],
        name,
        &element(4, &u),
    )
}

/// A struct array: `rows[e]` holds element `e`'s field values (unnamed `miMATRIX` elements).
fn strukt(name: &str, fields: &[&str], rows: &[Vec<Vec<u8>>]) -> Vec<u8> {
    let len = 32usize;
    let mut body = Vec::new();
    body.extend(5u32.to_le_bytes());
    body.extend(4u32.to_le_bytes());
    body.extend((len as i32).to_le_bytes());
    body.extend(0u32.to_le_bytes()); // a full 8-byte tag + data for the field name length
    let mut names = Vec::new();
    for f in fields {
        let mut n = f.as_bytes().to_vec();
        n.resize(len, 0);
        names.extend(n);
    }
    body.extend(element(1, &names));
    for r in rows {
        for v in r {
            body.extend(v);
        }
    }
    matrix(2, &[1, rows.len() as i32], name, &body)
}

fn compressed(e: &[u8]) -> Vec<u8> {
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(e).unwrap();
    let c = z.finish().unwrap();
    let mut b = Vec::new();
    b.extend(15u32.to_le_bytes());
    b.extend((c.len() as u32).to_le_bytes());
    b.extend(c);
    b
}

fn mat(vars: &[Vec<u8>]) -> Vec<u8> {
    let mut h =
        b"MATLAB 5.0 MAT-file, Platform: GLNXA64, Created on: Mon Oct 19 10:00:00 2026".to_vec();
    h.resize(116, b' ');
    h.extend([0u8; 8]);
    h.extend(0x0100u16.to_le_bytes());
    h.extend(b"IM");
    for v in vars {
        h.extend(v);
    }
    h
}

/// The EEG fields of a 2-channel dataset with `pnts` points and `trials` epochs.
fn fields(pnts: i32, trials: i32, data: Vec<u8>) -> Vec<(String, Vec<u8>)> {
    let lab = |s: &str| chars("", s);
    let ev = |t: &str, lat: f64| {
        vec![
            chars("", t),
            double("", &[1, 1], &[lat]),
            double("", &[0, 0], &[]),
        ]
    };
    vec![
        ("setname".into(), chars("setname", "demo")),
        ("nbchan".into(), scalar("nbchan", 2.0)),
        ("pnts".into(), scalar("pnts", f64::from(pnts))),
        ("trials".into(), scalar("trials", f64::from(trials))),
        ("srate".into(), scalar("srate", 250.0)),
        ("xmin".into(), double("xmin", &[1, 1], &[-0.1])),
        ("ref".into(), chars("ref", "common")),
        ("data".into(), data),
        (
            "chanlocs".into(),
            strukt(
                "chanlocs",
                &["labels", "type"],
                &[vec![lab("Fz"), lab("EEG")], vec![lab("Cz"), lab("")]],
            ),
        ),
        (
            "event".into(),
            strukt(
                "event",
                &["type", "latency", "duration"],
                &[ev("stim", 1.0), ev("resp", 3.5)],
            ),
        ),
    ]
}

fn as_struct(fields: Vec<(String, Vec<u8>)>) -> Vec<u8> {
    // one EEG struct: each field value renamed to "" (struct fields are unnamed elements)
    let names: Vec<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
    let values: Vec<Vec<u8>> = fields.iter().map(|(_, v)| unname(v)).collect();
    strukt("EEG", &names, &[values])
}

/// Rewrite an `miMATRIX` element's name to empty.
fn unname(e: &[u8]) -> Vec<u8> {
    // tag (8) + flags (16) + dims element + name element
    let dims_len = u32::from_le_bytes(e[28..32].try_into().unwrap()) as usize;
    let dims_end = 32 + dims_len.div_ceil(8) * 8;
    let name_len = u32::from_le_bytes(e[dims_end + 4..dims_end + 8].try_into().unwrap()) as usize;
    let name_end = dims_end + 8 + name_len.div_ceil(8) * 8;
    let mut body = e[8..dims_end].to_vec();
    body.extend(element(1, b""));
    body.extend(&e[name_end..]);
    element(14, &body)
}

fn samples(n: usize) -> Vec<f32> {
    (0..n).map(|i| i as f32 * 0.5).collect()
}

#[test]
fn embedded_single_variable() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.set");
    let v = samples(8);
    let bytes = mat(&[as_struct(fields(4, 1, single("data", &[2, 4], &v)))]);
    assert!(EeglabReader.sniff(&bytes[..128], &p).is_some());
    std::fs::write(&p, &bytes).unwrap();
    let mut ds = EeglabDataset::open(&p).unwrap();
    let info = ds.info().unwrap();
    let t = &info.traces[0];
    assert_eq!(
        (t.sample_rate_hz, t.sample_count, t.sweep_count),
        (250.0, 4, 1)
    );
    assert_eq!(t.channels[1].name, "Cz");
    assert_eq!(t.channels[0].extra["type"], "EEG");
    assert_eq!(t.extra["setname"], "demo");
    assert_eq!(t.extra["mat_writer"], "MATLAB GLNXA64");
    let tr = ds.read_trace(0, 0, 1, 2).unwrap();
    assert_eq!(tr.channels[0], vec![1.0, 2.0]);
    assert_eq!(tr.channels[1], vec![1.5, 2.5]);
    let rows = ds.read_table(0, 0, 10).unwrap();
    assert_eq!(rows.columns[0], vec![0.0, 2.5 / 250.0]);
    assert!(rows.columns[1].iter().all(|d| d.is_nan()));
    assert_eq!(
        info.tables[0].columns[5].extra["categories"],
        serde_json::json!(["stim", "resp"])
    );
    assert!(ds.check().unwrap().ok);
}

#[test]
fn fields_as_variables_compressed_with_fdt_and_epochs() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("b.set");
    let vars: Vec<Vec<u8>> = fields(3, 2, chars("data", "b.fdt"))
        .into_iter()
        .map(|(_, v)| compressed(&v))
        .collect();
    std::fs::write(&p, mat(&vars)).unwrap();
    let v = samples(12); // 2 channels × 3 points × 2 epochs
    std::fs::write(
        dir.path().join("b.fdt"),
        v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<_>>(),
    )
    .unwrap();
    let mut ds = EeglabDataset::open(&p).unwrap();
    let t = &ds.info().unwrap().traces[0];
    assert_eq!(
        (t.sweep_count, t.sample_count, t.start_s),
        (2, 3, Some(-0.1))
    );
    assert_eq!(t.channels[0].dtype, "float32");
    let tr = ds.read_trace(0, 1, 0, 10).unwrap();
    assert_eq!(tr.channels[0], vec![3.0, 4.0, 5.0]);
    assert_eq!(tr.channels[1], vec![3.5, 4.5, 5.5]);
    // the .fdt opens the dataset too
    assert!(EeglabDataset::open(&dir.path().join("b.fdt")).is_ok());
}

#[test]
fn large_embedded_data_is_read_lazily() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("c.set");
    let n: i32 = 600_000; // 2 × 600,000 doubles: past the parse-time limit
    let v: Vec<f64> = (0..2 * n).map(|i| f64::from(i) * 0.25).collect();
    let data = double("data", &[2, n], &v);
    for compress in [false, true] {
        let vars: Vec<Vec<u8>> = fields(n, 1, data.clone())
            .into_iter()
            .map(|(_, v)| if compress { compressed(&v) } else { v })
            .collect();
        std::fs::write(&p, mat(&vars)).unwrap();
        let mut ds = EeglabDataset::open(&p).unwrap();
        let tr = ds.read_trace(0, 0, 299_999, 2).unwrap();
        assert_eq!(tr.channels[0], vec![599_998.0 * 0.25, 600_000.0 * 0.25]);
        assert_eq!(tr.channels[1][0], 599_999.0 * 0.25);
    }
}

#[test]
fn damaged_and_foreign_files_fail_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("d.set");
    let bytes = mat(&[as_struct(fields(
        4,
        1,
        single("data", &[2, 4], &samples(8)),
    ))]);
    for n in [0, 100, 128, 140, 200, bytes.len() - 9] {
        std::fs::write(&p, &bytes[..n]).unwrap();
        if let Ok(mut ds) = EeglabDataset::open(&p) {
            let _ = ds.info();
            let _ = ds.read_trace(0, 0, 0, 10);
        }
    }
    // an Axona .set is text: not claimed
    assert!(EeglabReader.sniff(b"trial_date Monday", &p).is_none());
    // a MAT-file without an EEG structure
    std::fs::write(&p, mat(&[double("x", &[1, 1], &[1.0])])).unwrap();
    assert!(EeglabDataset::open(&p).is_err());
    // a MAT 7.3 header without an HDF5 file after it fails cleanly
    let mut h = b"MATLAB 7.3 MAT-file, Platform: GLNXA64, Created on: Mon Oct 19 10:00:00 2026 HDF5 schema 1.00 .".to_vec();
    h.resize(512, b' ');
    std::fs::write(&p, &h).unwrap();
    assert!(EeglabDataset::open(&p).is_err());
}

/// A MAT 7.3 (HDF5) dataset with one variable per field, written with hdf5-pure's MAT writer.
fn mat73(pnts: usize, trials: usize, data: &[f32], fdt: Option<&str>) -> Vec<u8> {
    let mut b = hdf5_pure::mat::MatBuilder::new(hdf5_pure::mat::Options::default());
    b.write_scalar_f64("nbchan", 2.0).unwrap();
    b.write_scalar_f64("pnts", pnts as f64).unwrap();
    b.write_scalar_f64("trials", trials as f64).unwrap();
    b.write_scalar_f64("srate", 500.0).unwrap();
    b.write_scalar_f64("xmin", 0.0).unwrap();
    b.write_char("setname", "h5").unwrap();
    match fdt {
        Some(n) => b.write_char("data", n).unwrap(),
        None if trials > 1 => b.write_f32("data", &[2, pnts, trials], data).unwrap(),
        None => b.write_f32("data", &[2, pnts], data).unwrap(),
    };
    b.finish().unwrap()
}

#[test]
fn mat73_embedded_and_epoched() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("h.set");
    std::fs::write(&p, mat73(3, 1, &samples(6), None)).unwrap();
    let mut ds = EeglabDataset::open(&p).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("MAT 7.3"));
    assert_eq!(info.traces[0].sample_count, 3);
    let tr = ds.read_trace(0, 0, 1, 2).unwrap();
    assert_eq!(tr.channels[0], vec![1.0, 2.0]);
    assert_eq!(tr.channels[1], vec![1.5, 2.5]);
    // 2 channels × 3 points × 2 epochs, read by epoch rows
    std::fs::write(&p, mat73(3, 2, &samples(12), None)).unwrap();
    let mut ds = EeglabDataset::open(&p).unwrap();
    assert_eq!(ds.info().unwrap().traces[0].sweep_count, 2);
    let tr = ds.read_trace(0, 1, 1, 2).unwrap();
    assert_eq!(tr.channels[0], vec![4.0, 5.0]);
    assert_eq!(tr.channels[1], vec![4.5, 5.5]);
    // the samples in a .fdt
    std::fs::write(&p, mat73(3, 1, &[], Some("h.fdt"))).unwrap();
    std::fs::write(
        dir.path().join("h.fdt"),
        samples(6)
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let mut ds = EeglabDataset::open(&p).unwrap();
    assert_eq!(
        ds.read_trace(0, 0, 0, 3).unwrap().channels[1],
        vec![0.5, 1.5, 2.5]
    );
}

#[test]
fn mat73_large_data_read_by_rows() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("big.set");
    let n = 600_000; // 2 × 600,000 float32: past the parse-time limit
    let v: Vec<f32> = (0..2 * n).map(|i| i as f32).collect();
    std::fs::write(&p, mat73(n, 1, &v, None)).unwrap();
    let mut ds = EeglabDataset::open(&p).unwrap();
    let tr = ds.read_trace(0, 0, 599_998, 10).unwrap();
    assert_eq!(tr.channels[0], vec![1_199_996.0, 1_199_998.0]);
    assert_eq!(tr.channels[1], vec![1_199_997.0, 1_199_999.0]);
    std::fs::write(&p, mat73(n / 2, 2, &v, None)).unwrap();
    let mut ds = EeglabDataset::open(&p).unwrap();
    let tr = ds.read_trace(0, 1, 0, 1).unwrap();
    assert_eq!(tr.channels[1], vec![600_001.0]);
}
