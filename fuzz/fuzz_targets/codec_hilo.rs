//! HiLo byte unshuffle: output is a permutation of the input.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let out = openreadout_codecs::unshuffle_hilo(data);
    assert_eq!(out.len(), data.len());
});
