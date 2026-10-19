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
//! - **Bio-Rad droplet digital PCR** (`biorad-ddpcr`): QuantaSoft and QX Manager CSV exports.
//!   A results export is table 0 `results`, one row per well and target: concentration in
//!   copies/µL with its confidence intervals, positive, negative and accepted droplets. An
//!   amplitude export is table 0 `droplets`, one row per droplet of one well: the channel
//!   amplitudes and the cluster or per-target calls. Notes: `docs/formats/biorad-ddpcr.md`.
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
mod ddpcr;
mod model;
mod nanodrop;
mod qlp;
mod qubit;
mod table;
mod text;

use std::path::Path;

use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader};
use openreadout_core::source::Input;
use openreadout_core::{Error, Result};

pub use model::SampleTableDataset;
pub use qlp::QlpDataset;

/// Format id of NanoDrop exports.
pub const NANODROP_FORMAT_ID: &str = nanodrop::FORMAT_ID;

/// Format id of Qubit exports.
pub const QUBIT_FORMAT_ID: &str = qubit::FORMAT_ID;

/// Format id of Bio-Rad droplet digital PCR exports.
pub const DDPCR_FORMAT_ID: &str = ddpcr::FORMAT_ID;

/// Format id of Bio-Rad QuantaSoft plate files (`.qlp`).
pub const QLP_FORMAT_ID: &str = qlp::FORMAT_ID;

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

/// The Bio-Rad droplet digital PCR export reader (QuantaSoft, QX Manager).
#[derive(Debug, Default, Clone, Copy)]
pub struct DdpcrReader;

impl FormatReader for DdpcrReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::DDPCR)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: DDPCR_FORMAT_ID.into(),
            name: "Bio-Rad droplet digital PCR exports (QuantaSoft, QX Manager)".into(),
            vendor: "Bio-Rad".into(),
            extensions: vec!["csv".into()],
            family: "qpcr".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::DDPCR.confidence,
            known_gaps: vec![
                "QX Manager `.ddpcr` files are encrypted archives and are not read: export the results and amplitudes as CSV".into(),
                "Concentrations, confidence intervals and droplet counts are the software's own, as exported; nothing is recomputed from the amplitudes".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], _path: &Path) -> Option<Detection> {
        ddpcr::sniff(&head_text(head)).map(|layout| Detection {
            format_id: DDPCR_FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: Some(format!("{layout} layout")),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let (bytes, len) = read_all(input, DDPCR_FORMAT_ID, ddpcr::MAX_BYTES)?;
        let name = input
            .path()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let p = ddpcr::parse(&text::decode(&bytes), &name)?;
        Ok(Box::new(SampleTableDataset::new(
            self.descriptor(),
            input.path().to_path_buf(),
            len,
            p,
            "Bio-Rad",
            "",
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// The Bio-Rad QuantaSoft plate file reader (`.qlp`).
#[derive(Debug, Default, Clone, Copy)]
pub struct QlpReader;

impl FormatReader for QlpReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::QLP)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: QLP_FORMAT_ID.into(),
            name: "Bio-Rad QuantaSoft droplet digital PCR plate".into(),
            vendor: "Bio-Rad".into(),
            extensions: vec!["qlp".into()],
            family: "qpcr".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::QLP.confidence,
            known_gaps: vec![
                "Concentrations and confidence intervals are not stored in the file and are not computed; the droplet counts per class are".into(),
                "Each droplet's third value per channel and the per-droplet flags are not read".into(),
                "Plate setups in a layout other than QuantaSoft 1.7's 1386-byte well records are left out (samples and targets empty)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], _path: &Path) -> Option<Detection> {
        qlp::sniff(head).then_some(Detection {
            format_id: QLP_FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(QlpDataset::open(input, self.descriptor())?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

#[doc(hidden)]
/// Parse a ddPCR export from bytes (fuzz targets).
pub fn fuzz_ddpcr(bytes: &[u8]) {
    let _ = ddpcr::parse(&text::decode(bytes), "fuzz_A01_Amplitude.csv");
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
