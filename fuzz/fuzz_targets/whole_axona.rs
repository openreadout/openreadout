//! Whole Axona DacqUSB session: a .set file, the raw .bin and one tetrode file (parts split by the bundle separator): open -> info -> vendor -> entries -> check -> table/trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(&openreadout_ephys::AxonaReader, "whole_axona", "s.set", &["s.set", "s.bin", "s.1"], Some(0), data);
});
