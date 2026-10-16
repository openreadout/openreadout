//! Synthetic CBF and SMV files: round trips, integrity checks and damaged files.
#![allow(clippy::many_single_char_names)]

use openreadout_core::diffraction::Geometry;
use openreadout_core::{FormatReader, PlaneIndex};
use openreadout_diffraction::cbf::encode_byte_offset;
use openreadout_diffraction::common::{base64, md5};
use openreadout_diffraction::{CbfReader, SmvReader};

fn cbf_bytes(values: &[i64], w: usize, h: usize, md5_ok: bool) -> Vec<u8> {
    let coded = encode_byte_offset(values);
    let digest = if md5_ok {
        base64(&md5(&coded))
    } else {
        "AAAAAAAAAAAAAAAAAAAAAA==".into()
    };
    let mut f = Vec::new();
    f.extend_from_slice(
        b"###CBF: VERSION 1.5, CBFlib v0.7.8 - PILATUS detectors\r\n\r\ndata_test_0001\r\n\r\n_array_data.header_convention \"PILATUS_1.2\"\r\n_array_data.header_contents\r\n;\r\n# Detector: PILATUS 2M, S/N 24-0107\r\n# 2012-10-06T10:00:00.000\r\n# Pixel_size 172e-6 m x 172e-6 m\r\n# Exposure_time 0.1 s\r\n# Count_cutoff 1000 counts\r\n# Wavelength 1.0 A\r\n# Detector_distance 0.2 m\r\n# Beam_xy (2.5, 1.5) pixels\r\n# Start_angle 10.0 deg.\r\n# Angle_increment 0.1 deg.\r\n;\r\n\r\n_array_data.data\r\n;\r\n--CIF-BINARY-FORMAT-SECTION--\r\nContent-Type: application/octet-stream;\r\n     conversions=\"x-CBF_BYTE_OFFSET\"\r\nContent-Transfer-Encoding: BINARY\r\n",
    );
    f.extend_from_slice(
        format!(
            "X-Binary-Size: {}\r\nX-Binary-ID: 1\r\nX-Binary-Element-Type: \"signed 32-bit integer\"\r\nX-Binary-Element-Byte-Order: LITTLE_ENDIAN\r\nContent-MD5: {digest}\r\nX-Binary-Number-of-Elements: {}\r\nX-Binary-Size-Fastest-Dimension: {w}\r\nX-Binary-Size-Second-Dimension: {h}\r\nX-Binary-Size-Padding: 4095\r\n\r\n",
            coded.len(),
            w * h
        )
        .as_bytes(),
    );
    f.extend_from_slice(&[0x0c, 0x1a, 0x04, 0xd5]);
    f.extend_from_slice(&coded);
    f.extend_from_slice(&[0u8; 16]);
    f.extend_from_slice(b"\r\n--CIF-BINARY-FORMAT-SECTION----\r\n;\r\n\r\n");
    f
}

fn write(dir: &tempfile::TempDir, name: &str, b: &[u8]) -> std::path::PathBuf {
    let p = dir.path().join(name);
    std::fs::write(&p, b).unwrap();
    p
}

#[test]
fn cbf_round_trip_geometry_and_check() {
    let (w, h) = (4usize, 3usize);
    let values: Vec<i64> = vec![0, 5, -1, 300, -2, 70_000, 1000, 2, 3, 4, 5, -1];
    let d = tempfile::tempdir().unwrap();
    let p = write(&d, "x.cbf", &cbf_bytes(&values, w, h, true));
    assert!(CbfReader.sniff(&std::fs::read(&p).unwrap(), &p).is_some());
    let mut ds = CbfReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    let im = &info.images[0];
    assert_eq!((im.size_x, im.size_y), (4, 3));
    assert_eq!(info.format_version.as_deref(), Some("1.5"));
    let g = Geometry::of(im).unwrap();
    assert_eq!(g.beam_center_px, Some([2.5, 1.5]));
    assert_eq!(g.distance_mm, Some(200.0));
    assert_eq!(g.overload, Some(1000.0));
    assert!(g.negative_is_masked);
    let plane = ds.read_plane(0, PlaneIndex::default()).unwrap();
    let got: Vec<i64> = plane
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| i64::from(i32::from_le_bytes(*c)))
        .collect();
    assert_eq!(got, values);
    let r = ds.check().unwrap();
    assert!(r.ok, "{:?}", r.findings);
    assert!(r.findings.iter().any(|f| f.code == "masked_pixels"));
    assert!(r.findings.iter().any(|f| f.code == "overloads"));
    let e = openreadout_core::experiment::of_dataset(ds.as_ref(), &info);
    assert_eq!(e.method.unwrap().technique.unwrap().id, "CHMO:0000159");
}

