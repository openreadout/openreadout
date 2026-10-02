//! Whole-file BioLogic EC-Lab .mpt: header block, column labels, rows (decimal comma, missing columns).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_echem::MptReader, "whole_mpt", "mpt", data);
});
