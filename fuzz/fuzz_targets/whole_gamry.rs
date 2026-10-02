//! Whole-file Gamry .DTA: EXPLAIN header lines, NOTES blocks and TABLE blocks.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_echem::GamryReader, "whole_gamry", "DTA", data);
});
