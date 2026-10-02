//! WebP files (TIFF compression 50001 chunks).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = openreadout_codecs::webp_decode_limited(data, 64 << 20);
});
