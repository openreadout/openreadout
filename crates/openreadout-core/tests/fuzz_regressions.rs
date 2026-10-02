//! Fuzz regressions for core parsers: inputs from the `selection` cargo-fuzz target that
//! crashed (out of memory) an earlier `Selection::parse`, kept in
//! `tests/fixtures/malformed/`. Arguments are newline-separated, as in the fuzz target.

use std::path::PathBuf;

use openreadout_core::select::Selection;

#[test]
fn selection_fixtures_are_usage_errors() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/malformed");
    let mut n = 0;
    for p in std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()) {
        let text = String::from_utf8_lossy(&std::fs::read(&p).unwrap()).into_owned();
        let args: Vec<String> = text.split('\n').map(str::to_string).collect();
        let err = Selection::parse(&args).expect_err("an absurd selection must be rejected");
        assert_eq!(err.exit_code(), 2, "{}: {err}", p.display());
        n += 1;
    }
    assert!(n > 0, "no fixtures found");
}
