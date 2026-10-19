//! Whole-file Andor SIF: header lines, axis names and image structure, block flags, XML trailer: open -> info -> vendor -> entries -> check -> plane/table/trace/spectrum reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_sif::SifReader, "whole_sif", "sif", data);
});
