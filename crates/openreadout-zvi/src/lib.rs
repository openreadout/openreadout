//! Clean-room reader for Zeiss AxioVision ZVI files (OLE2 / MS-CFB compound files).
//!
//! The compound-file container comes from Microsoft's public MS-CFB specification (the shared
//! reader in `openreadout-core::cfb`); the ZVI streams, value encoding and tag ids were read off
//! hex dumps of corpus files and identified by comparing values across files and with
//! Bio-Formats' output (run as a black box). Layout and vocabulary: `docs/formats/zvi.md`.
//! Provenance: `docs/provenance/zvi.md`.
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
//! use openreadout_zvi::ZviReader;
//!
//! let format = ZviReader.descriptor();
//! assert_eq!(format.id, openreadout_zvi::FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::{FormatReader, PlaneIndex};
//! use openreadout_zvi::ZviReader;
//!
//! let mut dataset = ZviReader.open(Path::new("image.zvi"))?;
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
pub mod dataset;
#[doc(hidden)]
pub mod tags;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::cfb::CFB_MAGIC;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;

#[doc(hidden)]
pub use dataset::*;

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "zvi";

/// The ZVI reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct ZviReader;

impl FormatReader for ZviReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::ZVI)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "Zeiss AxioVision ZVI".into(),
            vendor: "Carl Zeiss (AxioVision)".into(),
            extensions: vec!["zvi".into()],
            family: "microscopy".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::ZVI.confidence,
            known_gaps: vec![
                "Derived without a specification from 16 corpus files (12 depositors, 8 microscope stands, 2003-2025): 16-bit grey (pixel format 4) and 3 x 16-bit colour (format 8) are validated bit for bit; 8-bit grey and 3 x 8-bit colour are inferred (a note says so); other formats exit 6".into(),
                "Time series (tag 2821), multi-position and mosaic files are handled by inferred index tags that no public file exercises (a note says so); the acquisition-setup folder (Image/RootFolder) is listed, not interpreted".into(),
                "Scale units other than micrometres (code 76) are not interpreted; uncalibrated axes have no physical size".into(),
                "Layers, shapes (annotations) and the document summary property sets are listed, not decoded".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if !has_extension(path, &["zvi"]) {
            return None;
        }
        Some(if head.starts_with(&CFB_MAGIC) {
            Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: Some("OLE2 compound file with the .zvi extension".into()),
            }
        } else {
            Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::ExtensionOnly,
                note: Some("ZVI extension, but no compound-file signature".into()),
            }
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(ZviDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(ZviDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
