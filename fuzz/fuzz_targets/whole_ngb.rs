//! Whole-file NETZSCH .ngb: zip container, stream section directories, the record grammar, tables and channel runs.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_thermal::NgbReader, "whole_ngb", "ngb-ss3", data);
});
