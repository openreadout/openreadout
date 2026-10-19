//! Whole-file CRAM: file definition, header container, container and slice headers, block
//! compression methods, compression header encodings, record decoding and base restoration.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_seq::CramReader, "whole_cram", "cram", data);
});
