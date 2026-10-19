//! Whole-file HORIBA LabSpec 6 (.l6s, .l6m): record runs, float32 arrays with counts from the file, maps: open -> info -> vendor -> entries -> check -> table/trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_spectro::LabSpecReader, "whole_labspec", "l6s", data);
});
