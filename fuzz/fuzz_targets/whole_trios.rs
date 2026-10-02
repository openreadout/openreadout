//! Whole-file TA Instruments TRIOS .tri: header strings, thumbnail, step objects, typed properties, signal records and flag arrays, parallel-plate moduli.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_thermal::TriosReader, "whole_trios", "tri", data);
});
