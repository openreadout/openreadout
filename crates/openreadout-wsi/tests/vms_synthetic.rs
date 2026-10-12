//! The synthetic VMS fixture (`oracle/make_vms_fixtures.py`): geometry, level 0 read interval by
//! interval equal to each JPEG decoded whole, the map level, attachments, `check`, and refusals
//! of damaged copies.

use std::path::{Path, PathBuf};

use openreadout_core::{FormatReader, PlaneIndex, Region};
use openreadout_wsi::VmsReader;

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vms-synthetic")
}

fn decode(name: &str) -> openreadout_codecs::Raster {
    openreadout_codecs::jpeg_decode(&std::fs::read(dir().join(name)).unwrap()).unwrap()
}

#[test]
fn geometry() {
    let ds = VmsReader.open(&dir().join("synthetic.vms")).unwrap();
    let info = ds.info().unwrap();
    let im = &info.images[0];
    assert_eq!((im.size_x, im.size_y), (96, 40));
    let sizes: Vec<(u32, u32)> = im
        .resolution_levels
        .iter()
        .map(|l| (l.size_x, l.size_y))
        .collect();
    assert_eq!(
        sizes,
        [(96, 40), (48, 20), (24, 10), (12, 5), (6, 2), (3, 1)]
    );
    assert_eq!(im.physical_size.x, Some(0.5));
    assert_eq!(
        im.objective.as_ref().unwrap().nominal_magnification,
        Some(20.0)
    );
    assert_eq!(im.name.as_deref(), Some("synthetic"));
    assert_eq!(im.extra["vms"]["map_downsample"], 8);
}

#[test]
fn level0_equals_the_jpegs_decoded_whole() {
    let mut ds = VmsReader.open(&dir().join("synthetic.vms")).unwrap();
    let p = ds.read_plane(0, PlaneIndex::default()).unwrap();
    let cells = [
        ("synthetic.jpg", 0, 0),
        ("synthetic(1,0).jpg", 64, 0),
        ("synthetic(0,1).jpg", 0, 24),
        ("synthetic(1,1).jpg", 64, 24),
    ];
    for (name, ox, oy) in cells {
        let r = decode(name);
        for y in 0..r.height {
            for x in 0..r.width {
                let s = ((y * r.width + x) * 3) as usize;
                let d = (((oy + y) * 96 + ox + x) * 3) as usize;
                assert_eq!(&p.data[d..d + 3], &r.data[s..s + 3], "{name} ({x},{y})");
            }
        }
    }
    let r = ds
        .read_region(0, PlaneIndex::default(), 0, Region::new(30, 10, 50, 25))
        .unwrap();
    for y in 0..25u32 {
        let a = ((y * 50) * 3) as usize;
        let b = (((10 + y) * 96 + 30) * 3) as usize;
        assert_eq!(&r.data[a..a + 150], &p.data[b..b + 150]);
    }
    // The map at full size is level 3.
    let m = ds.read_plane_level(0, PlaneIndex::default(), 3).unwrap();
    assert_eq!(m.data, decode("synthetic_map2.jpg").data);
    for level in 1..6 {
        let l = ds
            .read_plane_level(0, PlaneIndex::default(), level)
            .unwrap();
        assert_eq!(l.data.len(), (l.width * l.height * 3) as usize);
    }
}

#[test]
fn attachments_and_check() {
    let mut ds = VmsReader.open(&dir().join("synthetic.vms")).unwrap();
    let names: Vec<String> = ds
        .attachments()
        .unwrap()
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert_eq!(names, ["macro", "map", "optimisation file"]);
    let r = ds.check().unwrap();
    assert!(r.findings.is_empty(), "{:?}", r.findings);
    assert_eq!(ds.member_files().len(), 7);
}

/// A copy of the fixture with one file replaced.
fn copy_with(tag: &str, name: &str, bytes: &[u8]) -> PathBuf {
    let d = std::env::temp_dir().join(format!("vms-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    for e in std::fs::read_dir(dir()).unwrap() {
        let p = e.unwrap().path();
        std::fs::copy(&p, d.join(p.file_name().unwrap())).unwrap();
    }
    std::fs::write(d.join(name), bytes).unwrap();
    d.join("synthetic.vms")
}

#[test]
fn damaged_copies_are_refused() {
    // An optimisation offset that points into the middle of a row: the row does not follow its
    // restart marker (exit 4).
    let mut opt = std::fs::read(dir().join("synthetic.opt")).unwrap();
    let v = u64::from_le_bytes(opt[40..48].try_into().unwrap()) + 1;
    opt[40..48].copy_from_slice(&v.to_le_bytes());
    let p = copy_with("opt", "synthetic.opt", &opt);
    let mut ds = VmsReader.open(&p).unwrap();
    let e = ds.read_plane(0, PlaneIndex::default()).unwrap_err();
    assert_eq!(e.exit_code(), 4, "{e}");
    // Without the optimisation file the rows are found by scanning.
    let p = copy_with("noopt", "synthetic.opt", b"");
    let mut ds = VmsReader.open(&p).unwrap();
    assert!(ds.read_plane(0, PlaneIndex::default()).is_ok());
    // A cut image: inside its header the slide does not open, inside its data the rows are
    // missing (exit 4 both).
    let jpg = std::fs::read(dir().join("synthetic(1,0).jpg")).unwrap();
    let p = copy_with("cuthead", "synthetic(1,0).jpg", &jpg[..300]);
    let Err(e) = VmsReader.open(&p) else {
        panic!("a cut header must be refused")
    };
    assert_eq!(e.exit_code(), 4);
    let p = copy_with("cut", "synthetic(1,0).jpg", &jpg[..jpg.len() - 200]);
    let mut ds = VmsReader.open(&p).unwrap();
    assert_eq!(
        ds.read_plane(0, PlaneIndex::default())
            .unwrap_err()
            .exit_code(),
        4
    );
    // VMU settings and several focal layers are refused (exit 6).
    let vms = std::fs::read_to_string(dir().join("synthetic.vms")).unwrap();
    for (tag, text) in [
        (
            "vmu",
            vms.replace(
                "[Virtual Microscope Specimen]",
                "[Uncompressed Virtual Microscope Specimen]",
            ),
        ),
        ("layers", vms.replace("NoLayers=1", "NoLayers=3")),
    ] {
        let p = copy_with(tag, "synthetic.vms", text.as_bytes());
        let Err(e) = VmsReader.open(&p) else {
            panic!("{tag} must be refused")
        };
        assert_eq!(e.exit_code(), 6, "{tag}: {e}");
    }
}
