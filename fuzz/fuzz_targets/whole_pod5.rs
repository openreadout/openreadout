//! Whole-file Oxford Nanopore POD5: signature, FlatBuffer footer, the Arrow IPC reads, run-info and signal tables, VBZ signal rows.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_seq::Pod5Reader, "whole_pod5", "pod5", data);
});
