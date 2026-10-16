//! Whole-file Agilent 2100 Bioanalyzer XML export: samples, peaks, standard curve, base64 signals, sizing: open -> info -> vendor -> check -> trace and table reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_ce::BioanalyzerReader, "whole_bioanalyzer", "xml", data);
});
