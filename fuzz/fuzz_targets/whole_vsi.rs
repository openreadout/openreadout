//! Whole data set, cellSens VSI: `.vsi`, then `_s_/stack1/frame_t_0.ets`; parts separated by `openreadout_fuzz::BUNDLE_SEP`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(
        &openreadout_vsi::VsiReader,
        "whole_vsi",
        "",
        &["s.vsi", "_s_/stack1/frame_t_0.ets"],
        Some(0),
        data,
    );
});
