//! Whole-file BAM: BGZF blocks, the header and reference list, alignment records and auxiliary fields.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_seq::BamReader, "whole_bam", "bam", data);
});
