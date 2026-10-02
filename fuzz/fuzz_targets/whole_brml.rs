//! Whole-file Bruker .brml: zip container, DataContainer and RawData XML, data views and Datum rows.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_xrd::BrmlReader, "whole_brml", "brml", data);
});
