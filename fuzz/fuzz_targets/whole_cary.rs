//! Whole-file Agilent (Varian) Cary .dsw/.bsw/.bsk: the store chain, spectrum and baseline stores with their texts: open -> info -> vendor -> entries -> check -> trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_spectro::CaryReader, "whole_cary", "dsw", data);
});
