//! Gatan Digital Micrograph DM3 / DM4 / DM5 reader.
//!
//! Layout and vocabulary: `docs/formats/dm.md`. Provenance: `docs/provenance/dm.md`.
//! A file is a short big-endian header followed by a tree of tag groups and data tags; images
//! live in `ImageList`, each with `ImageData` (`Data`, `DataType`, `Dimensions`, `Calibrations`)
//! and `ImageTags` (microscope and acquisition metadata). DM5 stores the same tree in HDF5.

mod dataset;
mod dm5;
mod tags;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;

pub use dataset::{AxisMap, Calibration, DmDataset, DmImage, ImageDataType, length_to_um};
pub use dm5::Dm5Data;
pub use tags::{
    DmHeader, ElementType, INLINE_ARRAY_LIMIT, MAX_DEPTH, ParseIssue, Tag, TagGroup, TagType,
    TagValue,
};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "dm";

/// Header bytes that look like DM3/DM4: version, byte-order word, then a root group whose
/// sorted/open flags are 0 or 1.
pub fn looks_like_dm(head: &[u8]) -> bool {
    let Some(h) = DmHeader::parse(head) else {
        return false;
    };
    let at = usize::try_from(h.header_len).unwrap_or(usize::MAX);
    matches!(head.get(at..at + 2), Some([a, b]) if *a <= 1 && *b <= 1)
}

/// The DM3/DM4 reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct DmReader;

impl FormatReader for DmReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::DM)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "Gatan Digital Micrograph (DM3/DM4/DM5)".into(),
            vendor: "Gatan".into(),
            extensions: vec!["dm3".into(), "dm4".into(), "dm5".into()],
            family: "electron-microscopy".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::DM.confidence,
            known_gaps: vec![
                "Old-style packed complex (DataType 5), RGB (8) and the other RGB/RGBA variants (15-22, 24-26) are described but not decoded (exit 6): no public file shows their byte layout; complex (3, 13), RGBA (23), binary (14) and 64-bit integers are decoded".into(),
                "Packed Fourier transforms (DataType 27, 28) are returned as the stored half plane (X/2 + 1 columns), not mirrored into the full plane".into(),
                "Thumbnails are exposed as raw-pixel attachments, not images".into(),
                "Data with more than three dimensions (4D-STEM) is returned as frames of the first two dimensions, the scan positions flattened into T (extra.frame_grid gives the grid)".into(),
                "Line plots overlaying several images (ImageSourceList) are returned image by image; display settings and annotations are listed by `info --view full`, not interpreted".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if has_extension(path, &["dm5"]) && crate::emd::looks_like_hdf5(head) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: Some("HDF5 file with the .dm5 extension (DigitalMicrograph 5)".into()),
            });
        }
        if looks_like_dm(head) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        if has_extension(path, &["dm3", "dm4", "dm5"]) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::ExtensionOnly,
                note: Some(
                    "DM extension, but the file does not start with a DM3/DM4 header or an HDF5 signature".into(),
                ),
            });
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(DmDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(DmDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
