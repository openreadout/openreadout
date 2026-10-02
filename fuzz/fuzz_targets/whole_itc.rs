//! Whole-file MicroCal .itc: text header lines, injection lines, block markers and data rows: open -> info -> vendor -> entries -> check -> table/trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_biophys::ItcReader, "whole_itc", "itc", data);
});
