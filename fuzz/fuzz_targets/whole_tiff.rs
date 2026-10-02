//! Whole-file TIFF / OME-TIFF / BigTIFF: open -> info -> vendor -> entries -> check -> plane/table/trace/spectrum reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_tiff::TiffReader, "whole_tiff", "tif", data);
});
