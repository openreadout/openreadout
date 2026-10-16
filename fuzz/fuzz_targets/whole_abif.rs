//! Whole-file Applied Biosystems ABIF (.ab1/.fsa): header, directory entries, typed items, base calls, fragment sizing: open -> info -> vendor -> check -> trace and table reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_ce::AbifReader, "whole_abif", "ab1", data);
});
