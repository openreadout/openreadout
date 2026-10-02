//! LZMA2 (raw chunk stream, as in Chromeleon's compressed method, audit-trail and object
//! blobs). First 3 bytes: expected decoded length.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (expected, payload) = openreadout_fuzz::split_expected(data);
    if let Ok((out, used)) = openreadout_codecs::lzma2_decode(payload, expected) {
        assert!(expected == 0 || out.len() == expected);
        assert!(used <= payload.len());
    }
});
