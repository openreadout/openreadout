//! The external-link scan of NeXus master files (`data_NNNNNN` link messages in raw bytes).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    for (name, file, object) in openreadout_hdf5::nxmx::external_data_links(data) {
        assert!(name.starts_with("data_") && !file.is_empty() && object.starts_with('/'));
    }
});
