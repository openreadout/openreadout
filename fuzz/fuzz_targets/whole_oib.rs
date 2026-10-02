//! Whole-file Olympus FluoView OIB (compound file): open -> info -> vendor -> entries -> check -> plane reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_oif::OibReader, "whole_oib", "oib", data);
});
