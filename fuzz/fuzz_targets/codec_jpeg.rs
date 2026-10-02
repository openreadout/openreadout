//! JPEG (CZI compression 1, TIFF compression 7 with and without JPEGTables, VSI/ETS tiles).
//! First 3 bytes: the decoded-size bound the readers derive from the tile geometry (see
//! `openreadout_fuzz::split_expected`); the unbounded entry point is also fed small inputs.
//! The marker scanner the TIFF reader uses to pick the colour transform (`jpeg_markers`) is
//! fed the same splits.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (max, payload) = openreadout_fuzz::split_expected(data);
    let max = max.max(1 << 20);
    let _ = openreadout_codecs::jpeg_decode_limited(payload, max);
    let _ = openreadout_codecs::jpeg_markers(payload, None).map(|m| m.coded_color());
    // TIFF: the first half as the tables stream, the rest as the chunk.
    let (t, d) = payload.split_at(payload.len() / 2);
    let _ = openreadout_codecs::jpeg_markers(d, Some(t));
    for color in [
        openreadout_codecs::JpegColor::AsCoded,
        openreadout_codecs::JpegColor::ToRgb,
    ] {
        let _ = openreadout_codecs::jpeg_decode_tiff_limited(d, Some(t), color, max);
    }
});
