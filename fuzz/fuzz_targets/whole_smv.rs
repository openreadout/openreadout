//! Whole-file SMV (ADSC-style `.img`) diffraction image.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_diffraction::SmvReader, "whole_smv", "img", data);
});
