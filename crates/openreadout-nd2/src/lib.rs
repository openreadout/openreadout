//! Clean-room reader for Nikon ND2 files (chunk-based format).
//!
//! Layout and vocabulary: `docs/formats/nd2.md`. Provenance: `docs/provenance/nd2.md`.
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
//! use openreadout_nd2::Nd2Reader;
//!
//! let format = Nd2Reader.descriptor();
//! assert_eq!(format.id, openreadout_nd2::FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::{FormatReader, PlaneIndex};
//! use openreadout_nd2::Nd2Reader;
//!
//! let mut dataset = Nd2Reader.open(Path::new("timelapse.nd2"))?;
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
mod deinterleave;
#[doc(hidden)]
pub mod frames;
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing;
#[doc(hidden)]
pub mod legacy;
#[doc(hidden)]
pub mod lv;
#[doc(hidden)]
pub mod meta;
#[doc(hidden)]
pub mod variant;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;

#[doc(hidden)]
pub use container::{
    CHUNK_MAGIC, CHUNK_MAGIC_BYTES, CHUNK_MAP_SIGNATURE, ChunkHeader, ChunkMapEntry, FILE_MAP_NAME,
    FILE_SIGNATURE, LEGACY_SIGNATURE, Nd2File,
};
#[doc(hidden)]
pub use frames::CustomTag;
#[doc(hidden)]
pub use legacy::{BOX_MAP_SIGNATURE, LegacyBox, LegacyFile};
#[doc(hidden)]
pub use lv::lv_decode;
#[doc(hidden)]
pub use meta::{
    Attributes, CameraSetting, FrameLayout, Loop, LoopKind, Period, PlaneDesc, Position,
};
#[doc(hidden)]
pub use variant::{legacy_xml_decode, variant_decode};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "nd2";

/// The Nikon ND2 format reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct Nd2Reader;

impl FormatReader for Nd2Reader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::ND2)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "Nikon ND2".into(),
            vendor: "Nikon".into(),
            extensions: vec!["nd2".into()],
            family: "microscopy".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::ND2.confidence,
            known_gaps: vec![
                "Lossy-compressed frames (eCompression 1) are not decoded: no public sample exists to derive the codec from".into(),
                "Custom loops and unrecognized loop types are folded into T; NE-time sub-loops (pSubLoops) are not expanded".into(),
                "Legacy (JPEG 2000) files: events and per-frame times are read, ROIs and custom-data columns do not exist in that generation".into(),
                "ROI geometry is normalized from the documented tree layout but no corpus file contains ROIs; units are as stored".into(),
                "RGB planes carry no excitation/emission wavelengths (the nd2 package assigns pseudo-wavelengths; we do not)".into(),
                "RGB planes are returned R, G, B (modern files store B, G, R; validated on dye-coded planes); legacy JPEG 2000 RGB planes keep the codestream order their JP2 colour box declares as sRGB (inferred, no reference export)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if head.starts_with(&CHUNK_MAGIC_BYTES)
            && head.len() > 48
            && head[16..].starts_with(FILE_SIGNATURE.as_bytes())
        {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        if head.starts_with(&CHUNK_MAGIC_BYTES) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Likely,
                note: Some("chunk magic without the file signature".into()),
            });
        }
        if head.starts_with(&LEGACY_SIGNATURE) && has_extension(path, &["nd2"]) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Likely,
                note: Some("legacy JPEG 2000-based ND2".into()),
            });
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(dataset::Nd2Dataset::open(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
