//! Whole-file CZI: open -> info -> vendor metadata -> entries -> check -> read_plane(0, c0 z0 t0).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_czi::CziReader, "whole-czi", "czi", data);
});
