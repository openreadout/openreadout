//! Whole-file MC_DataTool binary export (.raw with header): open -> info -> vendor -> entries -> check -> trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_ephys::McsRawReader, "whole_mcs_raw", "raw", data);
});
