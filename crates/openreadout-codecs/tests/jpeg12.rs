//! 12-bit sequential JPEG against libjpeg-turbo: the streams in `fixtures/jpeg12/` were
//! written and decoded by libjpeg-turbo through imagecodecs (`oracle/make_jpeg12_fixtures.py`).
//! Our inverse DCT is the exact transform, libjpeg's an integer approximation, so samples may
//! differ by a grey level or two.

#![cfg(feature = "jpeg")]

use std::path::Path;

fn compare(name: &str, channels: u32) -> (usize, u16) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jpeg12");
    let data = std::fs::read(dir.join(format!("{name}.jpg"))).unwrap();
    let expected = std::fs::read(dir.join(format!("{name}.expected"))).unwrap();
    let r = openreadout_codecs::jpeg_decode(&data).unwrap();
    assert_eq!((r.channels, r.bits_per_sample), (channels, 16), "{name}");
    assert_eq!(r.data.len(), expected.len(), "{name}");
    let mut differ = 0;
    let mut worst = 0u16;
    for (a, b) in r
        .data
        .as_chunks::<2>()
        .0
        .iter()
        .zip(expected.as_chunks::<2>().0)
    {
        let (a, b) = (
            u16::from_le_bytes([a[0], a[1]]),
            u16::from_le_bytes([b[0], b[1]]),
        );
        if a != b {
            differ += 1;
            worst = worst.max(a.abs_diff(b));
        }
    }
    (differ, worst)
}

#[test]
fn twelve_bit_streams_match_libjpeg_within_two_levels() {
    for (name, channels) in [
        ("gray-37x29-q90", 1),
        ("gray-64x48-q100", 1),
        ("gray-40x24-q75-optimized", 1),
        ("ycbcr444-33x17-q95", 3),
        ("rgb-20x12-q90", 3),
    ] {
        let (differ, worst) = compare(name, channels);
        eprintln!("{name}: {differ} samples differ, at most by {worst}");
        assert!(worst <= 2, "{name}: a sample differs by {worst}");
    }
}
