//! CZI `ImageDocument` XML: the normalizer and the generic XML-to-JSON vendor dump.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let xml = String::from_utf8_lossy(data);
    let _ = openreadout_czi::fuzzing::fuzz_metadata_xml(&xml);
    if let Some(v) = openreadout_core::xmljson::xml_to_json(&xml) {
        let _ = serde_json::to_string(&v);
    }
});
