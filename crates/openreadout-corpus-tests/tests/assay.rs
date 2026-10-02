//! Plate analysis (`openreadout analyze assay`) against SciPy, R nls/drc/growthcurver and the Gen5
//! and SkanIt vendor results: the cases in `corpus/oracle/assay/*.json` (`oracle/assay.py`). The
//! same harness runs in `cargo test -p openreadout-assay`; here it runs under `--features corpus`
//! with the corpus files present.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test assay -- --nocapture`
#![cfg(feature = "corpus")]

#[path = "../../openreadout-assay/tests/oracle.rs"]
mod oracle;
