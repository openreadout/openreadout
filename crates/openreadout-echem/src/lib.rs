//! Clean-room readers for electrochemistry and battery-cycler data.
//!
//! - **BioLogic EC-Lab** (`biologic-mpr`, the binary `.mpr`; `biologic-mpt`, its `.mpt` text
//!   export): every stored column of every technique (CV, LSV, CA, CP, OCV, GCPL, Modulo Bat,
//!   PEIS/GEIS impedance, …) as channels of one trace sampled in time, with the technique and the
//!   acquisition start.
//! - **Gamry Framework** (`gamry-dta`, the `EXPLAIN` text `.DTA`): every table (the cycles of a
//!   CV as sweeps of one trace, impedance `ZCURVE`, the open-circuit record) with the header
//!   parameters.
//! - **Neware BTS** (`neware-nda`, the binary `.nda` of BTS 7-9; `neware-ndax`, the zip of `.ndc`
//!   page files of BTS 8+): every record of a battery-cycling test (voltage, current, step time,
//!   capacities and energies, cycle, program step and step type, auxiliary temperatures and
//!   voltages) and the executed steps.
//! - **Arbin MITS Pro** (`arbin-res`, the `.res` result database, a Microsoft Jet 4 file): every
//!   data point of a battery test (test and step time, current, voltage, capacities, energies,
//!   dV/dt, internal resistance, cycle and step), auxiliary voltages and temperatures, the
//!   per-cycle statistics and the events, with the test facts.
//!
//! A record is one trace: channel 0 `time` (s; the abscissa, `extra.axis` irregular) and one
//! channel per stored column in our vocabulary (`ewe`, `current`, `charge`, `cycle`, `z_real`, …;
//! EC-Lab's own label in the channel's `extra.label`). Values are as stored; quantities EC-Lab
//! computes only when it exports text (energies, capacities, efficiency) are not invented.
//! Notes: `docs/formats/biologic-eclab.md`.
//!
//! # Example
//!
//! ```
//! use openreadout_core::FormatReader;
//! use openreadout_echem::MprReader;
//!
//! assert_eq!(MprReader.descriptor().id, openreadout_echem::MPR_FORMAT_ID);
//! ```
//!
//! # API stability
//!
//! The supported API is what this page documents: the readers and their format ids.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod arbin;
mod assurance;
mod eclab;
mod gamry;
mod neware;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::series::{SeriesDataset, read_all};
use openreadout_core::source::Input;

/// Format id of BioLogic EC-Lab `.mpr` files.
pub const MPR_FORMAT_ID: &str = eclab::MPR_FORMAT_ID;
/// Format id of BioLogic EC-Lab `.mpt` text exports.
pub const MPT_FORMAT_ID: &str = eclab::MPT_FORMAT_ID;

/// Format id of Gamry Framework `.DTA` files.
pub const GAMRY_FORMAT_ID: &str = gamry::FORMAT_ID;

/// Format id of Neware BTS `.nda` files.
pub const NDA_FORMAT_ID: &str = neware::NDA_FORMAT_ID;
/// Format id of Neware BTS `.ndax` files.
pub const NDAX_FORMAT_ID: &str = neware::NDAX_FORMAT_ID;

/// Format id of Arbin `.res` result files.
pub const ARBIN_FORMAT_ID: &str = arbin::FORMAT_ID;

/// Largest file read (the largest public `.mpr` files are tens of MB).
const MAX_FILE: u64 = 2 << 30;

/// The BioLogic EC-Lab `.mpr` reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct MprReader;

