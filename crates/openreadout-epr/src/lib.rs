//! Clean-room readers for electron paramagnetic resonance (EPR/ESR) data.
//!
//! - **Bruker BES3T** (`bruker-bes3t`, `.DSC` descriptor + `.DTA` data, optionally `.XGF`/`.YGF`/
//!   `.ZGF` axis files; written by Xepr on ELEXSYS, EMX and EMXplus spectrometers): cw and pulse
//!   spectra, 1D, 2D and 3D, real or complex, with the field or time axis, the sweep axis and the
//!   standard parameters (microwave frequency and power, modulation, gain, time constant, scans,
//!   temperature).
//! - **Bruker ESP / WinEPR** (`bruker-esp`, `.par` parameters + `.spc` data): WinEPR and ESP
//!   300/380 spectra (1D and 2D) with the field axis and acquisition parameters.
//!
//! Every data set is one trace per data value of a point (almost always one): one sweep per
//! slice of the second and third dimensions, channel `intensity` (real data) or `real` and
//! `imaginary` (complex data), the abscissa in `extra.axis` (`magnetic_field` in G or mT, `time`
//! in ns, …; an axis file is channel 0). Notes: `docs/formats/bruker-epr.md`.
//!
//! # Example
//!
//! ```
//! use openreadout_core::FormatReader;
//! use openreadout_epr::Bes3tReader;
//!
//! assert_eq!(Bes3tReader.descriptor().id, openreadout_epr::BES3T_FORMAT_ID);
//! ```
//!
//! # API stability
//!
//! The supported API is what this page documents: the readers and their format ids.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod assurance;
mod bes3t;
mod common;
mod esp;
mod params;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader};
use openreadout_core::series::SeriesDataset;
use openreadout_core::source::Input;

/// Format id of Bruker BES3T (`.DSC`/`.DTA`) data sets.
pub const BES3T_FORMAT_ID: &str = bes3t::FORMAT_ID;
/// Format id of Bruker ESP/WinEPR (`.par`/`.spc`) data sets.
pub const ESP_FORMAT_ID: &str = esp::FORMAT_ID;

/// The Bruker BES3T reader (`.DSC` + `.DTA`).
#[derive(Debug, Default, Clone, Copy)]
pub struct Bes3tReader;

impl FormatReader for Bes3tReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::BRUKER_BES3T)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: BES3T_FORMAT_ID.into(),
            name: "Bruker BES3T EPR (.DSC/.DTA)".into(),
            vendor: "Bruker".into(),
            extensions: vec!["dsc".into(), "dta".into()],
            family: "epr".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::BRUKER_BES3T.confidence,
            known_gaps: vec![
                "ASCII data sets (IRFMT A), N-tuple axes (NTUP) and points whose values have different number formats are refused (exit 6)".into(),
                "Values are returned as stored: no normalization by scans, gain, power or conversion time (Xepr's and EasySpin's optional scalings)".into(),
                "The manipulation history layer (#MHL) is counted, not interpreted; processed data sets are read like acquired ones".into(),
                "Only the standard parameters are normalized; device parameters (#DSL) are in the vendor tree as text".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        if ext == "dsc" && params::is_descriptor(head) {
            return Some(Detection {
                format_id: BES3T_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        None
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        if let Some(d) = self.sniff(head, input.path()) {
            return Some(d);
        }
        let ext = input.path().extension()?.to_str()?.to_ascii_lowercase();
        (ext == "dta" && bes3t::has_descriptor(input)).then_some(Detection {
            format_id: BES3T_FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let p = bes3t::open(input)?;
        Ok(Box::new(SeriesDataset::new(
            self.descriptor(),
            input.path().to_path_buf(),
            p.size,
            p.file,
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// The Bruker ESP/WinEPR reader (`.par` + `.spc`).
#[derive(Debug, Default, Clone, Copy)]
pub struct EspReader;

impl FormatReader for EspReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::BRUKER_ESP)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: ESP_FORMAT_ID.into(),
            name: "Bruker ESP/WinEPR EPR (.par/.spc)".into(),
            vendor: "Bruker".into(),
            extensions: vec!["par".into(), "spc".into()],
            family: "epr".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::BRUKER_ESP.confidence,
            known_gaps: vec![
                "The .spc file has no header: its layout (little-endian float32 for WinEPR, big-endian int32 for ESP) and point count come from the .par file; a data set without its .par file is not recognised".into(),
                "Values are returned as stored: no normalization by scans, gain or conversion time".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        // a .spc file has no header: only a .par whose lines are ESP/WinEPR parameters is
        // recognised from its bytes (the sibling decides for the other inputs)
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("par"))
        {
            return None;
        }
        let text = String::from_utf8_lossy(&head[..head.len().min(4096)]);
        let keys = text
            .split(['\r', '\n'])
            .filter_map(|l| l.split_whitespace().next())
            .filter(|k| {
                matches!(
                    *k,
                    "JSS"
                        | "JDA"
                        | "JTM"
                        | "JON"
                        | "GST"
                        | "GSI"
                        | "HCF"
                        | "HSW"
                        | "RES"
                        | "MF"
                        | "ANZ"
                        | "RCT"
                        | "RTC"
                        | "RRG"
                        | "MPS"
                        | "MP"
                        | "TE"
                )
            })
            .count();
        (keys >= 3).then_some(Detection {
            format_id: ESP_FORMAT_ID,
            confidence: DetectConfidence::Likely,
            note: None,
        })
    }

    fn sniff_input(&self, _head: &[u8], input: &Input) -> Option<Detection> {
        esp::sibling_par(input).map(|_| Detection {
            format_id: ESP_FORMAT_ID,
            confidence: DetectConfidence::Likely,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let p = esp::open(input)?;
        Ok(Box::new(SeriesDataset::new(
            self.descriptor(),
            input.path().to_path_buf(),
            p.size,
            p.file,
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
