//! Clean-room readers for X-ray diffraction scans.
//!
//! - **PANalytical / Malvern Panalytical XRDML** (`panalytical-xrdml`, `.xrdml`, the public XML
//!   schema of Data Collector: X'Pert, Empyrean, Aeris): every scan of every measurement with its
//!   axis positions, counting times, attenuation factors, wavelength, tube and optics.
//! - **Bruker DIFFRAC `.raw`** (`bruker-raw`: `RAW1.01` of DIFFRACplus and `RAW4.00` of
//!   DIFFRAC.SUITE): every range with its start, step, scanned drive, step time, tube settings,
//!   wavelengths, user and sample.
//! - **Bruker DIFFRAC.SUITE `.brml`** (`bruker-brml`, a zip of XML): every data route of every
//!   raw-data member, its columns as the file's data views name them, the instrument, tube and
//!   wavelengths.
//! - **Rigaku `.ras` and `.rasx`** (`rigaku-ras`, `rigaku-rasx`; SmartLab Studio, PDXL,
//!   MiniFlex): every scan with its header (axis, speed, step, tube, wavelengths, operator).
//!
//! Every scan is one trace: channel `intensity` (counts as stored, attenuation applied as the
//! instrument reports them), the scan axis in `extra.axis` (`two_theta`, `omega`, … in °; a listed
//! axis is channel 0). Notes: `docs/formats/xrd.md`.
//!
//! # Example
//!
//! ```
//! use openreadout_core::FormatReader;
//! use openreadout_xrd::XrdmlReader;
//!
//! assert_eq!(XrdmlReader.descriptor().id, openreadout_xrd::XRDML_FORMAT_ID);
//! ```
//!
//! # API stability
//!
//! The supported API is what this page documents: the readers and their format ids.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod assurance;
mod brml;
mod common;
mod raw;
mod rigaku;
mod xrdml;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::series::{SeriesDataset, read_all};
use openreadout_core::source::Input;

/// Format id of PANalytical XRDML files.
pub const XRDML_FORMAT_ID: &str = xrdml::FORMAT_ID;

/// Format id of Bruker DIFFRAC `.raw` files.
pub const BRUKER_RAW_FORMAT_ID: &str = raw::FORMAT_ID;

/// Format id of Bruker DIFFRAC.SUITE `.brml` files.
pub const BRML_FORMAT_ID: &str = brml::FORMAT_ID;
/// Format id of Rigaku `.ras` files.
pub const RAS_FORMAT_ID: &str = rigaku::RAS_FORMAT_ID;
/// Format id of Rigaku `.rasx` files.
pub const RASX_FORMAT_ID: &str = rigaku::RASX_FORMAT_ID;

/// Largest XRDML document read.
const MAX_XRDML: u64 = 512 << 20;

/// The PANalytical XRDML reader (`.xrdml`).
#[derive(Debug, Default, Clone, Copy)]
pub struct XrdmlReader;

