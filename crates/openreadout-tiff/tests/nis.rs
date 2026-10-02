//! Synthetic NIS-Elements TIFF exports (docs/formats/tiff.md § NIS-Elements): the private tags
//! and the grouping of an export's files by the index tokens of their names.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use openreadout_core::reader::{FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input, MemFs, MemSource};
use openreadout_tiff::TiffReader;

const W: u32 = 6;
const H: u32 = 3;

fn plane(seed: u32) -> Vec<u8> {
    (0..W * H)
        .flat_map(|i| ((seed * 100 + i) as u16).to_le_bytes())
        .collect()
}

/// A one-page uint16 TIFF with the NIS-Elements tags 65326 (pixel size), 65329 (stage) and
/// 65331 (the `MetadataTiffV1_0` block).
fn nis_tiff(seed: u32, stage_x: f64) -> Vec<u8> {
    let mut b = b"II*\0\0\0\0\0".to_vec();
    b.extend(plane(seed));
    let pixel_at = b.len() as u32;
    b.extend(0.1625f64.to_le_bytes());
    let stage_at = b.len() as u32;
    for v in [stage_x, 5717.1, 2132.28, 7293.6] {
        b.extend(v.to_le_bytes());
    }
    let meta_at = b.len() as u32;
    let mut meta = Vec::new();
    meta.extend(1u32.to_le_bytes());
    meta.extend(16u32.to_le_bytes());
    for u in "MetadataTiffV1_0".encode_utf16() {
        meta.extend(u.to_le_bytes());
    }
    meta.extend([0u8; 16]);
    b.extend(&meta);
    while !b.len().is_multiple_of(2) {
        b.push(0);
    }
    let ifd = b.len() as u32;
    b[4..8].copy_from_slice(&ifd.to_le_bytes());
    let entries: [(u16, u16, u32, u32); 11] = [
        (256, 4, 1, W),
        (257, 4, 1, H),
        (258, 3, 1, 16),
        (259, 3, 1, 1),
        (262, 3, 1, 1),
        (273, 4, 1, 8),
        (278, 4, 1, H),
        (279, 4, 1, W * H * 2),
        (65326, 12, 1, pixel_at),
        (65329, 12, 4, stage_at),
        (65331, 1, meta.len() as u32, meta_at),
    ];
    b.extend((entries.len() as u16).to_le_bytes());
    for (tag, typ, count, value) in entries {
        b.extend(tag.to_le_bytes());
        b.extend(typ.to_le_bytes());
        b.extend(count.to_le_bytes());
        b.extend(value.to_le_bytes());
    }
    b.extend(0u32.to_le_bytes());
    b
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

#[test]
fn export_files_are_grouped_by_their_name_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let first = write(dir.path(), "exp_2xy01c1.tif", &nis_tiff(11, -2546.8));
    write(dir.path(), "exp_2xy01c2.tif", &nis_tiff(12, -2546.8));
    write(dir.path(), "exp_2xy02c1.tif", &nis_tiff(21, -2217.3));
    // another export and an unrelated file in the same folder are left alone
    write(dir.path(), "other_xy01c1.tif", &nis_tiff(99, 0.0));
    write(dir.path(), "exp_2xy01.tif", &nis_tiff(98, 0.0));
    let mut ds = TiffReader.open(&first).unwrap();
    let info = ds.info().unwrap();
    assert!(info.notes[0].contains("nis-elements"), "{:?}", info.notes);
    assert_eq!(info.images.len(), 2);
    let im = &info.images[0];
    assert_eq!(im.name.as_deref(), Some("exp_2xy01"));
    assert_eq!((im.size_c, im.size_z, im.size_t), (2, 1, 1));
    assert_eq!(im.physical_size.x, Some(0.1625));
    assert_eq!(im.channels[1].name.as_deref(), Some("c2"));
    assert_eq!(im.extra["stage_position_um"]["x"], -2546.8);
    for (image, c, seed) in [(0, 0, 11), (0, 1, 12), (1, 0, 21)] {
        let p = ds.read_plane(image, PlaneIndex { c, z: 0, t: 0 }).unwrap();
        assert_eq!(p.data, plane(seed), "image {image} c {c}");
    }
    // position 2 has no c2 file
    let e = ds
        .read_plane(1, PlaneIndex { c: 1, z: 0, t: 0 })
        .unwrap_err();
    assert_eq!(e.exit_code(), 4);
    let r = ds.check().unwrap();
    assert!(!r.ok);
    assert!(r.findings.iter().any(|f| f.code == "missing_planes"));
    assert_eq!(ds.member_files().len(), 2);
}

#[test]
fn a_single_export_file_without_tokens_is_one_image() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "snapshot.tif", &nis_tiff(5, 1.0));
    let mut ds = TiffReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.images.len(), 1);
    assert_eq!(info.images[0].physical_size.y, Some(0.1625));
    assert_eq!(
        ds.read_plane(0, PlaneIndex::default()).unwrap().data,
        plane(5)
    );
    assert!(ds.check().unwrap().ok);
}

/// The files of `dir` (written by the other tests' helpers) copied into memory under `mem/`:
/// readers must find siblings through the input's `Fs`, as for a dropped folder in the
/// browser.
fn in_memory(dir: &Path, open: &str) -> Input {
    let mut fs = MemFs::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let src = MemSource::new(name.clone(), std::fs::read(&p).unwrap());
        fs.insert(Path::new("mem").join(&name), Arc::new(src));
    }
    Input::new(Path::new("mem").join(open), Fs::new(Arc::new(fs)))
}

#[test]
fn export_files_are_grouped_in_memory() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "exp_2xy01c1.tif", &nis_tiff(11, -2546.8));
    write(dir.path(), "exp_2xy01c2.tif", &nis_tiff(12, -2546.8));
    write(dir.path(), "exp_2xy02c1.tif", &nis_tiff(21, -2217.3));
    write(dir.path(), "other_xy01c1.tif", &nis_tiff(99, 0.0));
    let mut ds = TiffReader
        .open_input(&in_memory(dir.path(), "exp_2xy01c1.tif"))
        .unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.images.len(), 2);
    assert_eq!(info.images[0].size_c, 2);
    let p = ds.read_plane(0, PlaneIndex { c: 1, z: 0, t: 0 }).unwrap();
    assert_eq!(p.data, plane(12));
    assert_eq!(ds.member_files().len(), 2);
}
