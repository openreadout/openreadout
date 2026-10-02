//! JPEG XR codestreams (CZI compression 4), with and without a declared geometry.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    if let Ok(r) = openreadout_codecs::jpegxr_decode(data) {
        let _ = openreadout_codecs::jpegxr_decode_expect(data, Some((r.width, r.height)));
    }
    let _ = openreadout_codecs::jpegxr_decode_expect(data, Some((1, 1)));
});
