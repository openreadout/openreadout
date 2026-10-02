//! Whole-file BioLogic EC-Lab .mpr: modules (57- and 65-byte headers), the data module column table and records, log and loop modules.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_echem::MprReader, "whole_mpr", "mpr", data);
});
