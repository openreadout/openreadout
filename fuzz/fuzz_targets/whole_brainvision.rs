//! Whole BrainVision recording: a .vhdr header, its .vmrk markers and the .eeg samples (parts split by the bundle separator): open -> info -> vendor -> entries -> check -> table/trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(&openreadout_ephys::BrainVisionReader, "whole_brainvision", "r.vhdr", &["r.vhdr", "r.vmrk", "r.eeg"], Some(0), data);
});
