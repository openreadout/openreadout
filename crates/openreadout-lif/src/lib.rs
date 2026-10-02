//! Clean-room reader for the Leica LIF family: LIF, LIFEXT, LOF, XLIF, XLEF, XLCF, XLLF.
//!
//! Layout and vocabulary: `docs/formats/lif.md`. Provenance: `docs/provenance/lif.md`.
//! A LIF file is a UTF-16 XML document followed by raw, uncompressed memory blocks.
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
//! use openreadout_lif::LifReader;
//!
//! let format = LifReader.descriptor();
//! assert_eq!(format.id, openreadout_lif::FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::{FormatReader, PlaneIndex};
//! use openreadout_lif::LifReader;
//!
//! let mut dataset = LifReader.open(Path::new("project.lif"))?;
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
pub mod container;
mod dataset;
#[doc(hidden)]
pub mod frames;
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing;
#[doc(hidden)]
pub mod mosaic;
mod storage;
#[doc(hidden)]
pub mod xlef;
#[doc(hidden)]
pub mod xml;

use std::path::Path;

use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Result};

#[doc(hidden)]
pub use container::{
    ContainerHeader, ContainerKind, LOF_TEXT, LifFile, MAGIC, MemoryBlock, TEXT_MARKER,
};
#[doc(hidden)]
pub use mosaic::{MosaicLayout, PlacementMethod, TilePlacement, layout};
#[doc(hidden)]
pub use xlef::{FrameRef, XmlContainer, XmlKind};
#[doc(hidden)]
pub use xml::{
    Axis, ChannelDesc, DimensionDesc, FlimInfo, HardwareInfo, ImageNode, TilePosition, TileScan,
};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "lif";

/// Extensions of the XML-only members of the family.
const XML_EXTENSIONS: [&str; 4] = ["xlif", "xlef", "xlcf", "xllf"];

/// The Leica LIF format reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct LifReader;

impl FormatReader for LifReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::LIF)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "Leica LIF".into(),
            vendor: "Leica Microsystems".into(),
            extensions: vec![
                "lif".into(),
                "lifext".into(),
                "lof".into(),
                "xlif".into(),
                "xlef".into(),
                "xlcf".into(),
                "xllf".into(),
            ],
            family: "microscopy".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::LIF.confidence,
            known_gaps: vec![
                "FLIM/TCSPC raw photon data (FALCON) is detected and described but not decoded; reading those planes exits 6".into(),
                "XLIF frames stored as single-page TIFF files are read (validated on two LAS X exports); JPEG and PNG frames follow the same rule unvalidated (a note says so); BMP and multi-page OME/Aivia TIFF frames are not read (exit 6)".into(),
                "LOF, XLEF and XLCF are implemented from liffile's documentation and synthetic test files only; XLLF folder lists and XLIF files with TIFF frames are validated on real LAS X 3.7 exports (two depositors)".into(),
                "Tile scans are stitched from stage positions without registration or blending; LAS X 'Merged' images may differ where it registered overlapping tiles".into(),
                "Rotation (DimID 6) and XT/T-slice (DimID 7/8) addressing is validated on synthetic files only".into(),
                "LIFEXT sidecar images (pyramid previews, histograms) are listed from the .lif but read by opening the .lifext itself".into(),
                "Version-1 LIF containers (32-bit block lengths) are untested".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        let definite = |note: Option<&str>| {
            Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: note.map(str::to_string),
            })
        };
        if container::looks_like_lif(head) {
            return definite(None);
        }
        if container::looks_like_lifext(head) {
            return definite(Some("Leica LIFEXT sidecar (images attached to a .lif)"));
        }
        if container::looks_like_lof(head) {
            return definite(Some("Leica LOF (single-object) file"));
        }
        if has_extension(path, &XML_EXTENSIONS) && xlef::looks_like_xml_container(head) {
            return definite(Some(
                "Leica XML container (XLIF/XLEF/XLCF/XLLF); images are read from the files it references",
            ));
        }
        if has_extension(path, &["lof", "lifext", "xlif", "xlef", "xlcf", "xllf"]) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::ExtensionOnly,
                note: Some(
                    "Leica LOF/XLEF-family extension, but the content does not match the expected header"
                        .into(),
                ),
            });
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn reads_any_source(&self) -> bool {
        true
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let fs: &Fs = input.fs();
        let head = container::read_head(fs, input.path(), 4096)?;
        let known = container::looks_like_lif(&head)
            || container::looks_like_lifext(&head)
            || container::looks_like_lof(&head)
            || xlef::looks_like_xml_container(&head);
        if !known {
            return Err(Error::corrupt(
                FORMAT_ID,
                "file does not start with a LIF/LOF/LIFEXT header block or a Leica XML container",
            ));
        }
        Ok(Box::new(dataset::LifDataset::open(input)?))
    }
}
