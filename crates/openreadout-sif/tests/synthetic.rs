//! Synthetic Andor SIF files written from the format note (`docs/formats/andor-sif.md`): a
//! calibrated kinetic series of spectra with a background block, a binned image, and damaged
//! files (no panic, clean errors).

use openreadout_core::FormatReader;
use openreadout_core::reader::PlaneIndex;
use openreadout_sif::SifReader;

fn block(width: u32, height: u32, frames: u32, calibration: &str, values: &[f32]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(b"65567 0 0 1 1504875832 -50 ");
    b.extend_from_slice(b"\x00 \x00 \x00 \x02 \x00 ");
    b.extend_from_slice(b"0 0.1 0.117 0.117 5 ");
    b.extend_from_slice(b"\x00 ");
    b.extend_from_slice(b"0.585 3.3e-08 0 1 20 0 0 0 \x02 \x00 256 0\n");
    b.extend_from_slice(b"DU420_OE \n");
    let name = b"C:\\data\\test.sif";
    b.extend_from_slice(format!(" {width} {height} {}\n", name.len()).as_bytes());
    b.extend_from_slice(name);
    b.extend_from_slice(b" \n65538 4\nnote\n");
    b.extend_from_slice(b"65538 \x01 \x02 \x03 \x00 0.027 0\n");
    b.extend_from_slice(b"65540 0 0 500 0 0 1200 1200\n0 SR193i\n");
    b.extend_from_slice(b"65540 \x02 \x00 \x01 \x00 \x01 \x00\n");
    b.extend_from_slice(calibration.as_bytes());
    b.extend_from_slice(b"\n0 1 0 0\n0 1 0 0\n422\n13\n13\n1\n");
    b.extend_from_slice(b"10\nWavelength6\nCounts12\nPixel number");
    let px = width * height;
    b.extend_from_slice(
        format!(
            "65541 1 {height} {width} 1 {frames} 1 {} {px}\n65538 1 {height} {width} 1 1 1 0\n",
            px * frames
        )
        .as_bytes(),
    );
    for f in 0..frames {
        b.extend_from_slice(format!("{}\n", f * 585_000).as_bytes());
    }
    b.extend_from_slice(b"0\n");
    for v in values {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b
}

fn file(blocks: &[(usize, Vec<u8>)]) -> Vec<u8> {
    let mut b = b"Andor Technology Multi-Channel File\n65538 1\n".to_vec();
    b.extend_from_slice(&blocks[0].1);
    for role in 1..5 {
        match blocks.iter().find(|(r, _)| *r == role) {
            Some((_, x)) => {
                b.extend_from_slice(b"1\n");
                b.extend_from_slice(x);
            }
            None => b.extend_from_slice(b"0\n"),
        }
    }
    b.extend_from_slice(b"<?xml version=\"1.0\" ?><Signals><Signal><TInstaImageData><PreAmpGainText>1x</PreAmpGainText></TInstaImageData></Signal></Signals>");
    b
}

fn write(dir: &tempfile::TempDir, name: &str, b: &[u8]) -> std::path::PathBuf {
    let p = dir.path().join(name);
    std::fs::write(&p, b).unwrap();
    p
}

#[test]
fn calibrated_kinetic_spectra_with_a_background_block() {
    let dir = tempfile::tempdir().unwrap();
    let signal = block(
        4,
        1,
        2,
        "400 0.5 0 0",
        &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0],
    );
    let background = block(4, 1, 1, "400 0.5 0 0", &[0.5, 0.5, 0.5, 0.5]);
    let p = write(&dir, "spectra.sif", &file(&[(0, signal), (2, background)]));
    let mut ds = SifReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("65567"));
    assert_eq!(info.traces.len(), 2);
    let t = &info.traces[0];
    assert_eq!(t.name.as_deref(), Some("signal"));
    assert_eq!((t.sample_count, t.sweep_count), (4, 2));
    assert_eq!(t.channels[0].unit.as_deref(), Some("nm"));
    assert_eq!(info.traces[1].name.as_deref(), Some("background"));
    let s1 = ds.read_trace(0, 1, 0, 10).unwrap();
    assert_eq!(s1.channels[0], vec![400.5, 401.0, 401.5, 402.0]);
    assert_eq!(s1.channels[1], vec![5.0, 6.0, 7.0, 8.0]);
    assert_eq!(
        ds.read_trace(1, 0, 0, 10).unwrap().channels[1],
        vec![0.5; 4]
    );
    let v = ds.vendor_metadata().unwrap();
    assert!(v["xml"].is_object(), "{v}");
    let r = ds.check().unwrap();
    assert!(r.findings.is_empty(), "{:?}", r.findings);
}

#[test]
fn an_image_block() {
    let dir = tempfile::tempdir().unwrap();
    let vals: Vec<f32> = (0..6).map(|x| x as f32).collect();
    let p = write(
        &dir,
        "image.sif",
        &file(&[(0, block(3, 2, 1, "0 1 0 0", &vals))]),
    );
    let mut ds = SifReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    let im = &info.images[0];
    assert_eq!((im.size_x, im.size_y, im.size_t), (3, 2, 1));
    assert_eq!(im.acquired_at.as_deref(), Some("2017-09-08T13:03:52Z"));
    let plane = ds.read_plane(0, PlaneIndex::default()).unwrap();
    assert_eq!(plane.data.len(), 24);
    assert_eq!(&plane.data[20..24], &5.0f32.to_le_bytes());
}

#[test]
fn damaged_files_give_clean_errors() {
    let dir = tempfile::tempdir().unwrap();
    let good = file(&[(0, block(4, 1, 2, "400 0.5 0 0", &[1.0; 8]))]);
    let mut cut = good.clone();
    cut.truncate(good.len() - 160);
    let p = write(&dir, "cut.sif", &cut);
    let mut ds = SifReader.open(&p).unwrap();
    assert!(
        ds.check()
            .unwrap()
            .findings
            .iter()
            .any(|f| f.code == "truncated")
    );
    assert!(ds.read_trace(0, 1, 0, 4).is_err());
    for i in (0..good.len()).step_by(5) {
        let mut b = good.clone();
        b[i] ^= 0x5a;
        let p = write(&dir, "flip.sif", &b);
        if let Ok(mut ds) = SifReader.open(&p) {
            let _ = ds.info();
            let _ = ds.check();
            let _ = ds.read_trace(0, 0, 0, 10);
            let _ = ds.read_plane(0, PlaneIndex::default());
        }
    }
}
