//! Whole-file Agilent Seahorse .asyr: gzip, the assay XML, plate map, readings, spans and protocol: open -> info -> vendor -> entries -> check -> table/trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_biophys::SeahorseReader, "whole_seahorse", "asyr", data);
});