#[test]
fn cbf_bad_md5_truncation_and_damage_do_not_panic() {
    let values: Vec<i64> = (0..64).map(|i| (i * 37) % 500).collect();
    let d = tempfile::tempdir().unwrap();
    let p = write(&d, "bad.cbf", &cbf_bytes(&values, 8, 8, false));
    let mut ds = CbfReader.open(&p).unwrap();
    let r = ds.check().unwrap();
    assert!(r.findings.iter().any(|f| f.code == "md5_mismatch"));
    let good = cbf_bytes(&values, 8, 8, true);
    // cut inside the binary data: opens, the plane read is a corrupt-file error
    let cut = write(&d, "cut.cbf", &good[..good.len() - 60]);
    if let Ok(mut ds) = CbfReader.open(&cut) {
        let _ = ds.info();
        assert!(ds.read_plane(0, PlaneIndex::default()).is_err());
        let _ = ds.check();
    }
    // flip bytes everywhere: never a panic
    for i in (0..good.len()).step_by(7) {
        let mut b = good.clone();
        b[i] ^= 0x5a;
        let q = write(&d, "flip.cbf", &b);
        if let Ok(mut ds) = CbfReader.open(&q) {
            let _ = ds.info();
            let _ = ds.read_plane(0, PlaneIndex::default());
            let _ = ds.check();
        }
    }
}

fn smv_bytes(w: usize, h: usize, big: bool) -> Vec<u8> {
    let head = format!(
        "{{\nHEADER_BYTES=  512;\nDIM=2;\nBYTE_ORDER={};\nTYPE=unsigned_short;\nSIZE1={w};\nSIZE2={h};\nPIXEL_SIZE=0.1;\nDISTANCE=100.0;\nWAVELENGTH=1.0;\nBEAM_CENTER_X=0.3;\nBEAM_CENTER_Y=0.2;\nOSC_START=0;\nOSC_RANGE=1;\nTIME=1.0;\n}}\n",
        if big { "big_endian" } else { "little_endian" }
    );
    let mut b = head.into_bytes();
    b.resize(512, b' ');
    for i in 0..(w * h) as u16 {
        let v = i * 100;
        b.extend_from_slice(&if big {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        });
    }
    b
}

#[test]
fn smv_both_byte_orders_and_adsc_beam_centre() {
    let d = tempfile::tempdir().unwrap();
    for big in [false, true] {
        let p = write(&d, "x.img", &smv_bytes(5, 4, big));
        let mut ds = SmvReader.open(&p).unwrap();
        let info = ds.info().unwrap();
        let g = Geometry::of(&info.images[0]).unwrap();
        // ADSC: X is the slow axis, Y the fast one, in mm
        let [x, y] = g.beam_center_px.unwrap();
        assert!((x - 2.0).abs() < 1e-9 && (y - 3.0).abs() < 1e-9);
        let plane = ds.read_plane(0, PlaneIndex::default()).unwrap();
        assert_eq!(u16::from_le_bytes([plane.data[2], plane.data[3]]), 100);
        assert!(ds.check().unwrap().ok);
    }
    let good = smv_bytes(5, 4, false);
    let cut = write(&d, "cut.img", &good[..good.len() - 3]);
    let mut ds = SmvReader.open(&cut).unwrap();
    assert!(ds.read_plane(0, PlaneIndex::default()).is_err());
    assert!(!ds.check().unwrap().ok);
    for i in (0..512).step_by(5) {
        let mut b = good.clone();
        b[i] ^= 0x21;
        let q = write(&d, "flip.img", &b);
        if let Ok(mut ds) = SmvReader.open(&q) {
            let _ = ds.info();
            let _ = ds.read_plane(0, PlaneIndex::default());
            let _ = ds.check();
        }
    }
}
