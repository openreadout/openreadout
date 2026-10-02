//! Whole-file Neware .nda: version byte, data section of version 29, BTS 9.0/9.1 record runs.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_echem::NdaReader, "whole_nda", "nda", data);
});
