//! Whole-file NIfTI-1/2 (and gzip-compressed NIfTI): open -> info -> vendor -> check -> planes.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_medical::NiftiReader, "whole_nifti", "nii", data);
});
