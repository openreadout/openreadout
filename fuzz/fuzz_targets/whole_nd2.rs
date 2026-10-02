//! Whole-file ND2: open -> info -> vendor metadata -> entries -> check -> read_plane(0, c0 z0 t0).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_nd2::Nd2Reader, "whole-nd2", "nd2", data);
});
