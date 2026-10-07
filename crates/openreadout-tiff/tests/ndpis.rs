//! Synthetic Hamamatsu NDPI sets (docs/formats/tiff.md § NDPI sets): a `.ndpis` text file and
//! small uncompressed NDPI files written by hand, one per channel; missing, unreadable and
//! mismatched members.

use std::path::Path;

use openreadout_core::reader::{FormatReader, PlaneIndex};
use openreadout_tiff::TiffReader;

const W: u32 = 8;
const H: u32 = 4;

fn plane(seed: u8) -> Vec<u8> {
    (0..W * H)
        .map(|i| seed.wrapping_mul(16).wrapping_add(i as u8))
        .collect()
}

/// One IFD entry: tag, type, count, value-or-offset (little-endian classic TIFF).
fn entry(b: &mut Vec<u8>, tag: u16, typ: u16, count: u32, value: u32) {
    b.extend(tag.to_le_bytes());
    b.extend(typ.to_le_bytes());
    b.extend(count.to_le_bytes());
    b.extend(value.to_le_bytes());
}

/// An NDPI file: one uncompressed `w` × H uint8 page at 40x (tag 65421), with the fluorescence
/// filter name `filter` in tag 65434 when given.
fn ndpi(seed: u8, w: u32, filter: Option<&str>) -> Vec<u8> {
    let mut b = b"II*\0\0\0\0\0".to_vec();
    let data: Vec<u8> = if w == W {
        plane(seed)
    } else {
        vec![seed; (w * H) as usize]
    };
    b.extend(&data);
    let name_at = b.len() as u32;
    let name = filter.map(|f| {
        let mut v = f.as_bytes().to_vec();
        v.push(0);
        v
    });
    if let Some(n) = &name {
        b.extend(n);
    }
    while !b.len().is_multiple_of(4) {
        b.push(0);
    }
    let ifd = b.len() as u32;
    b[4..8].copy_from_slice(&ifd.to_le_bytes());
    let mut entries = 11u16;
    if name.is_some() {
        entries += 1;
    }
    b.extend(entries.to_le_bytes());
    entry(&mut b, 256, 4, 1, w);
    entry(&mut b, 257, 4, 1, H);
    entry(&mut b, 258, 3, 1, 8);
    entry(&mut b, 259, 3, 1, 1);
    entry(&mut b, 262, 3, 1, 1);
    entry(&mut b, 273, 4, 1, 8);
    entry(&mut b, 277, 3, 1, 1);
    entry(&mut b, 278, 4, 1, H);
    entry(&mut b, 279, 4, 1, w * H);
    entry(&mut b, 65420, 4, 1, 1);
    entry(&mut b, 65421, 11, 1, 40.0f32.to_bits());
    if let Some(n) = &name {
        entry(&mut b, 65434, 2, n.len() as u32, name_at);
    }
    b.extend(0u32.to_le_bytes());
    b
}

const SET: &str = "[NanoZoomer Digital Pathology Image Set]\r\nNoImages=3\r\nImage0=s-DAPI.ndpi\r\nImage1=s-FITC.ndpi\r\nImage2=s-TRITC.ndpi\r\n";

/// A set of three members in `dir`; the TRITC file has no filter tag.
fn write_set(dir: &Path) -> std::path::PathBuf {
    std::fs::write(dir.join("s-DAPI.ndpi"), ndpi(1, W, Some("DAPI 2 (387)"))).unwrap();
    std::fs::write(dir.join("s-FITC.ndpi"), ndpi(2, W, Some("FITC 2 (485)"))).unwrap();
    std::fs::write(dir.join("s-TRITC.ndpi"), ndpi(3, W, None)).unwrap();
    let p = dir.join("s.ndpis");
    std::fs::write(&p, SET).unwrap();
    p
}

