//! Clean-room readers for the exports of small bench instruments.
//!
//! - **Thermo Fisher NanoDrop** (`nanodrop`): the NanoDrop 8000 report (`.txt`), the NanoDrop
//!   Eight data export (`.tsv`/`.txt`, with each sample's absorbance spectrum), the NanoDrop
//!   One report (`.csv`) and the NanoDrop One UV-Vis spectrum export (`.tsv`). Table 0 has one
//!   row per measurement: sample, time, concentration with its unit, A260, A280, A260/A280,
//!   A260/A230 and the application's other columns. Spectra are trace 0, one sweep per sample,
//!   on a wavelength axis in nm. Notes: `docs/formats/nanodrop.md`.
//! - **Thermo Fisher Qubit** (`qubit`): Qubit 4 and Qubit Flex exports (`.csv`): one row per
//!   tube read with the assay, the tube and original sample concentrations with their units,
//!   the dilution, the standards' and the sample's fluorescence. Notes: `docs/formats/qubit.md`.
//!
//! # Example
//!
//! ```
//! use openreadout_core::FormatReader;
//! use openreadout_benchtop::NanoDropReader;
//!
//! assert_eq!(NanoDropReader.descriptor().id, openreadout_benchtop::NANODROP_FORMAT_ID);
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::FormatReader;
//! use openreadout_benchtop::NanoDropReader;
//!
//! let mut dataset = NanoDropReader.open(Path::new("dsDNA.tsv"))?;
//! let samples = dataset.read_table(0, 0, u64::MAX)?;
//! println!("{} samples", samples.columns[0].len());
//! # Ok::<(), openreadout_core::Error>(())
//! ```
//!
//! # API stability
//!
//! The supported API is what this page documents: the readers, their format ids and the
//! dataset they return.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod assurance;
mod model;
mod nanodrop;
mod qubit;
mod table;
mod text;

use std::path::Path;

use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader};
use openreadout_core::source::Input;
use openreadout_core::{Error, Result};

pub use model::SampleTableDataset;

/// Format id of NanoDrop exports.
pub const NANODROP_FORMAT_ID: &str = nanodrop::FORMAT_ID;

/// Format id of Qubit exports.
pub const QUBIT_FORMAT_ID: &str = qubit::FORMAT_ID;

const VENDOR: &str = "Thermo Fisher Scientific";

/// Read a whole file (exports are small).
fn read_all(input: &Input, format: &'static str, cap: u64) -> Result<(Vec<u8>, u64)> {
    let f = input.open()?;
    let len = f.size().map_err(|e| Error::io(input.path(), e))?;
    if len > cap {
        return Err(Error::unsupported(
            format,
            format!("a {} MiB file", len >> 20),
            "Exports of this instrument are kilobytes to a few megabytes; this file is probably not one. Split it or open it with the vendor software.",
        ));
    }
    let n = usize::try_from(len).map_err(|_| Error::Other("file larger than memory".into()))?;
    let mut buf = vec![0u8; n];
    f.read_exact_at(0, &mut buf)
        .map_err(|e| Error::io(input.path(), e))?;
    Ok((buf, len))
}

/// The head of a file as text (at most 64 KiB, so a line may be cut).
fn head_text(head: &[u8]) -> String {
    text::decode(head)
}

/// The NanoDrop export reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct NanoDropReader;

impl FormatReader for NanoDropReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::NANODROP)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: NANODROP_FORMAT_ID.into(),
            name: "Thermo Fisher NanoDrop exports".into(),
            vendor: VENDOR.into(),
            extensions: vec!["txt".into(), "tsv".into(), "csv".into()],
            family: "spectrophotometer".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::NANODROP.confidence,
            known_gaps: vec![
                "Native NanoDrop workbooks (.twbk, .sql) and Excel saves of the exports are not read: export the data as text".into(),
                "The NanoDrop 8000's standards table (a second table after a blank line) is kept as text in the vendor metadata, not parsed".into(),
                "Ratios of the One's UV-Vis spectrum export are read off the spectrum without the nucleic-acid application's 340 nm baseline correction".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], _path: &Path) -> Option<Detection> {
        nanodrop::sniff(&head_text(head)).map(|layout| Detection {
            format_id: NANODROP_FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: Some(format!("{layout} layout")),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let (bytes, len) = read_all(input, NANODROP_FORMAT_ID, nanodrop::MAX_BYTES)?;
        let p = nanodrop::parse(&text::decode(&bytes))?;
        Ok(Box::new(SampleTableDataset::new(
            self.descriptor(),
            input.path().to_path_buf(),
            len,
            p,
            VENDOR,
            "CHMO:0000292",
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// The Qubit export reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct QubitReader;

impl FormatReader for QubitReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::QUBIT)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: QUBIT_FORMAT_ID.into(),
            name: "Thermo Fisher Qubit exports".into(),
            vendor: VENDOR.into(),
            extensions: vec!["csv".into()],
            family: "fluorometer".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::QUBIT.confidence,
            known_gaps: vec![
                "Excel saves of the exports are not read: export the data as CSV".into(),
                "Concentrations are the Qubit's own, as stored; the standard curve is not refitted"
                    .into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], _path: &Path) -> Option<Detection> {
        qubit::sniff(&head_text(head)).map(|layout| Detection {
            format_id: QUBIT_FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: Some(format!("{layout} layout")),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let (bytes, len) = read_all(input, QUBIT_FORMAT_ID, qubit::MAX_BYTES)?;
        let p = qubit::parse(&text::decode(&bytes))?;
        Ok(Box::new(SampleTableDataset::new(
            self.descriptor(),
            input.path().to_path_buf(),
            len,
            p,
            VENDOR,
            "CHMO:0000060",
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

#[doc(hidden)]
/// Parse a NanoDrop export from bytes (fuzz targets).
pub fn fuzz_nanodrop(bytes: &[u8]) {
    let _ = nanodrop::parse(&text::decode(bytes));
}

#[doc(hidden)]
/// Parse a Qubit export from bytes (fuzz targets).
pub fn fuzz_qubit(bytes: &[u8]) {
    let _ = qubit::parse(&text::decode(bytes));
}
