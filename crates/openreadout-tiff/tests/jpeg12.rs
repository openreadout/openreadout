//! 12-bit JPEG pages (compression 7, BitsPerSample 12). The fixtures in `fixtures/jpeg12/` were
//! written by tifffile 2026.9.20 + imagecodecs 2026.8.16 (libjpeg-turbo) and `<name>.expected`
//! is tifffile's decoded page (`oracle/make_jpeg12_fixtures.py`). Our inverse DCT is the exact
//! transform and libjpeg's an integer approximation, so samples may differ by a grey level or two.

use std::path::PathBuf;

use openreadout_core::pixel::PixelType;
use openreadout_core::reader::{FormatReader, PlaneIndex};
use openreadout_tiff::TiffReader;

fn compare(name: &str, spp: u32) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jpeg12");
    let mut ds = TiffReader.open(&dir.join(format!("{name}.tif"))).unwrap();
    let plane = ds.read_plane(0, PlaneIndex::default()).unwrap();
    assert_eq!(plane.pixel_type, PixelType::Uint16, "{name}");
    assert_eq!(plane.samples_per_pixel, spp, "{name}");
    let want = std::fs::read(dir.join(format!("{name}.expected"))).unwrap();
    assert_eq!(plane.data.len(), want.len(), "{name}");
    let worst = plane
        .data
        .as_chunks::<2>()
        .0
        .iter()
        .zip(want.as_chunks::<2>().0)
        .map(|(a, b)| u16::from_le_bytes([a[0], a[1]]).abs_diff(u16::from_le_bytes([b[0], b[1]])))
        .max()
        .unwrap_or(0);
    assert!(
        worst <= 2,
        "{name}: a sample differs from tifffile by {worst}"
    );
    assert!(ds.check().unwrap().ok, "{name}");
}

#[test]
fn twelve_bit_grey_tiles_match_tifffile() {
    compare("gray12-tiled", 1);
}

#[test]
fn twelve_bit_rgb_strips_match_tifffile() {
    compare("rgb12-strips-ascoded", 3);
}
