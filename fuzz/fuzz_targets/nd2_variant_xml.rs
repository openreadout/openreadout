//! ND2 version-2 "variant" XML decoder and the normalizers that consume its output.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(v) = openreadout_nd2::variant_decode(data) {
        openreadout_nd2::fuzzing::fuzz_normalize(&v);
        let _ = serde_json::to_string(&v);
    }
});
