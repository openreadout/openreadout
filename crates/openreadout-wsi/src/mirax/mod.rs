//! 3DHISTECH MIRAX (`.mrxs`): a `.mrxs` preview JPEG next to a directory of the same name with
//! `Slidedat.ini` (settings), `Index.dat` (where every stored image lies) and `Data*.dat` (the
//! images). Layout and vocabulary: `docs/formats/mirax.md`. Provenance: `docs/provenance/mirax.md`.

pub mod dataset;
pub mod index;
pub mod ini;
pub mod render;
pub mod slide;

use std::path::Path;

use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;
use openreadout_core::{Error, Result};

pub use slide::FORMAT_ID;

/// The 3DHISTECH MIRAX reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct MiraxReader;

impl FormatReader for MiraxReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::MIRAX)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "3DHISTECH MIRAX slide".into(),
            vendor: "3DHISTECH".into(),
            extensions: vec!["mrxs".into()],
            family: "microscopy".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::MIRAX.confidence,
            known_gaps: vec![
                "No specification: the layout comes from OpenSlide's public format documentation and the corpus files; placement of camera photos was derived by comparison with OpenSlide run as a black box".into(),
                "Levels above 0 place camera photos at fractional pixels: values are bilinear resamplings as OpenSlide renders them (within a few grey levels of OpenSlide), not stored samples".into(),
                "Slides without a camera position table (exported with overlaps removed) are listed but their pixels are refused (exit 6): no such development file yet".into(),
                "Fluorescence slides with more than three filters, or filters sharing a colour component, are refused (exit 6)".into(),
                "Level 0 is too large to read whole: read a region (`--region`) or a coarser level (`--level`)".into(),
                "The scan-information records (focus, motor positions) and stitching records are listed as attachments, not decoded".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if !has_extension(path, &["mrxs"]) {
            return None;
        }
        // The .mrxs itself is a JPEG preview; `sniff_input` also looks for the data directory.
        Some(Detection {
            format_id: FORMAT_ID,
            confidence: if head.starts_with(&[0xFF, 0xD8]) {
                DetectConfidence::Definite
            } else {
                DetectConfidence::ExtensionOnly
            },
            note: None,
        })
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        let path = input.path();
        if !has_extension(path, &["mrxs"]) {
            return None;
        }
        let fs = input.fs();
        let has_dir = slide::data_dir(fs, path).is_some();
        Some(Detection {
            format_id: FORMAT_ID,
            confidence: if head.starts_with(&[0xFF, 0xD8]) && has_dir {
                DetectConfidence::Definite
            } else {
                DetectConfidence::ExtensionOnly
            },
            note: (!has_dir).then(|| {
                "`.mrxs` without its data directory (the same name without extension)".into()
            }),
        })
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
        Ok(Box::new(dataset::MiraxDataset::open(input)?))
    }
}
