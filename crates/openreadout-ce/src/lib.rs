//! Clean-room readers for capillary and chip electrophoresis files.
//!
//! - **Applied Biosystems ABIF** (`abif`: `.ab1`, `.abi`, `.fsa`, `.hid`), written by every
//!   Applied Biosystems genetic analyzer (310, 3100, 3130, 3500, 3730, SeqStudio). A sequencing
//!   file gives the base calls with their quality values and peak positions (table `base calls`)
//!   and the four dye channels, analysed and raw; a fragment-analysis file gives the dye
//!   channels sized in base pairs against the size standard (trace `sized`, sized here with the
//!   Local Southern method) and the standard's matched fragments (table `size standard`).
//!   Notes: `docs/formats/abif.md`.
//!
//! # Example
//!
//! ```
//! use openreadout_core::FormatReader;
//! use openreadout_ce::AbifReader;
//!
//! assert_eq!(AbifReader.descriptor().id, openreadout_ce::ABIF_FORMAT_ID);
//! ```
//!
//! Reading the base calls of a Sanger trace:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::FormatReader;
//! use openreadout_ce::AbifReader;
//!
//! let mut ds = AbifReader.open(Path::new("read.ab1"))?;
//! let info = ds.info()?;
//! let calls = ds.read_table(0, 0, u64::MAX)?; // position, base, quality, scan
//! println!("{} bases", info.tables[0].row_count);
//! # Ok::<(), openreadout_core::Error>(())
//! ```
//!
//! # API stability
//!
//! The supported API is what this page documents: the readers, their format ids, the size
//! standards ([`ladders`]) and the sizing functions ([`sizing`]).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod abif;
mod assurance;
mod bioanalyzer;
mod build;
mod chip;
mod fastq;
mod fragment;
pub mod ladders;
pub mod sizing;
mod tapestation;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::series::{SeriesDataset, read_all};
use openreadout_core::source::Input;

pub use fastq::{FastqExportReport, base_call_tables, default_fastq_output, export_fastq};

/// Format id of Applied Biosystems ABIF files.
pub const ABIF_FORMAT_ID: &str = abif::FORMAT_ID;

/// Largest ABIF file read (instrument files are under 2 MB).
const MAX_ABIF: u64 = 256 << 20;

/// The Applied Biosystems ABIF reader (`.ab1`, `.abi`, `.fsa`, `.hid`).
#[derive(Debug, Default, Clone, Copy)]
pub struct AbifReader;

