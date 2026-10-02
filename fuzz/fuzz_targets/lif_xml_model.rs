//! LIF XML header -> image nodes (dimensions, channels, attachments, tiles).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let xml = String::from_utf8_lossy(data);
    let _ = openreadout_lif::fuzzing::fuzz_xml_model(&xml);
});
