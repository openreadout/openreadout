//! Whole-file National Instruments TDMS: segment lead-ins, metadata (objects, raw-data indexes,
//! DAQmx scalers, properties), chunk placement, values and scaling: open -> info -> vendor ->
//! entries -> check -> trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_tdms::TdmsReader, "whole_tdms", "tdms", data);
});
