//! Sciex QTRAP grid scans (quadrupole and ion-trap scans in `.wiff.scan`: segments, then 4-bit
//! point codes) and the precursor-charge records of `DDERealTimeDataEx`.
#![no_main]

use libfuzzer_sys::fuzz_target;
use openreadout_sciex::layout::{decode_grid_scan, dependent_charges};

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    if let Ok(g) = decode_grid_scan(data) {
        let n = g.grid_len();
        for &(k, _) in g.points.iter().take(64) {
            let _ = g.at(k);
        }
        let _ = g.at(n);
    }
    let _ = dependent_charges(data);
});
