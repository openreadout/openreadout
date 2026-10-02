//! Whole data set, Bruker timsTOF `.d`: `analysis.tdf` (SQLite), then `analysis.tdf_bin`; parts separated by `openreadout_fuzz::BUNDLE_SEP`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(
        &openreadout_bruker_tims::BrukerTimsReader,
        "whole_tims",
        "",
        &["s.d/analysis.tdf", "s.d/analysis.tdf_bin"],
        Some(0),
        data,
    );
});
