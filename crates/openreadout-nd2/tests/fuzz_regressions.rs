//! Fuzz regressions: inputs that crashed an earlier version of this reader (panics,
//! arithmetic overflow, out-of-memory aborts from header-sized allocations, stack
//! exhaustion), found by the cargo-fuzz targets in `fuzz/` and kept in
//! `tests/fixtures/malformed/`. Each must yield `Ok` or a clean `Error` (exit code 2, 4,
//! 5 or 6) from every `Dataset` method, never a panic.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use openreadout_core::Error;
use openreadout_core::reader::{FormatReader, PlaneIndex};

fn fixtures(prefix: &str) -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/malformed");
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(prefix))
        })
        .collect();
    v.sort();
    v
}

/// Run every `Dataset` method; return the errors they produced.
fn exercise(path: &Path) -> Vec<Error> {
    let mut errs = Vec::new();
    let mut ds = match openreadout_nd2::Nd2Reader.open(path) {
        Ok(ds) => ds,
        Err(e) => return vec![e],
    };
    errs.extend(ds.info().err());
    errs.extend(ds.vendor_metadata().err());
    errs.extend(ds.entries().err());
    errs.extend(ds.check().err());
    errs.extend(ds.read_plane(0, PlaneIndex::default()).err());
    errs
}

#[test]
fn file_fixtures_fail_cleanly() {
    let files = fixtures("file-");
    assert!(!files.is_empty(), "no fixtures found");
    for p in files {
        eprintln!("fixture {}", p.display());
        let errs = catch_unwind(AssertUnwindSafe(|| exercise(&p)))
            .unwrap_or_else(|_| panic!("{} panicked", p.display()));
        for e in errs {
            assert!(
                matches!(e.exit_code(), 2 | 4 | 5 | 6),
                "{}: unexpected error class {} ({e})",
                p.display(),
                e.code()
            );
        }
    }
}

#[test]
fn metadata_fixtures_decode_or_fail_cleanly() {
    for p in fixtures("lv-") {
        let data = std::fs::read(&p).unwrap();
        let _ = catch_unwind(|| openreadout_nd2::lv_decode(&data))
            .unwrap_or_else(|_| panic!("{} panicked", p.display()));
    }
    for p in fixtures("variant-") {
        let data = std::fs::read(&p).unwrap();
        let _ = catch_unwind(|| openreadout_nd2::variant_decode(&data))
            .unwrap_or_else(|_| panic!("{} panicked", p.display()));
    }
}
