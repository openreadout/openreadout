//! Whole-file Arbin .res: the Jet 4 catalog, table definitions, data pages, rows (null masks, variable offsets, overflow pointers, long values, compressed text) and Arbin's tables.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_echem::ArbinReader, "whole_arbin", "res", data);
});
