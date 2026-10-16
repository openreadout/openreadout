//! Whole-file NanoDrop exports (8000 report, Eight export with spectra, One CSV, One UV-Vis spectrum blocks): layout sniffing, delimited lines, numbers, dates, units: open -> info -> vendor -> entries -> check -> table/trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_benchtop::fuzz_nanodrop(data);
    openreadout_fuzz::whole_file(&openreadout_benchtop::NanoDropReader, "whole_nanodrop", "txt", data);
});
