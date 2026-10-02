//! Clean-room reader for Olympus/Evident OIR files (FluoView), including multi-file series whose
//! pixel blocks continue in `<name>_00001`, `<name>_00002`, ... next to the `.oir`.
//!
//! Layout and vocabulary: `docs/formats/oir.md`. Provenance: `docs/provenance/oir.md`.
//! An OIR file is a 96-byte header, a chain of typed blocks (XML documents, frame properties,
//! chunk tags and raw pixel chunks, a bitmap thumbnail) and a block index at the end.
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
//! use openreadout_oir::OirReader;
//!
//! let format = OirReader.descriptor();
//! assert_eq!(format.id, openreadout_oir::FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::{FormatReader, PlaneIndex};
//! use openreadout_oir::OirReader;
//!
//! let mut dataset = OirReader.open(Path::new("scan.oir"))?;
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
#[doc(hidden)]
pub mod dataset;
#[doc(hidden)]
pub mod meta;

use std::path::Path;

use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;
use openreadout_core::{Error, Result};

#[doc(hidden)]
pub use container::{
    Block, BlockKind, ChunkName, ChunkTag, FIRST_BLOCK_OFFSET, INDEX_MARKER, MAGIC, OirFile,
    OirHeader, XmlDoc, documents, looks_like_oir,
};
#[doc(hidden)]
pub use dataset::OirDataset;
#[doc(hidden)]
pub use meta::{
    AxisDesc, ChannelDesc, ChannelSettings, FrameGeometry, FrameRecord, ImageProperties, LaserLine,
    ObjectiveDesc,
};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "oir";

/// The Olympus/Evident OIR format reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct OirReader;

impl FormatReader for OirReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::OIR)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "Olympus/Evident OIR".into(),
            vendor: "Evident (Olympus)".into(),
            extensions: vec!["oir".into()],
            family: "microscopy".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::OIR.confidence,
            known_gaps: vec![
                "Compressed pixel blocks are not known to exist and are not handled; 4-byte samples are read as float32 on the strength of oirfile's documentation only (no corpus file)".into(),
                "RGB (colour camera) acquisitions are exposed as three planar channels, not as interleaved RGB; no corpus file".into(),
                "Line scans are exposed as XY planes whose height is the number of stored lines (no reinterpretation of Y as time); no corpus file".into(),
                "POIR/MPOIR archives (zip collections of OIR files, mosaic layouts in matl.omp2info) are not read; open the .oir files inside".into(),
                "Chunk names with axis letters other than t, l and z are listed by `check` but not exposed".into(),
                "Excitation wavelength is reported only when exactly one enabled laser line is linked to the channel".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if container::looks_like_oir(head) {
            let continuation = !has_extension(path, &["oir"]);
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: continuation.then(|| {
                    "OIR continuation file (pixel blocks of a multi-file series); open the .oir file of the series for its metadata".to_string()
                }),
            });
        }
        if has_extension(path, &["oir"]) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::ExtensionOnly,
                note: Some(
                    "`.oir` extension, but the file does not start with OLYMPUSRAWFORMAT".into(),
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
        let path = input.path();
        if !input.fs().is_file(path) {
            return Err(Error::io(
                path,
                std::io::Error::new(std::io::ErrorKind::NotFound, "not a file"),
            ));
        }
        Ok(Box::new(dataset::OirDataset::open(input)?))
    }
}
