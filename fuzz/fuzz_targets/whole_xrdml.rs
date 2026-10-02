//! Whole-file PANalytical XRDML: XML measurements, scans, positions, intensities/counts and attenuation factors.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_xrd::XrdmlReader, "whole_xrdml", "xrdml", data);
});
