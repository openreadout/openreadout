//! Readers for electrophysiology recordings that have no other home in OpenReadout:
//! HEKA PatchMaster bundles (`heka-patchmaster`), CED Spike2 `.smr`/`.smrx` files (`ced-spike2`),
//! Open Ephys recordings (`open-ephys`, binary and legacy formats) and WinWCP `.wcp` files
//! (`winwcp`).
//!
//! Layout and vocabulary: `docs/formats/heka-patchmaster.md`, `docs/formats/ced-spike2.md`,
//! `docs/formats/open-ephys.md`, `docs/formats/winwcp.md`.
//! Provenance: `docs/provenance/heka-patchmaster.md` (HEKA's public file-format documents and the
//! MIT-licensed load-heka-python), `docs/provenance/ced-spike2.md` (Neo's BSD-3 `Spike2RawIO`); no
//! vendor software or SDK was opened.
//!
//! # Example
//!
//! ```
//! use openreadout_core::FormatReader;
//! use openreadout_ephys::HekaReader;
//!
//! let format = HekaReader.descriptor();
//! assert_eq!(format.id, openreadout_ephys::HEKA_FORMAT_ID);
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::FormatReader;
//! use openreadout_ephys::HekaReader;
//!
//! let mut dataset = HekaReader.open(Path::new("cell.dat"))?;
//! let info = dataset.info()?; // headers only
//! for t in &info.traces {
//!     println!("{:?}: {} sweep(s) at {} Hz", t.name, t.sweep_count, t.sample_rate_hz);
//! }
//! let sweep = dataset.read_trace(0, 0, 0, 1000)?; // trace 0, sweep 0, first 1000 samples
//! println!("{:?}", sweep.channels[0].first());
//! # Ok::<(), openreadout_core::Error>(())
//! ```
//!
//! # API stability
//!
//! The supported API is what this page documents. The parser modules are public only so that
//! tests and fuzz targets can reach them: they are hidden from this documentation and may change
//! in any release.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod assurance;
#[doc(hidden)]
pub mod heka;
#[doc(hidden)]
pub mod openephys;
#[doc(hidden)]
pub mod spike2;
#[doc(hidden)]
pub mod winwcp;

pub use heka::{HEKA_FORMAT_ID, HekaReader};
pub use openephys::{OPEN_EPHYS_FORMAT_ID, OpenEphysReader};
pub use spike2::{SPIKE2_FORMAT_ID, Spike2Reader};
pub use winwcp::{WINWCP_FORMAT_ID, WinWcpReader};
