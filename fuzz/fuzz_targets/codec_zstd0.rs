//! zstd (CZI compression 5). First 3 bytes: expected decoded length.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (expected, payload) = openreadout_fuzz::split_expected(data);
    if let Ok(out) = openreadout_codecs::zstd_decode(payload, expected) {
        assert!(expected == 0 || out.len() == expected);
    }
});
