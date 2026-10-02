//! Whole-file TA Instruments .001: UTF-16 header, form feed, signal count, float32 records, end record.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_thermal::TaReader, "whole_ta001", "001", data);
});
