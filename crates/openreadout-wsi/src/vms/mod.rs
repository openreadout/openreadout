//! Hamamatsu VMS (`.vms`): a settings file naming a grid of large baseline JPEGs with restart
//! intervals, a low-resolution map JPEG, a macro photo and an optimisation file of MCU-row
//! offsets. Layout and vocabulary: `docs/formats/hamamatsu-vms.md`. Provenance:
//! `docs/provenance/hamamatsu-vms.md`.

pub mod dataset;
pub mod jpeg;
pub mod slide;

use std::path::Path;

use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;
use openreadout_core::{Error, Result};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "hamamatsu-vms";

/// First line of a VMS settings file.
pub const VMS_SECTION: &str = "Virtual Microscope Specimen";
/// First line of a VMU settings file (uncompressed NGR images; not read).
pub const VMU_SECTION: &str = "Uncompressed Virtual Microscope Specimen";

/// The Hamamatsu VMS reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct VmsReader;

/// Which settings section the file starts with, if any.
pub fn section_of(head: &[u8]) -> Option<&'static str> {
    let text = String::from_utf8_lossy(&head[..head.len().min(256)]);
    let first = text
        .trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())?;
    match first {
        l if l == format!("[{VMS_SECTION}]") => Some(VMS_SECTION),
        l if l == format!("[{VMU_SECTION}]") => Some(VMU_SECTION),
        _ => None,
    }
}

impl FormatReader for VmsReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::VMS)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "Hamamatsu VMS slide".into(),
            vendor: "Hamamatsu".into(),
            extensions: vec!["vms".into(), "vmu".into()],
            family: "microscopy".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::VMS.confidence,
            known_gaps: vec![
                "No specification: the layout comes from OpenSlide's public format page and one public slide, checked against OpenSlide run as a black box".into(),
                "Levels the file does not store (1/2 and 1/4 of the slide images, 1/2 to 1/8 of the map) are decoded at reduced DCT size, as OpenSlide does; their values are close to OpenSlide's, not identical".into(),
                "VMU files (uncompressed NGR images) and slides with more than one focal layer are refused (exit 6): no public sample".into(),
                "Level 0 is too large to read whole: read a region (`--region`) or a coarser level (`--level`)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        let ext = has_extension(path, &["vms", "vmu"]);
        let section = section_of(head);
        if !ext && section.is_none() {
            return None;
        }
        Some(Detection {
            format_id: FORMAT_ID,
            confidence: if section.is_some() {
                DetectConfidence::Definite
            } else {
                DetectConfidence::ExtensionOnly
            },
            note: (section == Some(VMU_SECTION))
                .then(|| "VMU (uncompressed NGR images): detected, not read".into()),
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
        Ok(Box::new(dataset::VmsDataset::open(input)?))
    }
}
