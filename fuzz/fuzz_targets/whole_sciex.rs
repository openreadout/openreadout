//! Whole data set, Sciex `.wiff` + `.wiff.scan`: the compound file (method, sample, scan index)
//! and its scan-data companion; parts separated by `openreadout_fuzz::BUNDLE_SEP`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(
        &openreadout_sciex::SciexWiffReader,
        "whole_sciex",
        "",
        &["s.wiff", "s.wiff.scan"],
        Some(0),
        data,
    );
});