impl FormatReader for XrdmlReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::PANALYTICAL_XRDML)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: XRDML_FORMAT_ID.into(),
            name: "PANalytical XRDML".into(),
            vendor: "Malvern Panalytical".into(),
            extensions: vec!["xrdml".into()],
            family: "diffraction".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::PANALYTICAL_XRDML.confidence,
            known_gaps: vec![
                "Area-detector frames (2D XRDML) are refused (exit 6); 1D scans of 1D and 2D detectors are read".into(),
                "Intensities are returned as stored (counts, attenuation applied, as the schema defines them); count rates are intensities divided by the counting time in `extra.counting_time_s`".into(),
                "Optics are kept in the vendor tree; only the tube, wavelength and detector name are normalized".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if xrdml::looks_like(head) {
            return Some(Detection {
                format_id: XRDML_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        has_extension(path, &["xrdml"]).then_some(Detection {
            format_id: XRDML_FORMAT_ID,
            confidence: DetectConfidence::ExtensionOnly,
            note: Some("XRDML extension but no xrdMeasurements element in the first 4 kB".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let bytes = read_all(input, XRDML_FORMAT_ID, MAX_XRDML)?;
        let file = xrdml::parse(&bytes)?;
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

/// Largest Bruker RAW file read.
const MAX_RAW: u64 = 1 << 30;

/// The Bruker DIFFRAC `.raw` reader (`RAW1.01`, `RAW4.00`).
#[derive(Debug, Default, Clone, Copy)]
pub struct BrukerRawReader;

impl FormatReader for BrukerRawReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::BRUKER_RAW)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: BRUKER_RAW_FORMAT_ID.into(),
            name: "Bruker DIFFRAC RAW".into(),
            vendor: "Bruker".into(),
            extensions: vec!["raw".into()],
            family: "diffraction".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::BRUKER_RAW.confidence,
            known_gaps: vec![
                "RAW1.01 (DIFFRACplus) and RAW4.00 (DIFFRAC.SUITE) are read; the older `RAW ` and `RAW2` versions are refused (exit 6)".into(),
                "Ranges whose records carry per-point parameters other than the measured 2θ, and RAW4 records wider than one float32, are refused (exit 6)".into(),
                "The scanned axis is the drive the range header marks as moving, else 2θ for coupled and detector scans; other scan types are refused".into(),
                "Detector, slit and goniometer settings are kept in the vendor tree; only the tube, wavelengths, step time, user and sample are normalized".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], _path: &Path) -> Option<Detection> {
        raw::looks_like(head).then_some(Detection {
            format_id: BRUKER_RAW_FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let bytes = read_all(input, BRUKER_RAW_FORMAT_ID, MAX_RAW)?;
        let file = raw::parse(&bytes)?;
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

/// Open a zip container held by `input`.
fn zip_of(input: &Input, format: &'static str) -> Result<openreadout_core::zip::ZipIndex> {
    openreadout_core::zip::ZipIndex::open(input.fs(), input.path(), format)
}

/// The Bruker DIFFRAC.SUITE `.brml` reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct BrmlReader;

impl FormatReader for BrmlReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::BRUKER_BRML)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: BRML_FORMAT_ID.into(),
            name: "Bruker DIFFRAC.SUITE BRML".into(),
            vendor: "Bruker".into(),
            extensions: vec!["brml".into()],
            family: "diffraction".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::BRUKER_BRML.confidence,
            known_gaps: vec![
                "2D detector frames (recorded views wider than one value) are refused (exit 6); integrated 1D counts are read".into(),
                "Counts are returned as stored; absorber factors are a separate channel and are not applied".into(),
                "Evaluation, template and instruction containers are not decoded".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        (openreadout_core::zip::is_zip(head) && has_extension(path, &["brml"])).then_some(
            Detection {
                format_id: BRML_FORMAT_ID,
                confidence: DetectConfidence::Likely,
                note: None,
            },
        )
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        if !openreadout_core::zip::is_zip(head) {
            return None;
        }
        let z = zip_of(input, BRML_FORMAT_ID).ok()?;
        brml::is_brml(z.members.iter().map(|m| m.name.as_str())).then_some(Detection {
            format_id: BRML_FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let z = zip_of(input, BRML_FORMAT_ID)?;
        let file = brml::parse(&z)?;
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

/// The Rigaku `.ras` reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct RasReader;

impl FormatReader for RasReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::RIGAKU_RAS)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: RAS_FORMAT_ID.into(),
            name: "Rigaku RAS".into(),
            vendor: "Rigaku".into(),
            extensions: vec!["ras".into()],
            family: "diffraction".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::RIGAKU_RAS.confidence,
            known_gaps: vec![
                "Intensities are returned as stored; the attenuation column is a separate channel and is not applied (a file with factors other than 1 is flagged)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if rigaku::looks_like_ras(head) {
            return Some(Detection {
                format_id: RAS_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        None.or_else(|| {
            (has_extension(path, &["ras"]) && head.starts_with(b"*RAS")).then_some(Detection {
                format_id: RAS_FORMAT_ID,
                confidence: DetectConfidence::Likely,
                note: None,
            })
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let bytes = read_all(input, RAS_FORMAT_ID, MAX_XRDML)?;
        let file = rigaku::parse_ras(&bytes)?;
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

/// The Rigaku `.rasx` reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct RasxReader;

impl FormatReader for RasxReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::RIGAKU_RASX)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: RASX_FORMAT_ID.into(),
            name: "Rigaku RASX".into(),
            vendor: "Rigaku".into(),
            extensions: vec!["rasx".into()],
            family: "diffraction".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::RIGAKU_RASX.confidence,
            known_gaps: vec![
                "Intensities are returned as stored; the attenuation column is a separate channel and is not applied (a file with factors other than 1 is flagged)".into(),
                "Area-detector images stored in a .rasx are not read".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        (openreadout_core::zip::is_zip(head) && has_extension(path, &["rasx"])).then_some(
            Detection {
                format_id: RASX_FORMAT_ID,
                confidence: DetectConfidence::Likely,
                note: None,
            },
        )
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        if !openreadout_core::zip::is_zip(head) {
            return None;
        }
        let z = zip_of(input, RASX_FORMAT_ID).ok()?;
        rigaku::is_rasx(z.members.iter().map(|m| m.name.as_str())).then_some(Detection {
            format_id: RASX_FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let z = zip_of(input, RASX_FORMAT_ID)?;
        let file = rigaku::parse_rasx(&z)?;
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
