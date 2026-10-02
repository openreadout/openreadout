//! Whole data set, Bruker experiment directory: `acqus`, `fid`, `pdata/1/procs`, `pdata/1/1r`, `acqu2s`, `ser`; parts separated by `openreadout_fuzz::BUNDLE_SEP`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(
        &openreadout_nmr::BrukerReader,
        "whole_bruker",
        "",
        &["acqus", "fid", "pdata/1/procs", "pdata/1/1r", "acqu2s", "ser"],
        None,
        data,
    );
});
