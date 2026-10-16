//! HDF5 bitshuffle filter (32008) with LZ4: the chunk framing and the bit transpose. First 3
//! bytes: expected decoded length; the next byte picks the element size (1, 2, 4 or 8).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (expected, payload) = openreadout_fuzz::split_expected(data);
    let Some((&es, chunk)) = payload.split_first() else {
        return;
    };
    let es = 1usize << (es % 4);
    if let Ok(out) = openreadout_codecs::bitshuffle_decode(chunk, es, 2, 0, expected) {
        assert!(expected == 0 || out.len() == expected);
    }
    let _ = openreadout_codecs::bitshuffle_decode(chunk, es, 0, 64, 0);
});
