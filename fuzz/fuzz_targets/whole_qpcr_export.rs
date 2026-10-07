//! Whole-file qPCR results export as text (Bio-Rad CFX Quantification Cq Results CSV, Applied Biosystems text Results): open -> info -> vendor -> entries -> check -> table/trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_qpcr::ExportReader, "whole_qpcr_export", "csv", data);
});
