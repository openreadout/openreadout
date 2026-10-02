//! Clean-room readers for thermal-analysis instrument files.
//!
//! - **NETZSCH Proteus** (`netzsch-ngb`: `.ngb-ss3`, `.ngb-bs3`, `.ngb-ds3` from STA/TG
//!   instruments, `.ngb-sd7` from DSC, `.ngb-dla`/`.ngb-cla` from dilatometers): every channel of
//!   every measurement run (time, sample temperature, DSC signal, mass, gas flows, length change,
//!   furnace channels) as one trace per run, with the instrument, operator, sample and date.
//!
//! - **TA Instruments** (`ta-universal-analysis`: the `.001`, `.002`, … data files of Q-series DSC,
//!   TGA, SDT, DMA and TMA instruments that Universal Analysis reads): every stored signal as a
//!   channel, with the instrument, serial, operator, sample, size and method.
//!
//! - **TA Instruments TRIOS** (`ta-trios`: `.tri` files of DSC, TGA, DMA and Discovery
//!   rheometers): one trace per procedure step with its stored signals, and for parallel-plate
//!   oscillation steps the moduli TRIOS computes. Notes: `docs/formats/ta-trios.md`.
//!
//! A run is one trace: channel 0 `time` (s; the abscissa, `extra.axis` irregular), then one
//! channel per stored signal in our vocabulary, values as stored (the DSC signal in µV, not
//! converted with the vendor's sensitivity calibration). Notes: `docs/formats/netzsch-ngb.md`.
//!
//! # Example
//!
//! ```
//! use openreadout_core::FormatReader;
//! use openreadout_thermal::NgbReader;
//!
//! assert_eq!(NgbReader.descriptor().id, openreadout_thermal::NGB_FORMAT_ID);
//! ```
//!
//! # API stability
//!
//! The supported API is what this page documents: the readers and their format ids.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod assurance;
mod ngb;
mod ta;
mod trios;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader};
use openreadout_core::series::SeriesDataset;
use openreadout_core::source::Input;
use openreadout_core::zip::ZipIndex;

/// Format id of NETZSCH Proteus `.ngb-*` measurement files.
pub const NGB_FORMAT_ID: &str = ngb::FORMAT_ID;

/// Format id of TA Instruments data files as Universal Analysis reads them (`.001`, …).
pub const TA_FORMAT_ID: &str = ta::FORMAT_ID;

/// Format id of TA Instruments TRIOS files (`.tri`).
pub const TRIOS_FORMAT_ID: &str = trios::FORMAT_ID;

/// The file extension letters after `.ngb-` of Proteus measurement files.
fn ngb_extension(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    let (_, ext) = name.rsplit_once(".ngb-")?;
    (ext.len() == 3 && ext.chars().all(|c| c.is_ascii_alphanumeric())).then(|| ext.to_string())
}

/// The NETZSCH Proteus `.ngb-*` reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct NgbReader;

impl FormatReader for NgbReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::NETZSCH_NGB)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: NGB_FORMAT_ID.into(),
            name: "NETZSCH Proteus (.ngb-*)".into(),
            vendor: "NETZSCH".into(),
            extensions: ["ngb-ss3", "ngb-bs3", "ngb-ds3", "ngb-sd7", "ngb-bd7", "ngb-dla", "ngb-cla"]
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            family: "thermal-analysis".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::NETZSCH_NGB.confidence,
            known_gaps: vec![
                "The DSC signal is returned in µV as stored; Proteus's sensitivity calibration (to mW, mW/mg) is not applied".into(),
                "The temperature program, PID settings and calibration tables are not decoded".into(),
                "Analysis (.ngb-taa) and state (.ngb-od7) files are not measurement files and are refused".into(),
                "Channels without a known meaning are returned as channel_<id> without a unit".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        (openreadout_core::zip::is_zip(head) && ngb_extension(path).is_some()).then_some(
            Detection {
                format_id: NGB_FORMAT_ID,
                confidence: DetectConfidence::Likely,
                note: None,
            },
        )
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        if !openreadout_core::zip::is_zip(head) {
            return None;
        }
        let z = ZipIndex::open(input.fs(), input.path(), NGB_FORMAT_ID).ok()?;
        z.has("Streams/stream_1.table").then_some(Detection {
            format_id: NGB_FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let z = ZipIndex::open(input.fs(), input.path(), NGB_FORMAT_ID)?;
        let mut file = ngb::parse(&z)?;
        if let (Some(ext), serde_json::Value::Object(v)) =
            (ngb_extension(input.path()), &mut file.vendor)
        {
            v.insert("extension".into(), serde_json::json!(ext));
            v.insert(
                "instrument_kind".into(),
                serde_json::json!(ngb::kind_of(&ext)),
            );
        }
        Ok(Box::new(SeriesDataset::new(
            self.descriptor(),
            input.path().to_path_buf(),
            z.file_len,
            file,
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// The TA Instruments data-file reader (`.001`, `.002`, … as Universal Analysis reads them).
#[derive(Debug, Default, Clone, Copy)]
pub struct TaReader;

impl FormatReader for TaReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::TA_UNIVERSAL_ANALYSIS)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: TA_FORMAT_ID.into(),
            name: "TA Instruments data file (Universal Analysis, .001)".into(),
            vendor: "TA Instruments".into(),
            extensions: vec!["001".into()],
            family: "thermal-analysis".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::TA_UNIVERSAL_ANALYSIS.confidence,
            known_gaps: vec![
                "Values are the stored signals; Universal Analysis's analyses (onsets, peak areas, normalisation by size) are not computed".into(),
                "Only the header key/value lines are decoded; calibration lines are kept in the vendor tree".into(),
                "TRIOS (.tri) files are a different format (`ta-trios`)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], _path: &Path) -> Option<Detection> {
        ta::looks_like(head).then_some(Detection {
            format_id: TA_FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let bytes = openreadout_core::series::read_all(input, TA_FORMAT_ID, 4 << 30)?;
        let file = ta::parse(&bytes)?;
        Ok(Box::new(SeriesDataset::new(
            self.descriptor(),
            input.path().to_path_buf(),
            bytes.len() as u64,
            file,
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// The TA Instruments TRIOS reader (`.tri`: DSC, TGA, DMA and Discovery rheometers).
#[derive(Debug, Default, Clone, Copy)]
pub struct TriosReader;

impl FormatReader for TriosReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::TA_TRIOS)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: TRIOS_FORMAT_ID.into(),
            name: "TA Instruments TRIOS (.tri)".into(),
            vendor: "TA Instruments".into(),
            extensions: vec!["tri".into()],
            family: "thermal-analysis".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::TA_TRIOS.confidence,
            known_gaps: vec![
                "Values are the stored signals; variables TRIOS computes (normalised heat flow, derivatives, viscosities of flow steps, DMA moduli) are not, except the oscillation moduli of parallel-plate rheometer steps".into(),
                "Signals of unknown meaning are returned as signal_<guid> without a unit".into(),
                "Files of the 2019 generation (TRIOS 3/4, byte 2 = 12) store their data differently and are refused".into(),
                "Per-point oscillation waveforms and analysis results stored in the file are not decoded".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], _path: &Path) -> Option<Detection> {
        trios::looks_like(head).then_some(Detection {
            format_id: TRIOS_FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let bytes = openreadout_core::series::read_all(input, TRIOS_FORMAT_ID, 4 << 30)?;
        let file = trios::parse(&bytes)?;
        Ok(Box::new(SeriesDataset::new(
            self.descriptor(),
            input.path().to_path_buf(),
            bytes.len() as u64,
            file,
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
