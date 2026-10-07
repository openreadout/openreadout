//! Chunked compression (CZI compression 7): varint header entries, then zstd or LZ4 chunks.
//! First 3 bytes: expected length; the low bit of the first byte picks 1 or 2 bytes/sample.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (expected, payload) = openreadout_fuzz::split_expected(data);
    let bps = if expected & 1 == 0 { 2 } else { 1 };
    if let Ok(out) = openreadout_codecs::chunked_decode(payload, expected, bps) {
        assert!(expected == 0 || out.len() == expected);
    }
});
