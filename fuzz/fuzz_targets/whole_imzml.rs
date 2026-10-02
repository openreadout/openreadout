//! Whole data set, imzML: `.imzML` XML, then the `.ibd` binary; parts separated by `openreadout_fuzz::BUNDLE_SEP`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(
        &openreadout_mzml::ImzmlReader,
        "whole_imzml",
        "",
        &["s.imzML", "s.ibd"],
        Some(0),
        data,
    );
});
