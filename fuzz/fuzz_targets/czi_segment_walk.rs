//! CZI container: file header, metadata/directory/attachment-directory positions, and the
//! sequential segment walk (`CziFile::open`).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    let p = openreadout_fuzz::scratch_file("czi-walk", "czi", data);
    if let Ok(f) = openreadout_czi::CziFile::open(&p) {
        let _ = (f.segments.len(), f.entries.len(), f.attachments.len(), f.problems.len());
    }
});
