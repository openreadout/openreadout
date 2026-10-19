//! Whole-file QuantaSoft `.qlp` plates: the IFD chain, setup records, droplet records and class bytes, with sizes taken from the file: open -> info -> vendor -> entries -> check -> table reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_benchtop::QlpReader, "whole_qlp", "qlp", data);
});
