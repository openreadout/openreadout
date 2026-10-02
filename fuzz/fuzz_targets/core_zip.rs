//! The shared zip reader (`openreadout_core::zip`: qPCR RDML/`.eds`/`.pcrd`, OpenLab CDS
//! `.dx`/`.rx`, zipped Zarr stores): central directory, ZIP64 records, local headers, stored and
//! deflated members (CRC-checked, inflation bounded by the declared size), stored ranges.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(members) = openreadout_core::zip::read_all_from_bytes(data.to_vec()) {
        assert!(members.iter().all(|m| m.len() <= 64 << 20));
    }
});
