//! HEKA PatchMaster bundle files (`.dat`): a bundle header, the samples, and the pulsed tree
//! (Root → Group → Series → Sweep → Trace). Notes: `docs/formats/heka-patchmaster.md`.

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;

pub mod bundle;
pub mod dataset;
pub mod tree;

/// Format id of PatchMaster bundles.
pub const HEKA_FORMAT_ID: &str = "heka-patchmaster";

/// The PatchMaster reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct HekaReader;

impl FormatReader for HekaReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::HEKA)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: HEKA_FORMAT_ID.into(),
            name: "HEKA PatchMaster bundle (.dat)".into(),
            vendor: "HEKA Elektronik (Harvard Bioscience) PatchMaster".into(),
            extensions: vec!["dat".into()],
            family: "electrophysiology".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::HEKA.confidence,
            known_gaps: vec![
                "Stimulus (PGF) templates, amplifier (.amp), solution and marker trees are not decoded; the command waveform is not synthesized".into(),
                "Files with separate .pul/.pgf files (DAT1), PULSE-era files, big-endian (PowerPC) files and PatchMaster Next v2000 (64-bit) bundles are refused".into(),
                "Non-zero Y offsets and scaled float traces are refused (never seen in a development file)".into(),
                "Values are not zero-subtracted; PatchMaster's zero level is reported per channel".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if bundle::looks_like_bundle(head) {
            return Some(Detection {
                format_id: HEKA_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        // `DATA` at byte 0 is the pre-bundle PatchMaster raw-data file; the extension alone is
        // shared with many formats, so it is never enough.
        if has_extension(path, &["dat"]) && head.get(..4) == Some(b"DATA") {
            return Some(Detection {
                format_id: HEKA_FORMAT_ID,
                confidence: DetectConfidence::Likely,
                note: Some("PatchMaster raw-data file without a bundle header (`DATA`); its trees are in separate files, which this reader does not open".into()),
            });
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(dataset::HekaDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(dataset::HekaDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
