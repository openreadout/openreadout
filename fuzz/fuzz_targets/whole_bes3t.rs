//! Whole-data-set Bruker BES3T: a .DSC descriptor, its .DTA data and a .YGF axis file (parts split by the bundle separator): open -> info -> vendor -> entries -> check -> trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(&openreadout_epr::Bes3tReader, "whole_bes3t", "s.DSC", &["s.DSC", "s.DTA", "s.YGF"], Some(0), data);
});
