//! Whole-file DeltaVision: header, extended header, section order: open -> info -> vendor -> entries -> check -> plane/table/trace/spectrum reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_dv::DvReader, "whole_dv", "dv", data);
});
