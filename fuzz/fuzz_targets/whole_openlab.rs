//! Whole data set, Agilent OpenLab CDS injection: `a.dx` (zip: manifest, signal and instrument-curve parts), its `a.rx` results package and a sequence `.acaml`, in one folder; parts separated by `openreadout_fuzz::BUNDLE_SEP`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(
        &openreadout_chrom::OpenLabReader,
        "whole_openlab",
        "set.rslt/a.dx",
        &["set.rslt/a.dx", "set.rslt/a.rx", "set.rslt/set.acaml"],
        Some(0),
        data,
    );
});
