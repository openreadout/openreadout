//! JPEG colour coding in TIFF pages (docs/formats/tiff.md § JPEG colour). The fixtures are
//! 64 × 64 crops of OpenSlide's CMU-1-Small-Region.svs (CC0-1.0) written by Bio-Formats,
//! run as a black box (see `oracle/make_tiff_jpeg_fixtures.py` for the full-size versions):
//!
//! * `bf670-rgb-jfif-64.ome.tif` — Bio-Formats 6.7.0 `-compression JPEG -tilex 32 -tiley 32`
//!   from a PNG: photometric RGB pages whose tiles are JFIF (Y, Cb, Cr, 4:2:0) streams;
//! * `bf850-ycbcr-planar-64.ome.tif` — Bio-Formats 8.5.0, same options, from an uncompressed
//!   TIFF: photometric YCbCr with separate sample planes, which is refused.
//!
//! The expected sums are tifffile 2026.9.20 + imagecodecs 2026.8.16 (libjpeg-turbo) decoding
//! the same file.

use std::path::PathBuf;

use openreadout_core::reader::{FormatReader, PlaneIndex};
use openreadout_tiff::TiffReader;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/jpeg")
        .join(name)
}

#[test]
fn bioformats_6_rgb_page_with_jfif_streams_decodes_to_rgb() {
    let mut ds = TiffReader
        .open(&fixture("bf670-rgb-jfif-64.ome.tif"))
        .unwrap();
    let plane = ds.read_plane(0, PlaneIndex::default()).unwrap();
    assert_eq!(
        (plane.width, plane.height, plane.samples_per_pixel),
        (64, 64, 3)
    );
    let mut sums = [0u64; 3];
    for px in plane.data.as_chunks::<3>().0 {
        for (s, v) in sums.iter_mut().zip(px) {
            *s += u64::from(*v);
        }
    }
    // tifffile: 753179, 410222, 560551 (means 183.9, 100.2, 136.9). Decoded as Y, Cb, Cr
    // the means would be near 128 for the two chroma samples. Chroma upsampling differs from
    // libjpeg-turbo's by at most 3 counts per sample.
    let want = [753_179u64, 410_222, 560_551];
    for (s, (got, want)) in sums.iter().zip(want).enumerate() {
        let d = got.abs_diff(want) as f64 / 4096.0;
        assert!(d < 0.5, "sample {s}: mean differs from tifffile by {d:.3}");
    }
    let rep = ds.check().unwrap();
    assert!(rep.ok);
    assert!(
        rep.findings.iter().all(|f| f.code != "unsupported_samples"),
        "{:?}",
        rep.findings
    );
}

#[test]
fn separate_ycbcr_jpeg_planes_are_refused_not_misread() {
    let mut ds = TiffReader
        .open(&fixture("bf850-ycbcr-planar-64.ome.tif"))
        .unwrap();
    let err = ds.read_plane(0, PlaneIndex::default()).unwrap_err();
    assert!(
        matches!(err, openreadout_core::Error::Unsupported { .. }),
        "{err}"
    );
    let rep = ds.check().unwrap();
    assert!(
        rep.findings.iter().any(|f| f.code == "unsupported_samples"),
        "{:?}",
        rep.findings
    );
}
