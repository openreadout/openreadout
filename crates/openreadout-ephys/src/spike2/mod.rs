//! CED Spike2 data files: 32-bit `.smr` and 64-bit `.smrx`. Notes: `docs/formats/ced-spike2.md`.

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;

pub mod dataset;
pub mod file;
pub mod file64;
#[cfg(test)]
mod tests;

/// Format id of Spike2 files.
pub const SPIKE2_FORMAT_ID: &str = "ced-spike2";

/// The Spike2 reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct Spike2Reader;

impl FormatReader for Spike2Reader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::SPIKE2)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: SPIKE2_FORMAT_ID.into(),
            name: "CED Spike2 data file (.smr, .smrx)".into(),
            vendor: "Cambridge Electronic Design (CED) Spike2".into(),
            extensions: vec!["smr".into(), "smrx".into()],
            family: "electrophysiology".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::SPIKE2.confidence,
            known_gaps: vec![
                "64-bit .smrx: the layout is derived from public files and Spike2's own exports (no documentation); Adc, rising events and empty marker channels are validated, RealWave, AdcMark/RealMark/TextMark and level events follow the same rules unvalidated".into(),
                "Level-event channels are listed as event times; the level after each edge is not reported".into(),
                "Interleaved AdcMark waveforms are returned in stored (interleaved) order".into(),
                "Spike2 memory and virtual channels are not reconstructed; only stored channels are read".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if file::looks_like_smr(head) || file::looks_like_smrx(head) {
            return Some(Detection {
                format_id: SPIKE2_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        has_extension(path, &["smr", "smrx"]).then(|| Detection {
            format_id: SPIKE2_FORMAT_ID,
            confidence: DetectConfidence::ExtensionOnly,
            note: Some("Spike2 extension but no `(C) CED 87` or `S64` signature".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(dataset::Spike2Dataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(dataset::Spike2Dataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
