//! Whole-file chromatography reader: Thermo Scientific Chromeleon 7 archive (.cmbx zip of header.xml, the protocol-buffers sequence file and PtsLDiff signal members): open -> info -> vendor -> entries -> check -> trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_chrom::ChromeleonReader, "whole_chromeleon", "cmbx", data);
});
