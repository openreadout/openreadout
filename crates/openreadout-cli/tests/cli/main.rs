//! End-to-end CLI tests. Tests that need corpus files skip when the file is absent, so the
//! suite passes on a fresh checkout and does more once `cargo xtask corpus fetch` has run.

mod basics;
mod batch;
mod common;
mod edge_cases;
mod flow;
mod mass_spec;
mod microscopy;
mod signals;
