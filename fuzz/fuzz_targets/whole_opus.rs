//! Whole-file Bruker OPUS file: directory, parameter blocks, data/status pairing: open -> info -> vendor -> entries -> check -> plane/table/trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_spectro::OpusReader, "whole_opus", "0", data);
});
