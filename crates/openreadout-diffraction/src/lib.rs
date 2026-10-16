//! Clean-room readers for area-detector diffraction images, registered as separate formats:
//!
//! - `cbf`: CBF / imgCIF, including the miniCBF files PILATUS and EIGER detectors write
//!   (byte-offset compressed images with a `# key value` header). Derived from the IUCr imgCIF
//!   specification and FabIO (MIT) read as documentation.
//! - `smv`: SMV `.img` files (ADSC style: a `{ KEY=value; }` header and one uncompressed
//!   image), from X-ray CCDs and electron-diffraction cameras. Derived from dxtbx (BSD-3) and
//!   FabIO read as documentation.
//!
//! Both put the beam and detector geometry (wavelength, distance, beam centre, rotation) into
//! the shared `extra.diffraction` shape ([`openreadout_core::diffraction::Geometry`]), which
//! the preview uses for log contrast, masked pixels and resolution rings.
//!
//! Layout and vocabulary: `docs/formats/cbf.md`, `docs/formats/smv.md`. Provenance:
//! `docs/provenance/cbf.md`, `docs/provenance/smv.md`.
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
//! use openreadout_diffraction::{CbfReader, SmvReader};
//!
//! for format in [CbfReader.descriptor(), SmvReader.descriptor()] {
//!     println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! }
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::diffraction::Geometry;
//! use openreadout_core::{FormatReader, PlaneIndex};
//! use openreadout_diffraction::CbfReader;
//!
//! let mut dataset = CbfReader.open(Path::new("lysozyme_0001.cbf"))?;
//! let info = dataset.info()?; // the text header only
//! let image = &info.images[0];
//! if let Some(g) = Geometry::of(image) {
//!     println!("{:?} Å at {:?} mm, beam at {:?} px", g.wavelength_a, g.distance_mm, g.beam_center_px);
//! }
//! let plane = dataset.read_plane(0, PlaneIndex { c: 0, z: 0, t: 0 })?; // decodes the counts
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
pub mod cbf;
#[doc(hidden)]
pub mod common;
#[doc(hidden)]
pub mod smv;

pub use cbf::{CbfReader, FORMAT_ID as CBF_FORMAT_ID};
pub use smv::{FORMAT_ID as SMV_FORMAT_ID, SmvReader};

#[doc(hidden)]
pub use cbf::{CbfDataset, CbfLayout, PilatusHeader};
#[doc(hidden)]
pub use smv::{SmvDataset, SmvHeader};
