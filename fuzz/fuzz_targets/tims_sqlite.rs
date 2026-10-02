//! The pure-Rust read-only SQLite reader of the timsTOF reader: header, b-tree pages,
//! schema, then every table counted and read.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    let path = openreadout_fuzz::scratch_file("tims_sqlite", "tdf", data);
    let Ok(mut db) = openreadout_bruker_tims::SqliteDb::open(&path) else {
        return;
    };
    let _ = db.wal_frames();
    for t in db.tables() {
        let _ = db.count_rows(&t);
        if let Ok(tab) = db.read_table(&t) {
            for row in tab.rows.iter().take(64) {
                for v in row {
                    let _ = v.to_json();
                }
            }
        }
    }
});
