//! Clean-room reader for Zeiss CZI files.
//!
//! Layout and vocabulary: `docs/formats/czi.md`. Provenance: `docs/provenance/czi.md`.
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
//! use openreadout_czi::CziReader;
//!
//! let format = CziReader.descriptor();
//! assert_eq!(format.id, openreadout_czi::FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::{FormatReader, PlaneIndex};
//! use openreadout_czi::CziReader;
//!
//! let mut dataset = CziReader.open(Path::new("experiment.czi"))?;
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
pub mod attach;
#[doc(hidden)]
pub mod container;
mod convert;
mod dataset;
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing;
#[doc(hidden)]
pub mod mask;
#[doc(hidden)]
pub mod xml;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader};
use openreadout_core::source::Input;

#[doc(hidden)]
pub use attach::{EventRecord, parse_event_list, parse_time_stamps};
#[doc(hidden)]
pub use container::{
    ATTACHMENT_DATA_OFFSET, AttachmentEntry, CompressionId, CziFile, DimensionEntry,
    DirectoryEntry, FileHeader, FilePart, PixelTypeId, SEGMENT_HEADER_LEN, SegmentHeader,
    SegmentId, SubBlockHeader, master_path, part_path,
};
#[doc(hidden)]
pub use mask::{MASK_CHUNK_GUID, ValidMask, parse_valid_mask};
#[doc(hidden)]
pub use xml::{
    ChannelXml, ExperimentXml, ImageXml, ObjectiveXml, ScalingXml, SceneXml, SubBlockTags,
    parse_subblock_tags,
};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "czi";
/// First 10 bytes of every CZI file (ASCII).
pub const FILE_MAGIC: &[u8; 10] = b"ZISRAWFILE";

/// The Zeiss CZI format reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct CziReader;

impl FormatReader for CziReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::CZI)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "Zeiss CZI".into(),
            vendor: "Carl Zeiss Microscopy".into(),
            extensions: vec!["czi".into()],
            family: "microscopy".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::CZI.confidence,
            known_gaps: vec![
                "JPEG (id 1) subblocks decode with jpeg-decoder (8-bit) and our own 12-bit decoder, which differ from libjpeg-turbo by up to a few grey levels on lossy streams (lossless JPEG is bit-exact); 12-bit progressive or chroma-subsampled JPEG is not decoded".into(),
                "Chunked subblocks (id 7) are validated on synthetic files only (imagecodecs-written; no public file uses id 7); the HiLo split over more than one chunk is refused because libCZI's documentation and imagecodecs read it differently".into(),
                "JPEG-lossless (id 3) and camera/system raw (id >= 100) subblocks are not decoded (no public sample)".into(),
                "Multi-file documents: part discovery (`<name> (<k>).czi`) is inferred from synthetic fixtures; no public multi-file CZI was available".into(),
                "Dimensions H, I, R, V, B that vary are one image per combination of coordinates; validated on H only (SIM phases; Airyscan-era multi-track files where some channels exist at H = 0 only, the others read as 0 and listed in extra.absent_channels); no public file varies along I, R, V or B".into(),
                "Scenes above 4 GiB are read by region (`--region`), not whole; pyramid levels and regions are read with `--level`/`--region`, and `export` copies the pyramid".into(),
                "Subblocks stored below their logical size without a pyramid flag (Airyscan fast-scan) are not upsampled".into(),
                "Complex-valued pixel types are not decoded".into(),
                "Super-resolved renderings (subblocks stored above their logical size, e.g. PALM) are exposed as their own image at the stored size, pixel size divided by the stored-to-logical ratio; they are not resampled onto the other channels' grid".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], _path: &Path) -> Option<Detection> {
        head.starts_with(FILE_MAGIC).then_some(Detection {
            format_id: FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(dataset::CziDataset::open(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
