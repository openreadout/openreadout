//! Bruker ParaVision experiment: `acqp`, `method`, `pdata/1/visu_pars`, `pdata/1/2dseq`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(
        &openreadout_medical::ParavisionReader,
        "whole_paravision",
        "1",
        &["1/acqp", "1/method", "1/pdata/1/visu_pars", "1/pdata/1/2dseq"],
        None,
        data,
    );
});
