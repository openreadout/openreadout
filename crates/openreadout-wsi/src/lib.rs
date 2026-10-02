//! Clean-room readers for whole-slide formats that keep their pyramid in many stored images
//! next to a settings file: 3DHISTECH MIRAX (`.mrxs`, format id `mirax`).
//!
//! Layouts and vocabulary: `docs/formats/mirax.md`. Provenance: `docs/provenance/mirax.md`.
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
//! use openreadout_wsi::MiraxReader;
//!
//! let format = MiraxReader.descriptor();
//! assert_eq!(format.id, openreadout_wsi::mirax::FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a region of a slide:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::{FormatReader, PlaneIndex, Region};
//! use openreadout_wsi::MiraxReader;
//!
//! let mut dataset = MiraxReader.open(Path::new("slide.mrxs"))?;
//! let info = dataset.info()?; // headers only: fast even on multi-gigabyte slides
//! let level = &info.images[0].resolution_levels[2];
//! println!("level 2: {} x {} px", level.size_x, level.size_y);
//! let plane = dataset.read_region(0, PlaneIndex::default(), 2, Region::new(0, 0, 512, 512))?;
//! assert_eq!(plane.data.len(), plane.expected_len());
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
pub mod image;
#[doc(hidden)]
pub mod mirax;

pub use mirax::MiraxReader;
