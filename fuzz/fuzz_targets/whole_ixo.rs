//! Whole-file qPCR reader: Roche LightCycler 480 .ixo object stream: open -> info -> vendor -> entries -> check -> table/trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_qpcr::IxoReader, "whole_ixo", "ixo", data);
});
