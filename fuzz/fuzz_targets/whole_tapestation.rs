//! Whole-data-set Agilent TapeStation export: the XML and its _Electropherogram.csv (parts split by the bundle separator): open -> info -> vendor -> check -> trace and table reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(&openreadout_ce::TapeStationReader, "whole_tapestation", "s.xml", &["s.xml", "s_Electropherogram.csv"], Some(0), data);
});
