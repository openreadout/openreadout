//! Bruker BAF (`analysis.baf` in a `.d` directory, written by otofControl, micrOTOFcontrol and
//! the FT-ICR control software): a chain of tagged records with per-spectrum headers,
//! centroid and profile blocks and calibrations. Layout and vocabulary:
//! `docs/formats/bruker-baf.md`. Provenance: `docs/provenance/bruker-baf.md`.

pub mod calibration;
pub mod dataset;
pub mod records;
pub mod spectrum;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader};
use openreadout_core::source::Input;

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "bruker-baf";

/// The Bruker BAF reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct BrukerBafReader;

/// Whether `head` starts with the BAF pointer record.
pub fn looks_like(head: &[u8]) -> bool {
    head.get(4..8) == Some(&records::TAG_POINTERS.to_le_bytes()[..])
}

impl FormatReader for BrukerBafReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::BRUKER_BAF)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "Bruker BAF (.d)".into(),
            vendor: "Bruker".into(),
            extensions: vec!["d".into(), "baf".into()],
            family: "mass-spectrometry".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::BRUKER_BAF.confidence,
            known_gaps: vec![
                "No specification: the layout was derived from public files and checked against the depositors' vendor-library conversions".into(),
                "m/z use the recalibration in Calibrator.ami when the directory holds one, found by scanning that file for a payload like the spectra's (its container is not decoded); without it, the acquisition calibration, which can differ from vendor exports of recalibrated runs by a few ppm".into(),
                "Calibration kinds other than time of flight 3 and 10 and the FT-ICR form, and profile codings other than float32 (plain or zlib-compressed) and the packed delta coding, are refused (exit 6)".into(),
                "Spectra are returned one by one as stored; vendor converters may combine repeated MS/MS of one precursor".into(),
                "The third centroid array (read as peak widths), several list records and analysis.baf_xtr are not decoded".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        self.sniff_input(head, &Input::local(path))
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        let fs = input.fs();
        let path = input.path();
        let (dir, baf) = dataset::locate(fs, path)?;
        // A directory arrives with an empty head; the file with its own.
        let content = if fs.is_dir(path) {
            let mut h = [0u8; 8];
            fs.open(&baf)
                .ok()
                .and_then(|f| f.read_exact_at(0, &mut h).ok())
                .is_some_and(|()| looks_like(&h))
        } else {
            looks_like(head)
        };
        let named_d = dir.extension().is_some_and(|e| e.eq_ignore_ascii_case("d"));
        Some(Detection {
            format_id: FORMAT_ID,
            confidence: if content {
                DetectConfidence::Definite
            } else {
                DetectConfidence::ExtensionOnly
            },
            note: (!named_d).then(|| "directory holds analysis.baf but is not named *.d".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(dataset::BafDataset::open(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
