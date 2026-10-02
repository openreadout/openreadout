//! Whole plate, Revvity/PerkinElmer Harmony export: `Images/Index.idx.xml` plus one plane TIFF; parts separated by `openreadout_fuzz::BUNDLE_SEP`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(
        &openreadout_hcs::HarmonyReader,
        "whole_hcs_harmony",
        "",
        &["Images/Index.idx.xml", "Images/r03c07f01p01-ch1sk1fk1fl1.tiff"],
        Some(0),
        data,
    );
});
