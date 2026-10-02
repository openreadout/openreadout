//! Whole-file Bio-Rad CFX .pcrd: detection and the refusal path (the container is encrypted; exit 6).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_qpcr::PcrdReader, "whole_pcrd", "pcrd", data);
});
