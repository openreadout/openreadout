//! Whole-data-set Bruker ESP/WinEPR: a .par parameter file and its .spc data (parts split by the bundle separator).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(&openreadout_epr::EspReader, "whole_esp", "s.par", &["s.par", "s.spc"], Some(0), data);
});