#[test]
fn a_set_is_one_image_with_a_channel_per_file() {
    let dir = tempfile::tempdir().unwrap();
    let set = write_set(dir.path());
    let head = std::fs::read(&set).unwrap();
    assert!(TiffReader.sniff(&head, &set).is_some());
    let mut ds = TiffReader.open(&set).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.images.len(), 1);
    let im = &info.images[0];
    assert_eq!((im.size_x, im.size_y, im.size_c, im.size_z), (W, H, 3, 1));
    assert_eq!(im.plane_count, 3);
    let names: Vec<_> = im
        .channels
        .iter()
        .map(|c| c.name.clone().unwrap())
        .collect();
    assert_eq!(names, ["DAPI 2 (387)", "FITC 2 (485)", "s-TRITC"]);
    assert!(
        info.notes
            .iter()
            .any(|n| n == "TIFF sub-format: hamamatsu-ndpis")
    );
    for c in 0..3u8 {
        let p = ds
            .read_plane(
                0,
                PlaneIndex {
                    c: u32::from(c),
                    z: 0,
                    t: 0,
                },
            )
            .unwrap();
        assert_eq!(p.data, plane(c + 1));
    }
    assert_eq!(
        ds.read_plane(0, PlaneIndex { c: 3, z: 0, t: 0 })
            .unwrap_err()
            .exit_code(),
        2
    );
    assert_eq!(ds.member_files().len(), 3);
    let r = ds.check().unwrap();
    assert!(r.ok, "{:?}", r.findings);
    let v = ds.vendor_metadata().unwrap();
    assert_eq!(v["ndpis"]["NoImages"], "3");
    // A single member names its channel from tag 65434 too.
    let mut one = TiffReader.open(&dir.path().join("s-FITC.ndpi")).unwrap();
    let ch = &one.info().unwrap().images[0].channels[0];
    assert_eq!(ch.name.as_deref(), Some("FITC 2 (485)"));
    assert!(one.read_plane(0, PlaneIndex::default()).is_ok());
}

#[test]
fn a_missing_member_is_reported_and_its_channel_fails_with_a_hint() {
    let dir = tempfile::tempdir().unwrap();
    let set = write_set(dir.path());
    std::fs::remove_file(dir.path().join("s-FITC.ndpi")).unwrap();
    let mut ds = TiffReader.open(&set).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.images[0].size_c, 3);
    assert!(
        info.notes.iter().any(|n| n.contains("1 of the 3 files")),
        "{:?}",
        info.notes
    );
    assert!(ds.read_plane(0, PlaneIndex { c: 0, z: 0, t: 0 }).is_ok());
    let e = ds
        .read_plane(0, PlaneIndex { c: 1, z: 0, t: 0 })
        .unwrap_err();
    assert_eq!(e.exit_code(), 4, "{e}");
    assert!(e.to_string().contains("s-FITC.ndpi"), "{e}");
    assert!(
        e.hint().unwrap().contains("whole data set"),
        "{:?}",
        e.hint()
    );
    let r = ds.check().unwrap();
    assert!(!r.ok);
    let codes: Vec<&str> = r.findings.iter().map(|f| f.code.as_str()).collect();
    assert_eq!(codes, ["missing_file"]);
    assert_eq!(ds.member_files().len(), 2);
}

#[test]
fn a_set_with_no_member_on_disk_does_not_open() {
    let dir = tempfile::tempdir().unwrap();
    let set = dir.path().join("s.ndpis");
    std::fs::write(&set, SET).unwrap();
    let e = TiffReader.open(&set).err().expect("the set does not open");
    assert_eq!(e.exit_code(), 4, "{e}");
    assert!(
        e.hint().unwrap().contains("whole data set"),
        "{:?}",
        e.hint()
    );
}

#[test]
fn members_that_differ_or_do_not_open_are_refused_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let set = write_set(dir.path());
    std::fs::write(dir.path().join("s-TRITC.ndpi"), ndpi(3, W * 2, None)).unwrap();
    let e = TiffReader.open(&set).err().expect("the set does not open");
    assert_eq!(e.exit_code(), 4, "{e}");
    assert!(e.to_string().contains("differ"), "{e}");
    // A member that is not a TIFF: the set opens, `check` says the file is unreadable.
    std::fs::write(dir.path().join("s-TRITC.ndpi"), b"not a tiff").unwrap();
    let mut ds = TiffReader.open(&set).unwrap();
    assert_eq!(
        ds.read_plane(0, PlaneIndex { c: 2, z: 0, t: 0 })
            .unwrap_err()
            .exit_code(),
        4
    );
    let r = ds.check().unwrap();
    let codes: Vec<&str> = r.findings.iter().map(|f| f.code.as_str()).collect();
    assert_eq!(codes, ["unreadable_file"]);
    // A text file that is not a set is not detected as one.
    let other = dir.path().join("x.ndpis");
    std::fs::write(&other, "[Something else]\nImage0=a.ndpi\n").unwrap();
    assert!(TiffReader.sniff(b"[Something else]\n", &other).is_none());
}
