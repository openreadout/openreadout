//! Whole-file chromatography reader: Waters Empower ASCII export (.arw: quoted header names and values, then time/value rows): open -> info -> vendor -> entries -> check -> trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_chrom::EmpowerArwReader, "whole_empower_arw", "arw", data);
});
