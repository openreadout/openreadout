//! JPEG XL codestreams / containers (TIFF compression 50002/52546 chunks).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = openreadout_codecs::jpegxl_decode_limited(data, 64 << 20);
});
