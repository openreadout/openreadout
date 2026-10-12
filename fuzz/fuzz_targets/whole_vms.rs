//! Whole data set, Hamamatsu VMS: `s.vms`, the four grid JPEGs, `s_map2.jpg`, `s.opt`, `s_macro.jpg`; parts separated by `openreadout_fuzz::BUNDLE_SEP`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(
        &openreadout_wsi::VmsReader,
        "whole_vms",
        "",
        &["s.vms", "s.jpg", "s(1,0).jpg", "s(0,1).jpg", "s(1,1).jpg", "s_map2.jpg", "s.opt", "s_macro.jpg"],
        Some(0),
        data,
    );
});
