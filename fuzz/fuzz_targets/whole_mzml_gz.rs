//! Whole-file gzip-compressed mzML (`.mzML.gz`): gzip header/trailer parsing, restart points,
//! then the mzML reader on the decompressed view: open -> info -> check -> reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(
        &openreadout_mzml::MzmlReader,
        "whole_mzml_gz",
        "mzML.gz",
        data,
    );
});
