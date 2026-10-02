//! Whole data set, Varian/Agilent VnmrJ directory: `procpar`, `fid`, `text`; parts separated by `openreadout_fuzz::BUNDLE_SEP`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(
        &openreadout_nmr::VarianReader,
        "whole_varian",
        "",
        &["procpar", "fid", "text"],
        None,
        data,
    );
});
