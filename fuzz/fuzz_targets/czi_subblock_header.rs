//! CZI subblock payload header, directory entries and attachment entries from raw bytes.
#![no_main]

use libfuzzer_sys::fuzz_target;

use openreadout_czi::fuzzing;

fuzz_target!(|data: &[u8]| {
    let _ = fuzzing::fuzz_subblock_header(data);
    let _ = fuzzing::fuzz_directory_entries(data);
    let _ = fuzzing::fuzz_attachment_entries(data);
});
