//! CRAM entropy codecs on raw bytes: rANS 4x8, rANS Nx16 (stripe, pack, run length), the
//! adaptive arithmetic coder, fqzcomp and the name tokeniser. The first byte picks the codec.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&m, body)) = data.split_first() else {
        return;
    };
    let _ = openreadout_seq::cram::decode_codec(4 + m % 5, body);
});
