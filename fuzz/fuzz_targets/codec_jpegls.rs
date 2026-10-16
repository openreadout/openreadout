//! JPEG-LS streams (DICOM transfer syntaxes 1.2.840.10008.1.2.4.80 and .81).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = openreadout_codecs::jpegls_decode_limited(data, 64 << 20);
});
