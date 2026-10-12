//! Whole TDT block: a .tsq index and its .tev data (parts split by the bundle separator), opened as the block directory: open -> info -> vendor -> entries -> check -> table/trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(&openreadout_ephys::TdtReader, "whole_tdt", "b", &["b/s_B1.tsq", "b/s_B1.tev"], None, data);
});
