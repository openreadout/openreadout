//! Whole-file DICOM: open -> info -> vendor -> entries -> check -> plane reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_medical::DicomReader, "whole_dicom", "dcm", data);
});
