//! Clean-room readers for electron-microscopy formats, registered as separate formats:
//!
//! - [`MrcReader`]: MRC / CCP4 / MAP (MRC2014, an open CCP-EM specification), format id `mrc`;
//! - [`DmReader`]: Gatan Digital Micrograph DM3 / DM4, format id `dm`;
//! - [`SerReader`]: TIA / ES Vision series files `.ser` with their `.emi` sidecar, format id `ser`;
//! - [`EmdReader`]: Velox EMD (HDF5-based) files, format id `emd`.
//!
//! Vocabulary: `docs/formats/{em,mrc,dm,ser}.md`. Provenance: `docs/provenance/{mrc,dm,ser}.md`.
//!
//! # Example
//!
//! Every reader implements [`openreadout_core::FormatReader`]: `descriptor`
//! says what it reads, `sniff` recognises a file from its first bytes, and `open` returns a
//! [`openreadout_core::Dataset`] with a header-only `info()` summary and lazy data
//! access.
//!
//! ```
//! use openreadout_core::FormatReader;
//! use openreadout_em::MrcReader;
//!
//! let format = MrcReader.descriptor();
//! assert_eq!(format.id, "mrc");
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::{FormatReader, PlaneIndex};
//! use openreadout_em::MrcReader;
//!
//! let mut dataset = MrcReader.open(Path::new("tomogram.mrc"))?;
//! let info = dataset.info()?; // headers only: fast even on multi-gigabyte files
//! for image in &info.images {
//!     println!(
//!         "image {}: {} x {} px, {} channel(s), {} plane(s)",
//!         image.index, image.size_x, image.size_y, image.size_c, image.plane_count
//!     );
//! }
//! // Pixels are read one plane at a time: image 0, channel 0, first Z slice, first time point.
//! let plane = dataset.read_plane(0, PlaneIndex { c: 0, z: 0, t: 0 })?;
//! assert_eq!(plane.data.len(), plane.expected_len());
//! # Ok::<(), openreadout_core::Error>(())
//! ```
//!
//! Applications that accept any instrument file usually register every reader in an
//! [`openreadout_core::Registry`] and let it detect the format; that is what the
//! `openreadout` command-line tool does.
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
pub mod dm;
#[doc(hidden)]
pub mod emd;
#[doc(hidden)]
pub mod mrc;
#[doc(hidden)]
pub mod ser;
mod util;

pub use dm::DmReader;
pub use emd::EmdReader;
pub use mrc::MrcReader;
pub use ser::SerReader;
