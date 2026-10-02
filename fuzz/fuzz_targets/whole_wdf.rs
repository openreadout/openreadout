//! Whole-file Renishaw WiRE .wdf: block chain, origin lists, map grid, property sets, white-light EXIF: open -> info -> vendor -> entries -> check -> plane/table/trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_spectro::WdfReader, "whole_wdf", "wdf", data);
});
