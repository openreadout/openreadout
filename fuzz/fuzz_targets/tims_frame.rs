//! timsTOF frame blobs: a TDF frame (zstd, four byte planes of u32) and a TSF line spectrum.
//! First 4 bytes: scan-count hint (TDF) / peak count (TSF); byte 4 picks the decoder.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    if data.len() < 5 {
        return;
    }
    let hint = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let body = &data[5..];
    if data[4] & 1 == 0 {
        if let Ok(f) = openreadout_bruker_tims::decode_tdf_frame(body, hint) {
            let _ = f.scan_count();
        }
    } else {
        let clen = u32::try_from(body.len()).unwrap_or(u32::MAX);
        let _ = openreadout_bruker_tims::decode_tsf_spectrum(clen, body, hint as usize);
    }
});
