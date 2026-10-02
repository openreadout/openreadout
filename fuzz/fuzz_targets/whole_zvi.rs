//! Whole-file Zeiss AxioVision ZVI (OLE2 compound document): open -> info -> vendor -> entries -> check -> plane/table/trace/spectrum reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_zvi::ZviReader, "whole_zvi", "zvi", data);
});
