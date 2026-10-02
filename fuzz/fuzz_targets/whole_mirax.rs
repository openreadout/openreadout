//! Whole data set, 3DHISTECH MIRAX: `s.mrxs`, then `s/Slidedat.ini`, `s/Index.dat`, `s/Data0000.dat`; parts separated by `openreadout_fuzz::BUNDLE_SEP`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(
        &openreadout_wsi::MiraxReader,
        "whole_mirax",
        "",
        &["s.mrxs", "s/Slidedat.ini", "s/Index.dat", "s/Data0000.dat"],
        Some(0),
        data,
    );
});
