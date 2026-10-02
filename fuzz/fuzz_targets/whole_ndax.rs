//! Whole-file Neware .ndax: zip members, .ndc pages (count, bitmap, CRC-32), run information and auxiliary files.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_echem::NdaxReader, "whole_ndax", "ndax", data);
});
