//! Whole data set, SpikeGLX stream: `.meta` text, then the `.bin` samples; parts separated by `openreadout_fuzz::BUNDLE_SEP`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(
        &openreadout_spikeglx::SpikeGlxReader,
        "whole_spikeglx",
        "",
        &["s.imec0.ap.meta", "s.imec0.ap.bin"],
        Some(1),
        data,
    );
});
