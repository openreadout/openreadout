//! The synthetic MIRAX fixture (`oracle/make_mirax_fixtures.py`): geometry, placement of camera
//! photos at level 0 (stored samples copied, the later photo in raster order on top, empty
//! cameras left as fill), fractional placement at coarser levels, attachments and `check`.

use std::path::{Path, PathBuf};

use openreadout_core::{FormatReader, PlaneIndex, Region};
use openreadout_wsi::MiraxReader;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mirax-synthetic.mrxs")
}

/// The generator's image content (level, grid cell, pixel) → R, G, B.
fn stored(level: u32, gx: u32, gy: u32, x: u32, y: u32) -> [u8; 3] {
    [
        ((gx * 16 + x * 3 + level * 50) % 256) as u8,
        ((gy * 16 + y * 5 + level * 30) % 256) as u8,
        ((40 + level * 60 + x + y) % 256) as u8,
    ]
}

fn px(p: &openreadout_core::Plane, x: u32, y: u32) -> [u8; 3] {
    let o = ((y * p.width + x) * 3) as usize;
    [p.data[o], p.data[o + 1], p.data[o + 2]]
}

#[test]
fn geometry_and_levels() {
    let ds = MiraxReader.open(&fixture()).unwrap();
    let info = ds.info().unwrap();
    let im = &info.images[0];
    // 2 cameras of 16 px with a nominal 1-px overlap: 2 * (16 - 1) + 1
    assert_eq!((im.size_x, im.size_y), (31, 23));
    assert_eq!(im.samples_per_pixel, 3);
    assert_eq!(im.pyramid_levels, 4);
    let sizes: Vec<(u32, u32)> = im
        .resolution_levels
        .iter()
        .map(|l| (l.size_x, l.size_y))
        .collect();
    assert_eq!(sizes, vec![(31, 23), (15, 11), (7, 5), (3, 2)]);
    assert_eq!(im.physical_size.x, Some(0.25));
    assert_eq!(im.acquired_at.as_deref(), Some("2026-09-26T12:00:00"));
    let m = &im.extra["mirax"];
    assert_eq!(m["cameras_with_images"], 3);
    assert_eq!(m["camera_positions"], "position buffer");
}

#[test]
fn level0_copies_samples_and_the_later_photo_wins() {
    let mut ds = MiraxReader.open(&fixture()).unwrap();
    let p = ds.read_plane(0, PlaneIndex::default()).unwrap();
    assert_eq!((p.width, p.height), (31, 23));
    // camera 0 at (0, 0): image (0, 0) at pixels 0..4 x 0..3, image (1, 0) next to it
    for y in 0..3 {
        for x in 0..4 {
            assert_eq!(px(&p, x, y), stored(0, 0, 0, x, y), "({x},{y})");
            assert_eq!(px(&p, 4 + x, y), stored(0, 1, 0, x, y));
        }
    }
    // camera 1 at (15, 1) overlaps camera 0's last column (x = 15): camera 1 is later in raster
    // order (grid column 4 after 3 on the same grid row), so it shows there
    assert_eq!(px(&p, 15, 1), stored(0, 4, 0, 0, 0));
    assert_eq!(px(&p, 15, 0), stored(0, 3, 0, 3, 0)); // row 0: camera 1 starts at y = 1
    // camera 2 (bottom left, flag 0) has no images: fill colour
    assert_eq!(px(&p, 2, 20), [255, 255, 255]);
    // camera 3 at (14, 11): its first image
    assert_eq!(px(&p, 14, 11), stored(0, 4, 4, 0, 0));
    // a region equals the crop of the whole plane
    let r = ds
        .read_region(0, PlaneIndex::default(), 0, Region::new(10, 5, 12, 9))
        .unwrap();
    for y in 0..9 {
        for x in 0..12 {
            assert_eq!(px(&r, x, y), px(&p, 10 + x, 5 + y));
        }
    }
}

#[test]
#[allow(clippy::many_single_char_names)]
fn coarser_levels_resample_fractional_positions() {
    let mut ds = MiraxReader.open(&fixture()).unwrap();
    // level 1: camera 0's first image at (0, 0) exactly: copied
    let p1 = ds.read_plane_level(0, PlaneIndex::default(), 1).unwrap();
    assert_eq!(px(&p1, 1, 1), stored(1, 0, 0, 1, 1));
    // camera 1 at x = 15 / 2 = 7.5: pixel 8 lies half on its column 0 and column 1
    let a = stored(1, 4, 0, 0, 0);
    let b = stored(1, 4, 0, 1, 0);
    // y = 1 / 2 = 0.5 too: pixel (8, 1) samples rows 0 and 1 of columns 0 and 1
    let c = stored(1, 4, 0, 0, 1);
    let d = stored(1, 4, 0, 1, 1);
    let got = px(&p1, 8, 1);
    for k in 0..3 {
        let want = (f32::from(a[k]) + f32::from(b[k]) + f32::from(c[k]) + f32::from(d[k])) / 4.0;
        assert!((f32::from(got[k]) - want).abs() <= 0.5, "{got:?} vs {want}");
    }
    // every level reads, and region = crop of the level
    for level in 0..4 {
        let whole = ds
            .read_plane_level(0, PlaneIndex::default(), level)
            .unwrap();
        let (w, h) = (whole.width, whole.height);
        let r = ds
            .read_region(
                0,
                PlaneIndex::default(),
                level,
                Region::new(1, 1, w - 1, h - 1),
            )
            .unwrap();
        for y in 0..h - 1 {
            for x in 0..w - 1 {
                assert_eq!(px(&r, x, y), px(&whole, x + 1, y + 1), "level {level}");
            }
        }
    }
    assert!(ds.read_plane_level(0, PlaneIndex::default(), 4).is_err());
    assert!(
        ds.read_region(0, PlaneIndex::default(), 0, Region::new(30, 0, 5, 5))
            .is_err()
    );
}

#[test]
fn attachments_and_check() {
    let mut ds = MiraxReader.open(&fixture()).unwrap();
    let a = ds.attachments().unwrap();
    assert_eq!(a[0].name, "label");
    assert_eq!(a[0].content_type, "BMP");
    let label = ds.read_attachment(0).unwrap();
    assert!(label.starts_with(b"BM"));
    assert!(a.iter().any(|x| x.name == "mrxs preview"));
    let r = ds.check().unwrap();
    assert!(r.findings.is_empty(), "{:?}", r.findings);
}

#[test]
fn missing_directory_and_broken_index_are_errors() {
    let dir = std::env::temp_dir().join(format!("openreadout-mirax-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let lone = dir.join("lone.mrxs");
    std::fs::write(&lone, [0xFF, 0xD8]).unwrap();
    assert!(MiraxReader.open(&lone).is_err());
    // a copy with Index.dat cut short
    let src = fixture().with_extension("");
    let copy = dir.join("cut");
    std::fs::create_dir_all(&copy).unwrap();
    for f in ["Slidedat.ini", "Data0000.dat"] {
        std::fs::copy(src.join(f), copy.join(f)).unwrap();
    }
    let idx = std::fs::read(src.join("Index.dat")).unwrap();
    std::fs::write(copy.join("Index.dat"), &idx[..60]).unwrap();
    std::fs::write(dir.join("cut.mrxs"), [0xFF, 0xD8]).unwrap();
    let e = MiraxReader.open(&dir.join("cut.mrxs")).map(|_| ());
    assert!(e.is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
