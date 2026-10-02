//! Whole data set, Waters MassLynx `.raw` directory: `_HEADER.TXT`, `_extern.inf`, `_FUNCTNS.INF`, `_FUNC001.IDX`, `_FUNC001.DAT`, `_CHROMS.INF`, `_CHRO001.DAT`; parts separated by `openreadout_fuzz::BUNDLE_SEP`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(
        &openreadout_waters::WatersRawReader,
        "whole_waters",
        "s.raw",
        &["s.raw/_HEADER.TXT", "s.raw/_extern.inf", "s.raw/_FUNCTNS.INF", "s.raw/_FUNC001.IDX", "s.raw/_FUNC001.DAT", "s.raw/_CHROMS.INF", "s.raw/_CHRO001.DAT"],
        None,
        data,
    );
});
