//! Command-level operations over `OpenReadout` datasets, shared by the CLI, the MCP server and
//! the WebAssembly build. Everything here is built on the contract in `openreadout_core`
//! ([`Dataset`](openreadout_core::Dataset), [`FileInfo`](openreadout_core::FileInfo),
//! [`Experiment`](openreadout_core::Experiment)).
//!
//! - [`compare`]: `check --against`, a diff of two files' metadata and planes.
//! - [`explain`](mod@explain): `info --view explain`, a plain-English account of a file and
//!   answers to questions about it.
//! - [`extract`]: `export --attachment`, one embedded attachment written to a new file.
//! - [`project`]: `--only`, keep the JSON values an agent asked for by pointer.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod compare;
pub mod explain;
mod explain_next;
pub mod extract;
pub mod project;

pub use explain::{Explanation, explain};
