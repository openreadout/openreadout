//! Whole-file Cytiva ÄKTA UNICORN 3-5 .res: header, block directory, curve and event descriptors, text blocks: open -> info -> vendor -> entries -> check -> table/trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_fplc::UnicornResReader, "whole_unicorn_res", "res", data);
});
