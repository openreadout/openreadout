//! Whole-file Bio-Rad ddPCR CSV exports: results layouts (QuantaSoft, QX Manager), untitled trailing cells, `No Call`, amplitude files with `Target Value of` preambles: open -> info -> vendor -> entries -> check -> table reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_benchtop::fuzz_ddpcr(data);
    openreadout_fuzz::whole_file(&openreadout_benchtop::DdpcrReader, "whole_ddpcr", "csv", data);
});
