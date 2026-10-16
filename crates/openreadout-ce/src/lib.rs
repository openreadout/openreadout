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
mod build;
mod fragment;
pub mod ladders;
pub mod sizing;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::series::{SeriesDataset, read_all};
use openreadout_core::source::Input;

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
                "Fragment sizes are computed here (Local Southern against the size standard the file names, else the best-fitting standard for the standard's dye); the instrument software's own sizing is not stored in the file".into(),
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
