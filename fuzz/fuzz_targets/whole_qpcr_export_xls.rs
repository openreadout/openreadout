//! Whole-file qPCR results export as an Excel 97 workbook (Applied Biosystems Results .xls through calamine): open -> info -> vendor -> entries -> check -> table/trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_qpcr::ExportReader, "whole_qpcr_export_xls", "xls", data);
});