impl FormatReader for AbifReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::ABIF)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: ABIF_FORMAT_ID.into(),
            name: "Applied Biosystems ABIF".into(),
            vendor: "Applied Biosystems (Thermo Fisher Scientific)".into(),
            extensions: vec!["ab1".into(), "abi".into(), "fsa".into(), "hid".into()],
            family: "electrophoresis".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::ABIF.confidence,
            known_gaps: vec![
                "Fragment sizes are the instrument's when the file stores its size map (Data Collection 3 and later), else computed here by Local Southern against the size standard the file names, or the best-fitting standard for the standard's dye".into(),
                "Base calls are the basecaller's (PBAS2/PCON2/PLOC2); edited calls, when they differ, are a second table".into(),
                "Values above 32767 that the instrument lists apart (OvrI/OvrV) are not merged into the raw trace".into(),
                "Legacy and user element types (scan rate, feature tables) are kept as bytes in the vendor tree".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if abif::looks_like(head) {
            return Some(Detection {
                format_id: ABIF_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        has_extension(path, &["ab1", "abi", "fsa", "hid"]).then_some(Detection {
            format_id: ABIF_FORMAT_ID,
            confidence: DetectConfidence::ExtensionOnly,
            note: Some("ABIF extension but the file does not start with `ABIF`".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let bytes = read_all(input, ABIF_FORMAT_ID, MAX_ABIF)?;
        let parsed = abif::parse(&bytes)?;
        let file = build::build(&parsed);
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

/// Format id of Agilent 2100 Bioanalyzer XML exports.
pub const BIOANALYZER_FORMAT_ID: &str = bioanalyzer::FORMAT_ID;
/// Format id of Agilent TapeStation exports (XML with its electropherogram CSV).
pub const TAPESTATION_FORMAT_ID: &str = tapestation::FORMAT_ID;

/// Largest export read before decompression.
const MAX_EXPORT: u64 = 512 << 20;

/// The decompressed head of a file (gzip or plain), for detection.
fn plain_head(head: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    match openreadout_core::gzip::peek(head, 8192) {
        Some(v) if openreadout_core::gzip::is_gzip(head) => std::borrow::Cow::Owned(v),
        _ => std::borrow::Cow::Borrowed(head),
    }
}

/// The Agilent 2100 Bioanalyzer XML export reader (`.xml`, `.xml.gz` from 2100 Expert).
#[derive(Debug, Default, Clone, Copy)]
pub struct BioanalyzerReader;

impl FormatReader for BioanalyzerReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::BIOANALYZER)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: BIOANALYZER_FORMAT_ID.into(),
            name: "Agilent 2100 Bioanalyzer XML export".into(),
            vendor: "Agilent".into(),
            extensions: vec!["xml".into()],
            family: "electrophoresis".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::BIOANALYZER.confidence,
            known_gaps: vec![
                "Native .xad files are not read: export the chip from 2100 Expert as XML (File > Export)".into(),
                "Each well's processed signal is sized over the ladder's range only; signal outside it is left out".into(),
                "The raw (unprocessed) signal and the gel image are not read".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], _path: &Path) -> Option<Detection> {
        bioanalyzer::looks_like(&plain_head(head)).then_some(Detection {
            format_id: BIOANALYZER_FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let raw = read_all(input, BIOANALYZER_FORMAT_ID, MAX_EXPORT)?;
        let size = raw.len() as u64;
        let bytes = chip::plain(raw, BIOANALYZER_FORMAT_ID)?;
        let file = bioanalyzer::parse(&chip::text(&bytes))?;
        Ok(Box::new(SeriesDataset::new(
            self.descriptor(),
            input.path().to_path_buf(),
            size,
            file,
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// The Agilent TapeStation export reader: the XML of results, with
/// `<name>_Electropherogram.csv` next to it.
#[derive(Debug, Default, Clone, Copy)]
pub struct TapeStationReader;

impl FormatReader for TapeStationReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::TAPESTATION)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: TAPESTATION_FORMAT_ID.into(),
            name: "Agilent TapeStation export".into(),
            vendor: "Agilent".into(),
            extensions: vec!["xml".into()],
            family: "electrophoresis".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::TAPESTATION.confidence,
            known_gaps: vec![
                "Native run files (.D1000, .RNA, ...) are not read: export the XML and the electropherogram CSV from TapeStation Analysis".into(),
                "Lane signals are sized here against the ladder (aligned on the markers), not by the instrument; peak and region sizes are the instrument's".into(),
                "Without the electropherogram CSV next to the XML, only the results are read".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], _path: &Path) -> Option<Detection> {
        tapestation::looks_like(&plain_head(head)).then_some(Detection {
            format_id: TAPESTATION_FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let raw = read_all(input, TAPESTATION_FORMAT_ID, MAX_EXPORT)?;
        let size = raw.len() as u64;
        let xml = chip::text(&chip::plain(raw, TAPESTATION_FORMAT_ID)?);
        let csv_path = tapestation::csv_candidates(input.path())
            .into_iter()
            .find(|p| input.fs().exists(p));
        let csv = match &csv_path {
            Some(p) => {
                let b = read_all(
                    &input.with_path(p.clone()),
                    TAPESTATION_FORMAT_ID,
                    MAX_EXPORT,
                )?;
                Some(chip::text(&chip::plain(b, TAPESTATION_FORMAT_ID)?))
            }
            None => None,
        };
        let name = csv_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string());
        let mut file = tapestation::parse(&xml, csv.as_deref(), name.as_deref())?;
        file.members = csv_path.into_iter().collect();
        Ok(Box::new(SeriesDataset::new(
            self.descriptor(),
            input.path().to_path_buf(),
            size,
            file,
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
