//! zstd1 (CZI compression 6): header + zstd frame + optional HiLo unshuffle.
//! First 3 bytes: expected length; the low bit of the first byte picks 1 or 2 bytes/sample.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (expected, payload) = openreadout_fuzz::split_expected(data);
    let bps = if expected & 1 == 0 { 2 } else { 1 };
    let _ = openreadout_codecs::zstd1_header(payload);
    if let Ok(out) = openreadout_codecs::zstd1_decode(payload, expected, bps) {
        assert!(expected == 0 || out.len() == expected);
    }
});
