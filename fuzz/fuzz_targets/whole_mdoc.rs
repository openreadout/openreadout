//! Whole-file SerialEM `.mdoc`: sections, records, the section table.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_em::MdocReader, "whole_mdoc", "mdoc", data);
});
