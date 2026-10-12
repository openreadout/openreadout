//! The synthetic iSyntax fixture (`oracle/make_isyntax_fixtures.py`): geometry, pixels equal to
//! libisyntax's (values pinned from `make_isyntax_fixtures.py --check`), a missing tile read as
//! white, attachments, `check`, and refusals of truncated or altered copies.

use std::path::{Path, PathBuf};

use openreadout_core::{FormatReader, PlaneIndex, Region};
use openreadout_wsi::IsyntaxReader;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/isyntax-synthetic.isyntax")
}

fn px(p: &openreadout_core::Plane, x: u32, y: u32) -> [u8; 3] {
    let o = ((y * p.width + x) * 3) as usize;
    [p.data[o], p.data[o + 1], p.data[o + 2]]
}

fn sum(p: &openreadout_core::Plane) -> u64 {
    p.data.iter().map(|&v| u64::from(v)).sum()
}

#[test]
fn geometry_and_metadata() {
    let ds = IsyntaxReader.open(&fixture()).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("5.0"));
    let im = &info.images[0];
    assert_eq!((im.size_x, im.size_y, im.samples_per_pixel), (46, 30, 3));
    let sizes: Vec<(u32, u32)> = im
        .resolution_levels
        .iter()
        .map(|l| (l.size_x, l.size_y))
        .collect();
    assert_eq!(sizes, vec![(46, 30), (23, 15)]);
    assert_eq!(im.physical_size.x, Some(0.25));
    assert_eq!(im.acquired_at.as_deref(), Some("2026-10-12T12:00:00"));
    let m = &im.extra["isyntax"];
    assert_eq!(m["padding"], 9);
    assert_eq!(m["code_blocks"], 54);
    assert_eq!(m["tiles_per_scale"], serde_json::json!([12, 3]));
    assert_eq!(m["barcode"], "SYNTH-1");
}

#[test]
fn pixels_equal_libisyntax() {
    let mut ds = IsyntaxReader.open(&fixture()).unwrap();
    let l0 = ds.read_plane(0, PlaneIndex::default()).unwrap();
    assert_eq!(sum(&l0), 716_777);
    assert_eq!(px(&l0, 0, 0), [101, 136, 138]);
    assert_eq!(px(&l0, 20, 10), [158, 171, 170]);
    assert_eq!(px(&l0, 0, 29), [73, 117, 110]);
    assert_eq!(px(&l0, 45, 0), [147, 186, 191]);
    // The top-scale tile (1, 1) and its children do not exist: white.
    assert_eq!(px(&l0, 45, 29), [255, 255, 255]);
    let l1 = ds.read_plane_level(0, PlaneIndex::default(), 1).unwrap();
    assert_eq!(sum(&l1), 178_267);
    assert_eq!(px(&l1, 20, 10), [170, 190, 219]);
    // A region equals the same rectangle of the whole level.
    let r = ds
        .read_region(0, PlaneIndex::default(), 0, Region::new(10, 5, 20, 20))
        .unwrap();
    for y in 0..20 {
        for x in 0..20 {
            assert_eq!(px(&r, x, y), px(&l0, 10 + x, 5 + y));
        }
    }
    assert!(ds.read_plane_level(0, PlaneIndex::default(), 2).is_err());
}

#[test]
fn attachments_and_check() {
    let mut ds = IsyntaxReader.open(&fixture()).unwrap();
    let names: Vec<String> = ds
        .attachments()
        .unwrap()
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert_eq!(names, ["label", "macro", "icc profile (wsi)"]);
    assert!(ds.read_attachment(0).unwrap().starts_with(&[0xFF, 0xD8]));
    assert_eq!(
        ds.read_attachment(2).unwrap(),
        b"synthetic icc profile stand-in"
    );
    let r = ds.check().unwrap();
    assert!(r.findings.is_empty(), "{:?}", r.findings);
}

#[test]
fn damaged_copies_are_refused() {
    let bytes = std::fs::read(fixture()).unwrap();
    let dir = std::env::temp_dir().join(format!("isyntax-damaged-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let eot = bytes.iter().position(|&b| b == 4).unwrap();
    // Cut inside the XML header, right after it, and inside the code blocks.
    for (k, cut) in [eot / 2, eot + 4, bytes.len() - 100]
        .into_iter()
        .enumerate()
    {
        let p = dir.join(format!("cut{k}.isyntax"));
        std::fs::write(&p, &bytes[..cut]).unwrap();
        let res = IsyntaxReader.open(&p).and_then(|mut ds| {
            let info = ds.info()?;
            let (w, h) = (info.images[0].size_x, info.images[0].size_y);
            ds.read_region(0, PlaneIndex::default(), 0, Region::new(0, 0, w, h))
        });
        let e = res.expect_err("a cut file must be refused");
        assert_eq!(e.exit_code(), 4, "cut {k}: {e}");
    }
    // Data model 100 is refused with exit 6.
    let text = String::from_utf8_lossy(&bytes[..eot]).replace(">5.0<", ">100.5<");
    let mut v100 = text.into_bytes();
    v100.extend_from_slice(&bytes[eot..]);
    let p = dir.join("v100.isyntax");
    std::fs::write(&p, &v100).unwrap();
    let Err(e) = IsyntaxReader.open(&p) else {
        panic!("data model 100 must be refused")
    };
    assert_eq!(e.exit_code(), 6);
    let _ = std::fs::remove_dir_all(&dir);
}
