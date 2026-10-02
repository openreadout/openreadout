//! Whole data set, Magritek Spinsolve experiment directory: `acqu.par`, `data.1d`, `proc.par`, `processing.script`, `spectrum.1d`; parts separated by `openreadout_fuzz::BUNDLE_SEP`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(
        &openreadout_nmr::SpinsolveReader,
        "whole_spinsolve",
        "",
        &["acqu.par", "data.1d", "proc.par", "processing.script", "spectrum.1d"],
        None,
        data,
    );
});
