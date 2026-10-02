//! Whole-file Rigaku .ras: header blocks, data blocks and counts.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_xrd::RasReader, "whole_ras", "ras", data);
});
