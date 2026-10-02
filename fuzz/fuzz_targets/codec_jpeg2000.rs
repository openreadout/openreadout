//! JPEG 2000 codestreams / JP2 (legacy ND2 frames).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = openreadout_codecs::jpeg2000_decode(data);
});
