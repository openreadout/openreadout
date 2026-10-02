//! Regression: `stats --tidy --threads 2` over many multi-plane files deadlocked when the data
//! sets ran on the global rayon pool, because each measure waits (on a channel) for plane reads
//! it hands to the global pool from a helper thread. Its own test binary, so the global pool can
//! be made small.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test batch_threads`
#![cfg(feature = "corpus")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use openreadout_batch::measures::StatsMeasure;
use openreadout_batch::run::BatchRequest;
use openreadout_core::Registry;

fn files_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/files"),
        PathBuf::from,
    )
}

#[test]
fn many_multi_plane_files_on_a_small_global_pool_do_not_deadlock() {
    let files: Vec<PathBuf> = [
        "aics-ND2-dims-p4z5t3c2y32x32.nd2",
        "aics-ND2-dims-p2z5t3-2c4y32x32.nd2",
        "aics-ND2-dims-p1z5t3c2y32x32.nd2",
        "aics-ND2-dims-t3c2y32x32.nd2",
        "aics-ND2-dims-rgb-t3p2c2z3x64y64.nd2",
    ]
    .iter()
    .map(|f| files_dir().join(f))
    .filter(|p| p.exists())
    .collect();
    if files.len() < 3 {
        eprintln!("skip: corpus not fetched");
        return;
    }
    rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build_global()
        .unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let reg = Registry::new().with(Box::new(openreadout_nd2::Nd2Reader));
        let mut req = BatchRequest::default();
        // every file three times: more data sets than workers
        req.inputs = files.iter().chain(&files).chain(&files).cloned().collect();
        let r = openreadout_batch::run(&reg, &StatsMeasure::default(), &req, None, None);
        let _ = tx.send(r.map(|r| (r.inputs.ok, r.inputs.failed)));
    });
    let (ok, failed) = rx
        .recv_timeout(Duration::from_secs(120))
        .expect("batch stats did not finish in 120 s: deadlock")
        .unwrap();
    assert_eq!(failed, 0);
    assert!(ok >= 9);
}
