//! Fuzz regressions for the JCAMP-DX ASDF decoder: `asdf-*` inputs in
//! `tests/fixtures/malformed/` crashed or exhausted memory in the `jcamp_asdf` target
//! (`fuzz/fuzz_targets/jcamp_asdf.rs`). Each must decode or fail cleanly, within bounds.
//! Whole files (`file-*`) are replayed by `crates/openreadout-cli/tests/fuzz_regressions.rs`.

use std::path::PathBuf;

#[test]
fn asdf_fixtures_fail_cleanly() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/malformed");
    let mut n = 0;
    for e in std::fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path();
        if !p
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("asdf-"))
        {
            continue;
        }
        n += 1;
        let text = String::from_utf8_lossy(&std::fs::read(&p).unwrap()).into_owned();
        if let Ok(t) = openreadout_nmr::decode_asdf(&text) {
            assert!(t.y.len() <= 1 << 24, "{}", p.display());
        }
        let _ = openreadout_nmr::decode_groups(&text, 2);
        for line in text.lines() {
            let _ = openreadout_nmr::lex_asdf_line(line);
        }
    }
    assert!(n > 0, "no fixtures found");
}

#[test]
fn dup_counts_beyond_the_cap_are_errors() {
    // DUP `S073741824` repeats the value 2^30 times: 8 GiB of f64 before the cap was lowered.
    assert!(openreadout_nmr::decode_asdf("1 A1S073741824\n").is_err());
    let t = openreadout_nmr::decode_asdf("1 A1T\n").unwrap();
    assert_eq!(t.y, [11.0, 11.0]);
}
