//! Whole data set, Bruker BAF: `s.d/analysis.baf`, then `s.d/Calibrator.ami`; parts separated by `openreadout_fuzz::BUNDLE_SEP`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(
        &openreadout_bruker_tims::BrukerBafReader,
        "whole_baf",
        "s.d",
        &["s.d/analysis.baf", "s.d/Calibrator.ami"],
        None,
        data,
    );
});
