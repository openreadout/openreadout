//! Synthetic DeltaVision files written from the format note (`docs/formats/deltavision.md`):
//! every section order in both byte orders, the extended header, and damaged files.

use openreadout_core::FormatReader;
use openreadout_core::reader::PlaneIndex;
use openreadout_dv::DvReader;

/// 2 × 2 pixels, 2 wavelengths, 3 Z, 2 time points, uint16; sample value = section index.
fn dv(order: i16, big: bool) -> Vec<u8> {
    let i32b = |v: i32| if big { v.to_be_bytes() } else { v.to_le_bytes() };
    let i16b = |v: i16| if big { v.to_be_bytes() } else { v.to_le_bytes() };
    let f32b = |v: f32| if big { v.to_be_bytes() } else { v.to_le_bytes() };
    let (nc, nz, nt) = (2, 3, 2);
    let sections = nc * nz * nt;
    let ext_len = 2048;
    let mut b = vec![0u8; 1024];
    b[0..4].copy_from_slice(&i32b(2));
    b[4..8].copy_from_slice(&i32b(2));
    b[8..12].copy_from_slice(&i32b(sections));
    b[12..16].copy_from_slice(&i32b(6));
    b[40..44].copy_from_slice(&f32b(0.065));
    b[44..48].copy_from_slice(&f32b(0.065));
    b[48..52].copy_from_slice(&f32b(0.2));
    b[92..96].copy_from_slice(&i32b(ext_len));
    b[96..98].copy_from_slice(&i16b(-16224));
    b[128..130].copy_from_slice(&i16b(8));
    b[130..132].copy_from_slice(&i16b(32));
    b[180..182].copy_from_slice(&i16b(nt as i16));
    b[182..184].copy_from_slice(&i16b(order));
    b[196..198].copy_from_slice(&i16b(nc as i16));
    b[198..200].copy_from_slice(&i16b(528));
    b[200..202].copy_from_slice(&i16b(617));
    b[224..242].copy_from_slice(b"DECON3D:  4 0.1 5 ");
    let mut ext = vec![0u8; ext_len as usize];
    for s in 0..sections as usize {
        let at = s * 160 + 32;
        let (c, _z, t) = match order {
            0 => (s / (nz * nt) as usize, 0, (s / nz as usize) % nt as usize),
            1 => (s % nc as usize, 0, s / (nc * nz) as usize),
            _ => ((s / nz as usize) % nc as usize, 0, s / (nz * nc) as usize),
        };
        ext[at + 4..at + 8].copy_from_slice(&f32b(t as f32 * 2.5));
        ext[at + 32..at + 36].copy_from_slice(&f32b(0.05));
        ext[at + 40..at + 44].copy_from_slice(&f32b(if c == 0 { 490.0 } else { 555.0 }));
        ext[at + 44..at + 48].copy_from_slice(&f32b(if c == 0 { 528.0 } else { 617.0 }));
    }
    b.extend_from_slice(&ext);
    for s in 0..sections {
        for _ in 0..4 {
            b.extend_from_slice(&if big {
                (s as u16).to_be_bytes()
            } else {
                (s as u16).to_le_bytes()
            });
        }
    }
    b
}

fn write(dir: &tempfile::TempDir, name: &str, b: &[u8]) -> std::path::PathBuf {
    let p = dir.path().join(name);
    std::fs::write(&p, b).unwrap();
    p
}

#[test]
fn every_section_order_in_both_byte_orders() {
    let dir = tempfile::tempdir().unwrap();
    for order in 0..3i16 {
        for big in [false, true] {
            let p = write(&dir, "x_R3D_D3D.dv", &dv(order, big));
            let mut ds = DvReader.open(&p).unwrap();
            let info = ds.info().unwrap();
            let im = &info.images[0];
            assert_eq!((im.size_c, im.size_z, im.size_t), (2, 3, 2));
            assert_eq!(im.physical_size.z, Some(0.2));
            assert_eq!(im.channels[1].emission_nm, Some(617.0));
            assert_eq!(im.channels[1].excitation_nm, Some(555.0));
            assert_eq!(im.channels[0].exposure_ms, Some(50.0));
            assert_eq!(im.time_increment_s, Some(2.5));
            assert_eq!(im.extra["written_by"], "softWoRx deconvolution");
            // plane (c=1, z=2, t=1): its section index as the sample value
            let want: u16 = match order {
                0 => 2 + 3 * (1 + 2),
                1 => 1 + 2 * (2 + 3),
                _ => 2 + 3 * (1 + 2),
            };
            let plane = ds.read_plane(0, PlaneIndex { c: 1, z: 2, t: 1 }).unwrap();
            assert_eq!(&plane.data[0..2], &want.to_le_bytes(), "order {order} big {big}");
            assert!(ds.check().unwrap().findings.is_empty());
        }
    }
}

#[test]
fn damaged_files_give_clean_errors() {
    let dir = tempfile::tempdir().unwrap();
    let good = dv(0, false);
    let mut cut = good.clone();
    cut.truncate(good.len() - 10);
    let p = write(&dir, "cut.dv", &cut);
    let mut ds = DvReader.open(&p).unwrap();
    assert!(ds.check().unwrap().findings.iter().any(|f| f.code == "truncated"));
    assert!(ds.read_plane(0, PlaneIndex { c: 1, z: 2, t: 1 }).is_err());
    let p = write(&dir, "short.dv", &good[..500]);
    assert!(DvReader.open(&p).is_err());
    for i in (0..1100).step_by(3) {
        let mut b = good.clone();
        b[i] ^= 0xff;
        let p = write(&dir, "flip.dv", &b);
        if let Ok(mut ds) = DvReader.open(&p) {
            let _ = ds.info();
            let _ = ds.vendor_metadata();
            let _ = ds.check();
            let _ = ds.read_plane(0, PlaneIndex::default());
        }
    }
}
