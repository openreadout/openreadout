//! Whole EEGLAB dataset: a .set MAT-file and its .fdt samples (parts split by the bundle separator): open -> info -> vendor -> entries -> check -> table/trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(&openreadout_ephys::EeglabReader, "whole_eeglab", "r.set", &["r.set", "r.fdt"], Some(0), data);
});
