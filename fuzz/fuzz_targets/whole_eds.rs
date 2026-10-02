//! Whole-file qPCR reader: Applied Biosystems .eds (zip: SDS, 7500 and JSON layouts): open -> info -> vendor -> entries -> check -> table/trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_qpcr::EdsReader, "whole_eds", "eds", data);
});
