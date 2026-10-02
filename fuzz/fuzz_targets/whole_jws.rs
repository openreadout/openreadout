//! Whole-file JASCO .jws: compound-file streams (DataInfo, Y-Data, X-Data, records) and the flat L~S container: open -> info -> vendor -> entries -> check -> trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_spectro::JwsReader, "whole_jws", "jws", data);
});
