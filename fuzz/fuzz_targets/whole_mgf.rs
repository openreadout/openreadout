//! Whole-file MGF peak lists: line scan, file and block parameters, charges, retention-time ranges, peak lines: open -> info -> vendor -> entries -> check -> spectrum reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_mzml::MgfReader, "whole_mgf", "mgf", data);
});
