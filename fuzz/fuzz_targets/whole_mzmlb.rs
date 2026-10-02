//! Whole-file mzMLb (mzML in HDF5): open -> info -> vendor -> entries -> check -> reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(
        &openreadout_mzml::MzmlbReader,
        "whole_mzmlb",
        "mzMLb",
        data,
    );
});
