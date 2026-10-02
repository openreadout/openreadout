//! Whole-file Sartorius Octet .frd: the result XML, base64 float32 step arrays, point counts, step table.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_biophys::OctetReader, "whole_octet", "frd", data);
});
