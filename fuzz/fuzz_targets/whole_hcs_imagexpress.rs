//! Whole plate, Molecular Devices ImageXpress: an `.HTD` plate description and one plane TIFF; parts separated by `openreadout_fuzz::BUNDLE_SEP`. Odd inputs open the folder (no HTD reading) instead of the HTD.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    let open = if data.len() % 2 == 0 { Some(0) } else { None };
    openreadout_fuzz::whole_bundle(
        &openreadout_hcs::ImageXpressReader,
        "whole_hcs_imagexpress",
        "p",
        &["p/BSF018292-1A.HTD", "p/BSF018292-1A_A01_w1.TIF", "p/BSF018292-1A_A02_s2_w2.TIF"],
        open,
        data,
    );
});
