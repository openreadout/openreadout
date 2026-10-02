//! Whole-file Malvern Zetasizer .dts: the compound file, the Header identifier, record header walks, the sample-name rule and the size and zeta result structures.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_biophys::ZetasizerReader, "whole_zetasizer", "dts", data);
});
