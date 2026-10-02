//! Inputs that crashed the `assay_input` fuzz target (`fuzz/`), kept under
//! `tests/fixtures/malformed/`: layouts, well lists and long plate CSVs must give a result or an
//! error, never a panic.

use openreadout_assay::{PlateData, layout};

#[test]
fn malformed_layouts_fail_cleanly() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/malformed");
    let mut n = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let text = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned();
        let r = std::panic::catch_unwind(|| {
            let _ = layout::parse_layout(&text, "fixture");
            if let Some(first) = text.lines().next() {
                let _ = layout::parse_wells(first);
            }
            let _ = PlateData::from_long_csv(&text, "fixture");
        });
        assert!(r.is_ok(), "{} panicked", path.display());
        n += 1;
    }
    assert!(n > 0, "no fixtures in {}", dir.display());
}
