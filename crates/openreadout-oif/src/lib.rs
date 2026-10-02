//! Clean-room reader for Olympus FluoView OIF and OIB data sets (FV1000/FV1200/FV10i).
//!
//! An OIF data set is a main `.oif` settings file (UTF-16 INI text) plus a `<name>.oif.files`
//! folder of plane TIFFs, per-plane `.pty` property files, LUTs, ROIs and a thumbnail. An OIB
//! file packs the same files into one OLE2 compound file (MS-CFB, read with the shared
//! `openreadout-core::cfb` reader) with an `OibInfo.txt` stream mapping stream names to file
//! names. Planes are decoded by the TIFF reader (`openreadout-tiff`).
//!
//! Derived from hex dumps of corpus files and from the documentation and source of `oiffile`
//! (BSD-3-Clause, Christoph Gohlke); Bio-Formats is run only as a black-box oracle. Layout and
//! vocabulary: `docs/formats/oif.md`. Provenance: `docs/provenance/oif.md`.
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
//! use openreadout_oif::{OibReader, OifReader};
//!
//! for format in [OibReader.descriptor(), OifReader.descriptor()] {
//!     println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! }
//! assert_eq!(OibReader.descriptor().id, openreadout_oif::OIB_FORMAT_ID);
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::{FormatReader, PlaneIndex};
//! use openreadout_oif::OibReader;
//!
//! let mut dataset = OibReader.open(Path::new("cells.oib"))?;
//! let info = dataset.info()?; // headers only
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
pub mod dataset;
#[doc(hidden)]
pub mod settings;
#[doc(hidden)]
pub mod store;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::cfb::CFB_MAGIC;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;

#[doc(hidden)]
pub use dataset::{FvDataset, FvImage, PlaneName, parse_plane_name};
#[doc(hidden)]
pub use settings::{Section, Settings};
#[doc(hidden)]
pub use store::{Member, MemberSource, Store, StoreKind};

/// Format id of OIB (single compound file) data sets.
pub const OIB_FORMAT_ID: &str = "oib";
/// Format id of OIF (settings file plus `.oif.files` folder) data sets.
pub const OIF_FORMAT_ID: &str = "oif";

fn known_gaps() -> Vec<String> {
    vec![
        "Derived without a specification from 12 corpus data sets (FV1000/FV1200, FluoView 4.2): 16-bit grey planes, uncompressed or LZW; 8-bit and colour planes follow the TIFF reader but no corpus file has them".into(),
        "Plane file names with axis letters other than C, Z, T and L become separate images (inferred, no corpus file)".into(),
        "Line scans (XT) are exposed as planes whose height is the number of lines (Y is time); point scans and multi-area time lapse are not reinterpreted".into(),
        "ROI (.roi) and LUT (.lut) files are listed, not decoded; channel colours are not reported".into(),
        "Multi-page plane TIFFs are read from their first page only".into(),
    ]
}

/// The OIB (compound file) reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct OibReader;

/// The OIF (settings file plus data folder) reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct OifReader;

impl FormatReader for OibReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::OIB)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: OIB_FORMAT_ID.into(),
            name: "Olympus FluoView OIB".into(),
            vendor: "Evident (Olympus)".into(),
            extensions: vec!["oib".into()],
            family: "microscopy".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::OIB.confidence,
            known_gaps: known_gaps(),
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if !has_extension(path, &["oib"]) {
            return None;
        }
        Some(if head.starts_with(&CFB_MAGIC) {
            Detection {
                format_id: OIB_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: Some("OLE2 compound file with the .oib extension".into()),
            }
        } else {
            Detection {
                format_id: OIB_FORMAT_ID,
                confidence: DetectConfidence::ExtensionOnly,
                note: Some("OIB extension, but no compound-file signature".into()),
            }
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(FvDataset::open_input(input, StoreKind::Oib)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// Does this look like a FluoView settings file (UTF-16LE with a byte-order mark, then `[`)?
pub fn looks_like_settings(head: &[u8]) -> bool {
    head.starts_with(&[0xFF, 0xFE, b'[', 0])
}

impl FormatReader for OifReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::OIF)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: OIF_FORMAT_ID.into(),
            name: "Olympus FluoView OIF".into(),
            vendor: "Evident (Olympus)".into(),
            extensions: vec!["oif".into()],
            family: "microscopy".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::OIF.confidence,
            known_gaps: known_gaps(),
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if !has_extension(path, &["oif"]) {
            return None;
        }
        Some(
            if looks_like_settings(head) || head.first() == Some(&b'[') {
                Detection {
                    format_id: OIF_FORMAT_ID,
                    confidence: DetectConfidence::Definite,
                    note: Some(
                        "FluoView settings file; planes are read from the .oif.files folder next to it"
                            .into(),
                    ),
                }
            } else {
                Detection {
                    format_id: OIF_FORMAT_ID,
                    confidence: DetectConfidence::ExtensionOnly,
                    note: Some("OIF extension, but not a settings text file".into()),
                }
            },
        )
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(FvDataset::open_input(input, StoreKind::Oif)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
