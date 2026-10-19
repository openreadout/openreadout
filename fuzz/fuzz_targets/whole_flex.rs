//! Whole data set, Bruker flex MALDI-TOF spot: `0_A1/1/1SLin/acqus`, then `0_A1/1/1SLin/fid`;
//! parts separated by `openreadout_fuzz::BUNDLE_SEP`. Opens the run directory above the spot.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(
        &openreadout_bruker_tims::BrukerFlexReader,
        "whole_flex",
        "",
        &["0_A1/1/1SLin/acqus", "0_A1/1/1SLin/fid"],
        None,
        data,
    );
});
