//! Whole-file FASTQ, plain or gzip: records, multi-line sequences and qualities, the statistics scan.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_seq::FastqReader, "whole_fastq", "fastq", data);
});
