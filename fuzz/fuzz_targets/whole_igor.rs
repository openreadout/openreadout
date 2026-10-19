//! Whole-file Igor Pro binary wave or packed experiment: wave headers and sections, record chain, variables: open -> info -> vendor -> entries -> check -> plane/table/trace/spectrum reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_igor::IgorReader, "whole_igor", "ibw", data);
});
