//! HDF5-based formats through the pure-Rust `hdf5-pure` crate: Imaris `.ims` volumes
//! (pyramids, LZ4/deflate chunks), NWB 2.x (session fields, plain `TimeSeries` as traces) and a
//! generic HDF5 fallback whose `info --view structure` lists groups, datasets and attributes.
//!
//! Layout and vocabulary: `docs/formats/ims.md`, `docs/formats/hdf5.md`. Provenance:
//! `docs/provenance/ims.md`, `docs/provenance/hdf5.md`.
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
//! use openreadout_hdf5::ImsReader;
//!
//! let format = ImsReader.descriptor();
//! assert_eq!(format.id, openreadout_hdf5::IMS_FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::{FormatReader, PlaneIndex};
//! use openreadout_hdf5::ImsReader;
//!
//! let mut dataset = ImsReader.open(Path::new("volume.ims"))?;
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
pub mod generic;
#[doc(hidden)]
pub mod h5util;
#[doc(hidden)]
pub mod ims;
#[doc(hidden)]
pub mod ims_scene;
#[doc(hidden)]
pub mod nwb;
#[doc(hidden)]
pub mod nwb_tables;
#[doc(hidden)]
pub mod nwb_write;

pub use generic::{HDF5_FORMAT_ID, Hdf5Reader};
pub use ims::{IMS_FORMAT_ID, ImsReader};
pub use nwb::{NWB_FORMAT_ID, NwbReader};
pub use nwb_write::{
    NWB_VERSION, NwbExportOptions, NwbExportReport, NwbSeriesReport, default_nwb_output, export_nwb,
};

#[doc(hidden)]
pub use generic::Hdf5Dataset;
#[doc(hidden)]
pub use ims::{ImsDataset, ImsLevel};
#[doc(hidden)]
pub use nwb::{NwbDataset, NwbSeries};
