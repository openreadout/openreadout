//! Whole-file Rigaku .rasx: zip container, profiles and measurement conditions.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_xrd::RasxReader, "whole_rasx", "rasx", data);
});
