//! Whole-file Cytiva Biacore T200 evaluation .bme: the compound file, the data-manager storages, curve headers and the evaluation-item XML with its fits.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_biophys::BiacoreEvaluationReader, "whole_bme", "bme", data);
});
