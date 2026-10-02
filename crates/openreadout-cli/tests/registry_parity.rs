//! The corpus harness's shared registry (`openreadout-corpus-tests/tests/support/registry.rs`,
//! used by its `snapshots` and `intake` tests) lists the same readers in the same order as the
//! CLI's (`src/registry.rs`), so the snapshots record what the CLI would detect.

fn readers(path: &str) -> Vec<String> {
    let text = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap();
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix(".with(Box::new(")?;
            Some(rest.trim_end_matches("))").to_string())
        })
        .collect()
}

#[test]
fn the_corpus_registry_matches_the_cli() {
    let cli = readers("src/registry.rs");
    let corpus = readers("../openreadout-corpus-tests/tests/support/registry.rs");
    assert!(cli.len() > 40, "{cli:?}");
    assert_eq!(
        corpus, cli,
        "copy the reader list of crates/openreadout-cli/src/registry.rs into \
         crates/openreadout-corpus-tests/tests/support/registry.rs"
    );
}
