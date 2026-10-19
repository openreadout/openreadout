//! Reader for National Instruments TDMS files (`.tdms`): the format LabVIEW, DAQmx logging,
//! SignalExpress, FlexLogger and many custom acquisition programs write.
//!
//! A TDMS file is a series of segments, each a lead-in, optional metadata (objects and their
//! properties, raw-data indexes) and raw data. Objects are the file, groups and channels;
//! channels hold the values. `open` reads every lead-in and metadata section once and records
//! where each channel's values sit; values are read when a trace is. Channels become traces: one
//! trace per group when its channels share length and sample interval, else one per channel.
//! DAQmx raw data is read from its raw buffers and scaled with the `NI_Scale` properties.
//! Written from National Instruments' public description of the format. Notes:
//! `docs/formats/ni-tdms.md`; provenance: `docs/provenance/ni-tdms.md`.
//!
//! # Example
//!
//! ```
//! use openreadout_core::FormatReader;
//! use openreadout_tdms::TdmsReader;
//!
//! assert_eq!(TdmsReader.descriptor().id, openreadout_tdms::TDMS_FORMAT_ID);
//! ```
//!
//! # API stability
//!
//! The supported API is what this page documents: the reader, its format id and
//! [`TdmsDataset`] (returned boxed by `open`).
#![forbid(unsafe_code)]
#![warn(missing_docs)]
// Byte-layout code reads best with short names (f for the file, o for an object, p for a piece).
#![allow(clippy::many_single_char_names)]

mod assurance;
mod dataset;
mod parse;
mod values;

use std::path::Path;

use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;
use openreadout_core::{Error, Result};

pub use dataset::TdmsDataset;

/// Format id of TDMS files.
pub const TDMS_FORMAT_ID: &str = parse::FMT;

/// The TDMS reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct TdmsReader;

impl FormatReader for TdmsReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::TDMS)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: TDMS_FORMAT_ID.into(),
            name: "National Instruments TDMS".into(),
            vendor: "National Instruments (LabVIEW, DAQmx, SignalExpress)".into(),
            extensions: vec!["tdms".into()],
            family: "data-acquisition".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::TDMS.confidence,
            known_gaps: vec![
                "String channels are listed (count, first values in `vendor`), not returned as traces".into(),
                "Extended-precision, complex and fixed-point values, DAQmx digital-line data and scale types other than linear and polynomial are not read; `notes` name the channels".into(),
                "The `.tdms_index` file is not needed and not read: the segments of the `.tdms` file are walked directly".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if head.starts_with(b"TDSm") && head.len() >= 12 {
            return Some(Detection {
                format_id: TDMS_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        if head.starts_with(b"TDSh") {
            return Some(Detection {
                format_id: TDMS_FORMAT_ID,
                confidence: DetectConfidence::ExtensionOnly,
                note: Some(
                    "a TDMS index file (.tdms_index) holds no values: open the .tdms file beside it"
                        .into(),
                ),
            });
        }
        (has_extension(path, &["tdms"]) && !head.is_empty()).then(|| Detection {
            format_id: TDMS_FORMAT_ID,
            confidence: DetectConfidence::ExtensionOnly,
            note: Some("TDMS extension but no TDSm segment tag".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let f = input.open()?;
        let len = f.size().map_err(|e| Error::io(input.path(), e))?;
        let mut head = [0u8; 4];
        if f.read_exact_at(0, &mut head).is_ok() && &head == b"TDSh" {
            return Err(Error::unsupported(
                TDMS_FORMAT_ID,
                "TDMS index files (.tdms_index)",
                "Open the .tdms file with the same name: the index holds no values.",
            ));
        }
        let parsed = parse::parse(&f, len).map_err(|e| Error::corrupt(TDMS_FORMAT_ID, e))?;
        Ok(Box::new(TdmsDataset::new(
            input.path().to_path_buf(),
            f,
            len,
            self.descriptor(),
            parsed,
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
