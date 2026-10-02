//! Sample sheets and plate layouts (`--sample-sheet`): the first byte picks the file name the
//! reader sees (`.csv`, `.tsv`, `.txt`, `.xlsx`), the rest is the file; then every cell of the
//! long form is parsed as a well name and as a number, and the table writers and readers run on
//! the sheet as a table.
//!
//! Not on macOS: `openreadout-batch` links `zarrs` (through the index and OME-Zarr crates),
//! whose `inventory` initializers do not link under sanitizer coverage with Apple's linker (see
//! whole_zarr_zip.rs). Runs on Linux (CI).
#![no_main]

use libfuzzer_sys::fuzz_target;

#[cfg(not(target_os = "macos"))]
fn run(data: &[u8]) {
    let Some((&pick, body)) = data.split_first() else {
        return;
    };
    let ext = ["csv", "tsv", "txt", "xlsx"][usize::from(pick % 4)];
    let path = openreadout_fuzz::scratch_file("batch_sheet", ext, body);
    let Ok(sheet) = openreadout_batch::sheet::read(&path, None) else {
        return;
    };
    for row in &sheet.rows {
        assert_eq!(row.len(), sheet.columns.len(), "ragged long form");
        for cell in row {
            if let Some(w) = openreadout_batch::well::parse(cell) {
                assert!(w.row < openreadout_batch::well::MAX_ROWS);
                let _ = w.name();
            }
            let _ = openreadout_batch::table::parse_number(cell);
        }
    }
}

#[cfg(target_os = "macos")]
fn run(_: &[u8]) {}

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    run(data);
});
