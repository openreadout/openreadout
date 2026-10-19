//! Whole MCS HDF5 file: open -> info -> vendor -> entries -> check -> trace and table reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_ephys::McsHdf5Reader, "whole_mcs_hdf5", "h5", data);
});
