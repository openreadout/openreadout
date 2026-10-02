//! Whole-file Axon Text File (ATF 1.0): open -> info -> vendor -> entries -> check -> trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_abf::AtfReader, "whole_atf", "atf", data);
});
