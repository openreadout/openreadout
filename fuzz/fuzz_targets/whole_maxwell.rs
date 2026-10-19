//! Whole MaxWell .raw.h5 file: open -> info -> vendor -> entries -> check -> trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_ephys::MaxwellReader, "whole_maxwell", "raw.h5", data);
});