impl FormatReader for MprReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::BIOLOGIC_MPR)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: MPR_FORMAT_ID.into(),
            name: "BioLogic EC-Lab (.mpr)".into(),
            vendor: "BioLogic".into(),
            extensions: vec!["mpr".into()],
            family: "electrochemistry".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::BIOLOGIC_MPR.confidence,
            known_gaps: vec![
                "A file with a column id not validated against an EC-Lab export is refused (exit 6) with the advice to read its .mpt export; about 70 ids are known".into(),
                "Quantities EC-Lab computes only when exporting text (energies, capacities, efficiency, P/W where not stored) are not returned".into(),
                "Technique parameters (the settings module) are not decoded; the technique, channel and acquisition start are".into(),
                "Instrument model and software version are not in a decoded field of the .mpr (the .mpt export names them)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], _path: &Path) -> Option<Detection> {
        head.starts_with(eclab::MPR_MAGIC).then_some(Detection {
            format_id: MPR_FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let bytes = read_all(input, MPR_FORMAT_ID, MAX_FILE)?;
        let file = eclab::parse_mpr(&bytes)?;
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

/// The BioLogic EC-Lab `.mpt` text-export reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct MptReader;

impl FormatReader for MptReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::BIOLOGIC_MPT)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: MPT_FORMAT_ID.into(),
            name: "BioLogic EC-Lab text export (.mpt)".into(),
            vendor: "BioLogic".into(),
            extensions: vec!["mpt".into()],
            family: "electrochemistry".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::BIOLOGIC_MPT.confidence,
            known_gaps: vec![
                "Every column EC-Lab exported is returned, including those it computed at export; columns not in the vocabulary keep a name made from EC-Lab's label".into(),
                "Header facts are read from the `key : value` lines EC-Lab writes in English".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if head.starts_with(eclab::MPT_MAGIC) {
            return Some(Detection {
                format_id: MPT_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        let first = head.split(|&c| c == b'\n').next().unwrap_or(&[]);
        (has_extension(path, &["mpt", "txt"]) && eclab::looks_like_table_header(first)).then_some(
            Detection {
                format_id: MPT_FORMAT_ID,
                confidence: DetectConfidence::Likely,
                note: Some("an EC-Lab table without its header block".into()),
            },
        )
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let bytes = read_all(input, MPT_FORMAT_ID, MAX_FILE)?;
        let file = eclab::parse_mpt(&bytes)?;
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

/// The Gamry Framework `.DTA` reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct GamryReader;

impl FormatReader for GamryReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::GAMRY_DTA)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: GAMRY_FORMAT_ID.into(),
            name: "Gamry Framework (.DTA)".into(),
            vendor: "Gamry Instruments".into(),
            extensions: vec!["dta".into()],
            family: "electrochemistry".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::GAMRY_DTA.confidence,
            known_gaps: vec![
                "Text columns (the `Over` overload flags) are kept out of the traces".into(),
                "Header parameters are kept in the vendor tree; only the date, time, potentiostat, notes, area and scan rate are normalized".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], _path: &Path) -> Option<Detection> {
        gamry::looks_like(head).then_some(Detection {
            format_id: GAMRY_FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let bytes = read_all(input, GAMRY_FORMAT_ID, MAX_FILE)?;
        let file = gamry::parse(&bytes)?;
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

/// The Neware BTS `.nda` reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct NdaReader;

impl FormatReader for NdaReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::NEWARE_NDA)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: NDA_FORMAT_ID.into(),
            name: "Neware BTS (.nda)".into(),
            vendor: "Neware".into(),
            extensions: vec!["nda".into()],
            family: "electrochemistry".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::NEWARE_NDA.confidence,
            known_gaps: vec![
                "Versions 29 (BTS 7) and 130 (BTS 8/9, both record layouts) are read; other versions are refused (exit 6)".into(),
                "Auxiliary records of version-29 and BTS 9.0 files (0x65) are not decoded; BTS 9.1 records carry their temperature".into(),
                "A current range missing from the known scale table is refused".into(),
                "Cycle numbers are as stored (1-based); BTSDA's charge-first cycle renumbering is not applied".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], _path: &Path) -> Option<Detection> {
        head.starts_with(neware::NDA_MAGIC).then_some(Detection {
            format_id: NDA_FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let bytes = read_all(input, NDA_FORMAT_ID, MAX_FILE)?;
        let file = neware::parse_nda(&bytes)?;
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

/// The Neware BTS `.ndax` reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct NdaxReader;

impl FormatReader for NdaxReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::NEWARE_NDAX)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: NDAX_FORMAT_ID.into(),
            name: "Neware BTS (.ndax)".into(),
            vendor: "Neware".into(),
            extensions: vec!["ndax".into()],
            family: "electrochemistry".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::NEWARE_NDAX.confidence,
            known_gaps: vec![
                ".ndc versions 5, 11, 14, 16 and 17 are read; other versions are refused (exit 6)".into(),
                "In versions 11-17 only logged records carry capacities and energies (NaN between them); the step time between logged records follows from the logging interval (reported as assumed)".into(),
                "Auxiliary channels of versions 5, 11 and 16 are not decoded".into(),
                "The step program (Step.xml) is kept out; the executed steps are the steps table".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        (openreadout_core::zip::is_zip(head) && has_extension(path, &["ndax"])).then_some(
            Detection {
                format_id: NDAX_FORMAT_ID,
                confidence: DetectConfidence::Likely,
                note: None,
            },
        )
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        if !openreadout_core::zip::is_zip(head) {
            return None;
        }
        let z =
            openreadout_core::zip::ZipIndex::open(input.fs(), input.path(), NDAX_FORMAT_ID).ok()?;
        (z.has("data.ndc") && (z.has("TestInfo.xml") || z.has("VersionInfo.xml"))).then_some(
            Detection {
                format_id: NDAX_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            },
        )
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let z = openreadout_core::zip::ZipIndex::open(input.fs(), input.path(), NDAX_FORMAT_ID)?;
        let file = neware::parse_ndax(&z)?;
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

/// The Arbin MITS Pro `.res` reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct ArbinReader;

impl FormatReader for ArbinReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::ARBIN_RES)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: ARBIN_FORMAT_ID.into(),
            name: "Arbin MITS Pro result (.res)".into(),
            vendor: "Arbin Instruments".into(),
            extensions: vec!["res".into()],
            family: "electrochemistry".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::ARBIN_RES.confidence,
            known_gaps: vec![
                "Only the Jet 4 / ACE database of MITS Pro 4-8 is read; the MITS 10+ SQL Server data and the .xlsx/.csv exports are not".into(),
                "Smart-battery, CAN-BMS and multi-cell ACI tables are listed (ls) but not returned".into(),
                "The schedule (.sdu) is named, not read; step types are not in the file".into(),
                "`date_time` is the tester's local time; the file stores no time zone".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        (head.starts_with(&openreadout_core::jet::JET_SIGNATURE)
            && head.get(4..19) == Some(b"Standard Jet DB".as_slice())
            && has_extension(path, &["res"]))
        .then_some(Detection {
            format_id: ARBIN_FORMAT_ID,
            confidence: DetectConfidence::Likely,
            note: Some("a Jet database named .res".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let bytes = read_all(input, ARBIN_FORMAT_ID, MAX_FILE)?;
        let file = arbin::parse(&bytes)?;
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
