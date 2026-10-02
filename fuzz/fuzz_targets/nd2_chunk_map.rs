//! ND2 chunk-map payload parser, and the whole container open (map + rescue scan).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = openreadout_nd2::fuzzing::fuzz_chunk_map(data);
    let p = openreadout_fuzz::scratch_file("nd2-map", "nd2", data);
    let _ = openreadout_nd2::Nd2File::open(&p);
});
