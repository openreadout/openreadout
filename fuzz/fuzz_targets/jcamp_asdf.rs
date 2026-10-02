//! JCAMP-DX ASDF (compressed X++(Y..Y)) decoder: whole-table decoding, grouped decoding
//! (NTUPLES pages) and the per-line lexer.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = openreadout_nmr::decode_asdf(text);
    let _ = openreadout_nmr::decode_groups(text, 2);
    for line in text.lines().take(256) {
        let _ = openreadout_nmr::lex_asdf_line(line);
    }
});
