//! Whole-file Bruker DIFFRAC .raw (RAW1.01 and RAW4.00): headers, text and instrument records, range headers, drive records and data records.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_xrd::BrukerRawReader, "whole_bruker_raw", "raw", data);
});
