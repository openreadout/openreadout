//! LIF header block, UTF-16 XML extraction and the memory-block chain (`LifFile::open`).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = openreadout_lif::fuzzing::fuzz_sniff(data);
    let p = openreadout_fuzz::scratch_file("lif-container", "lif", data);
    if let Ok(f) = openreadout_lif::LifFile::open(&p) {
        let _ = (f.blocks.len(), f.truncated_at, f.xml.len());
    }
});
