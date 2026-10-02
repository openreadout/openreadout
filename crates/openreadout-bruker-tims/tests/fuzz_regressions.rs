//! Fuzz regressions for the timsTOF reader's own parsers, kept in `tests/fixtures/malformed/`:
//! `sqlite-*` inputs crashed the `tims_sqlite` target (read-only SQLite reader), `frame-*`
//! inputs the `tims_frame` target (first 4 bytes: scan-count hint or peak count; byte 4 picks
//! TDF (even) or TSF (odd); the rest is the blob). Whole data sets (`bundle-whole_tims-*`) are
//! replayed by `crates/openreadout-cli/tests/fuzz_regressions.rs`. Each must fail
//! cleanly, never panic or allocate without bound.

use std::path::PathBuf;

use openreadout_bruker_tims::{SqliteDb, decode_tdf_frame, decode_tsf_spectrum};
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

#[test]
fn sqlite_fixtures_fail_cleanly() {
    let files = fixtures("sqlite-");
    assert!(!files.is_empty(), "no fixtures found");
    for p in files {
        eprintln!("fixture {}", p.display());
        // The SQLite reader alone, every table.
        if let Ok(mut db) = SqliteDb::open(&p) {
            for t in db.tables() {
                let _ = db.count_rows(&t);
                let _ = db.read_table(&t);
            }
        }
        // And as the `analysis.tdf` of a `.d` directory.
        let dir = std::env::temp_dir().join(format!(
            "openreadout-tims-fuzz-{}-{}.d",
            std::process::id(),
            p.file_stem().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(&p, dir.join("analysis.tdf")).unwrap();
        std::fs::write(dir.join("analysis.tdf_bin"), [0u8; 64]).unwrap();
        if let Ok(mut ds) = openreadout_bruker_tims::BrukerTimsReader.open(&dir) {
            let _ = ds.info();
            let _ = ds.entries();
            let _ = ds.check();
            let _ = ds.read_plane(0, PlaneIndex::default());
            let _ = ds.read_spectrum(0, 0);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn frame_fixtures_fail_cleanly() {
    for p in fixtures("frame-") {
        let data = std::fs::read(&p).unwrap();
        if data.len() < 5 {
            continue;
        }
        let hint = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let body = &data[5..];
        if data[4] & 1 == 0 {
            let _ = decode_tdf_frame(body, hint);
        } else {
            let clen = u32::try_from(body.len()).unwrap_or(u32::MAX);
            let _ = decode_tsf_spectrum(clen, body, hint as usize);
        }
    }
    // An empty blob may not declare billions of scans (a 32 GiB offset table).
    assert!(decode_tdf_frame(&[], u32::MAX).is_err());
    assert!(decode_tdf_frame(&[], 927).is_ok());
}
