//! Whole-file GenePix Results .gpr: ATF header records, column titles and the feature table: open -> info -> vendor -> check -> table reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_biophys::GprReader, "whole_gpr", "gpr", data);
});
