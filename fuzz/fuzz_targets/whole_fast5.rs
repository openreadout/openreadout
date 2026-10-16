//! Whole-file Oxford Nanopore FAST5 (HDF5): read groups, channel and tracking attributes, deflate and VBZ signal chunks.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_seq::Fast5Reader, "whole_fast5", "fast5", data);
});
