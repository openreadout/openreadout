//! Whole-file Qubit 4 and Qubit Flex exports: header detection, `sep=` lines, untitled trailing columns, unit columns, run-id dates: open -> info -> vendor -> entries -> check -> table reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_benchtop::fuzz_qubit(data);
    openreadout_fuzz::whole_file(&openreadout_benchtop::QubitReader, "whole_qubit", "csv", data);
});
