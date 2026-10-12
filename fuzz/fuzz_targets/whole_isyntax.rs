//! Whole-file Philips iSyntax (XML header, seek table, wavelet code blocks): open -> info -> vendor -> entries -> check -> plane reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_wsi::IsyntaxReader, "whole_isyntax", "isyntax", data);
});
