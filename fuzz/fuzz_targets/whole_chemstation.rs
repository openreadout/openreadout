//! Whole data set, Agilent ChemStation `.D` directory: `dad1A.ch`, `dad1.uv`, `MSD1.MS`; parts separated by `openreadout_fuzz::BUNDLE_SEP`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(
        &openreadout_chrom::ChemStationReader,
        "whole_chemstation",
        "s.D",
        &["s.D/dad1A.ch", "s.D/dad1.uv", "s.D/MSD1.MS"],
        None,
        data,
    );
});
