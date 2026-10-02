//! Fuzz regressions for the codecs: inputs from the cargo-fuzz targets in `fuzz/` that
//! crashed a decoder (panic, heap overflow, unbounded output), kept in
//! `tests/fixtures/malformed/` and named after the target (`jpegxr-…`, `zstd0-…`, …).
//! Each must decode or fail with a `CodecError`, never panic.
//!
//! Codec targets other than JPEG XR and HiLo read a 3-byte little-endian expected length
//! from the front of the input (see `fuzz/src/lib.rs`), and so does this test.

use std::path::PathBuf;

fn fixtures() -> Vec<(String, Vec<u8>)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/malformed");
    let mut v: Vec<(String, Vec<u8>)> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&p).unwrap(),
            )
        })
        .collect();
    v.sort();
    v
}

fn split_expected(data: &[u8]) -> (usize, &[u8]) {
    if data.len() < 3 {
        return (0, data);
    }
    let n = usize::from(data[0]) | (usize::from(data[1]) << 8) | (usize::from(data[2]) << 16);
    (n, &data[3..])
}

#[test]
fn codec_fixtures_fail_cleanly() {
    let all = fixtures();
    assert!(!all.is_empty(), "no fixtures found");
    for (name, data) in all {
        let (expected, payload) = split_expected(&data);
        let r = std::panic::catch_unwind(|| match name.split('-').next().unwrap() {
            "jpegxr" => {
                let _ = openreadout_codecs::jpegxr_decode(&data);
            }
            "zstd0" => {
                let _ = openreadout_codecs::zstd_decode(payload, expected);
            }
            "zstd1" => {
                let _ = openreadout_codecs::zstd1_decode(payload, expected, 2);
            }
            "jpeg" => {
                // As fuzz_targets/codec_jpeg.rs: the prefix is the decoded-size bound.
                let max = expected.max(1 << 20);
                assert!(
                    openreadout_codecs::jpeg_decode_limited(payload, max).is_err(),
                    "{name}: a frame far larger than the bound must be refused"
                );
                let (t, d) = payload.split_at(payload.len() / 2);
                let _ = openreadout_codecs::jpeg_decode_tiff_limited(
                    d,
                    Some(t),
                    openreadout_codecs::JpegColor::AsCoded,
                    max,
                );
            }
            "lzw" => {
                let _ = openreadout_codecs::lzw_decode(payload, expected);
            }
            "zlib" => {
                let _ = openreadout_codecs::zlib_decode(payload, expected);
            }
            "jpeg2000" => {
                // As fuzz_targets/codec_jpeg2000.rs: the whole fixture is the codestream.
                let _ = openreadout_codecs::jpeg2000_decode(&data);
            }
            other => panic!("fixture {name}: unknown codec prefix {other}"),
        });
        assert!(r.is_ok(), "{name} panicked");
    }
}
