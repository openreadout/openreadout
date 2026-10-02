//! `book/src/guides/lab-shares.md` documents every column, every query alias and every personal-data rule.

use openreadout_index::query::FIELDS;
use openreadout_index::tables::{EXPERIMENTS, FILES, PROBLEMS};

#[test]
fn docs_cover_columns_aliases_and_rules() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../book/src/guides/lab-shares.md"
    );
    let Ok(doc) = std::fs::read_to_string(path) else {
        eprintln!("book/src/guides/lab-shares.md not found (packaged crate); skipped");
        return;
    };
    let mut missing = Vec::new();
    for c in EXPERIMENTS.iter().chain(FILES).chain(PROBLEMS) {
        if !doc.contains(&format!("| `{}` |", c.name)) {
            missing.push(format!("column {}", c.name));
        }
    }
    for a in FIELDS {
        if !doc.contains(&format!("`{}`", a.name)) {
            missing.push(format!("alias {}", a.name));
        }
    }
    for rule in [
        "operator_field",
        "operator_name_in_sample",
        "operator_name_in_field",
        "patient_name",
        "email_address",
        "phone_number",
        "mrn_keyword",
        "dob_keyword",
        "comment_field",
    ] {
        if !doc.contains(&format!("`{rule}`")) {
            missing.push(format!("pii rule {rule}"));
        }
    }
    assert!(
        missing.is_empty(),
        "missing from book/src/guides/lab-shares.md: {missing:?}"
    );
}
