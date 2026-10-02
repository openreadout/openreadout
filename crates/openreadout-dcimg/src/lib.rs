//! Clean-room reader for Hamamatsu DCIMG camera streams (ORCA-Flash, ORCA-Fusion).
//!
//! A DCIMG file is one camera stream: a file header, a session header with the frame geometry,
//! the frames, and each frame's camera counter and time stamp — in tables after the last frame
//! (format version 7) or in a trailer after every frame (version 0x1000000). Frames are exposed
//! as the time points of one image. Some cameras overwrite the first pixels of one sensor row
//! in every frame and store their real values separately; the reader puts them back.
//!
//! Derived from hex dumps of corpus files and the documentation of the `dcimg` Python package
//! (MIT, Giacomo Mazzamuto); Bio-Formats is run only as a black-box oracle. Layout and
//! vocabulary: `docs/formats/dcimg.md`. Provenance: `docs/provenance/dcimg.md`.
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
//! use openreadout_dcimg::DcimgReader;
//!
//! let format = DcimgReader.descriptor();
//! assert_eq!(format.id, openreadout_dcimg::FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::{FormatReader, PlaneIndex};
//! use openreadout_dcimg::DcimgReader;
//!
//! let mut dataset = DcimgReader.open(Path::new("stream.dcimg"))?;
//! let info = dataset.info()?; // headers only
//! let image = &info.images[0];
//! println!("{} x {} px, {} frames", image.size_x, image.size_y, image.size_t);
//! // Frames are time points: read the third one.
//! let plane = dataset.read_plane(0, PlaneIndex { c: 0, z: 0, t: 2 })?;
//! assert_eq!(plane.data.len(), plane.expected_len());
//! // Camera frame counters and time stamps, one record per frame.
//! let (total, records) = dataset.frames(0, Some(10))?;
//! println!("{total} frames; first: {}", records[0]);
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
pub mod layout;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader};

#[doc(hidden)]
pub use dataset::DcimgDataset;
#[doc(hidden)]
pub use layout::{
    CameraText, DCIMG_MAGIC, DcimgLayout, FrameLayout, FrameStamp, Problem, StoredPixels,
    VERSION_FRAMED, VERSION_PACKED, looks_like_dcimg,
};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "dcimg";

/// The DCIMG reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct DcimgReader;

impl FormatReader for DcimgReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::DCIMG)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "Hamamatsu DCIMG".into(),
            vendor: "Hamamatsu Photonics".into(),
            extensions: vec!["dcimg".into()],
            family: "microscopy".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::DCIMG.confidence,
            known_gaps: vec![
                "Derived without a specification from 15 corpus files (versions 7 and 0x1000000; ORCA-Flash4.0 and ORCA-Fusion; 16-bit); 8-bit frames are inferred".into(),
                "Each file is one image: Bio-Formats groups sibling files named `<stem>_NNN_NNN.dcimg` into a Z stack, this reader does not (nothing in the file says the siblings belong together)".into(),
                "Planes are returned in stored row order; Bio-Formats returns them bottom row first".into(),
                "Only the first session of a file is read (no corpus file has more than one)".into(),
                "No physical pixel size: the file records none".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], _path: &Path) -> Option<Detection> {
        looks_like_dcimg(head).then_some(Detection {
            format_id: FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(DcimgDataset::open(path)?))
    }

    fn open_input(&self, input: &openreadout_core::source::Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(DcimgDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
