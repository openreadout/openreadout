//! Philips iSyntax (`.isyntax`): an XML header ended by byte 0x04, a seek table, then the code
//! blocks of a wavelet pyramid in YCoCg. Layout and vocabulary: `docs/formats/philips-isyntax.md`.
//! Provenance: `docs/provenance/philips-isyntax.md` (a port of libisyntax, BSD-2-Clause).

pub mod codec;
pub mod dataset;
pub mod header;
pub mod pyramid;
pub mod render;
pub mod wavelet;

use std::path::Path;

use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;
use openreadout_core::{Error, Result};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "philips-isyntax";

/// The Philips iSyntax reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct IsyntaxReader;

/// Whether `head` starts like an iSyntax header (an XML declaration and the `DPUfsImport`
/// root object within the first bytes).
pub fn looks_like(head: &[u8]) -> bool {
    let n = head.len().min(512);
    let h = &head[..n];
    h.starts_with(b"<?xml") && openreadout_core::bytes::find(h, b"DPUfsImport").is_some()
}

impl FormatReader for IsyntaxReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::ISYNTAX)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "Philips iSyntax slide".into(),
            vendor: "Philips".into(),
            extensions: vec!["isyntax".into()],
            family: "microscopy".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::ISYNTAX.confidence,
            known_gaps: vec![
                "No public specification: the decoder is a port of libisyntax (BSD-2-Clause) and was validated against it on one public slide".into(),
                "Only data model 5 with compression method 16 is read; data model 100 and later (cluster headers) and compression method 19 are refused (exit 6) until a public sample exists".into(),
                "Pixel coordinates follow libisyntax: level L starts (3·2^levels − 3) >> L pixels into the tile grid; the scanner's own origin is not documented".into(),
                "Pixels are the decoded YCoCg wavelet data converted to RGB; the stored post-processing settings (sharpness, CLAHE) and the ICC profile are not applied".into(),
                "Level 0 is too large to read whole: read a region (`--region`) or a coarser level (`--level`)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        let ext = has_extension(path, &["isyntax"]);
        let content = looks_like(head);
        if !ext && !content {
            return None;
        }
        Some(Detection {
            format_id: FORMAT_ID,
            confidence: if content {
                DetectConfidence::Definite
            } else {
                DetectConfidence::ExtensionOnly
            },
            note: None,
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
        Ok(Box::new(dataset::IsyntaxDataset::open(input)?))
    }
}
