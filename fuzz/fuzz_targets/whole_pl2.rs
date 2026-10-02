//! Whole-file Plexon PL2 (OmniPlex): open -> info -> vendor -> entries -> check -> table/trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_plexon::PlexonReader, "whole_pl2", "pl2", data);
});
