//! Whole data set, FEI TIA series: `.ser`, then its `.emi` sidecar; parts separated by `openreadout_fuzz::BUNDLE_SEP`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(
        &openreadout_em::SerReader,
        "whole_ser",
        "",
        &["s_1.ser", "s.emi"],
        Some(0),
        data,
    );
});
