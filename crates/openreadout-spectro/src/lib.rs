//! Clean-room readers for optical and vibrational spectroscopy files.
//!
//! - **Bruker OPUS** (`bruker-opus`, `.0`, `.1`, … numbered files): FT-IR/NIR/Raman spectra
//!   (absorbance, transmittance, reflectance, single-channel sample and reference spectra,
//!   interferograms, phases, 3D series) with the instrument, optics, acquisition and Fourier
//!   transform parameters. Notes: `docs/formats/bruker-opus.md`.
//! - **Thermo Fisher OMNIC** (`thermo-omnic`, `.spa` single spectra and `.spg` groups): spectra
//!   with their acquisition header, title, timestamp and history. Notes:
//!   `docs/formats/thermo-omnic.md`.
//! - **Renishaw WiRE** (`renishaw-wdf`, `.wdf`): Raman spectra, line scans, depth and time
//!   series and maps with their stage coordinates, laser wavelength, the property sets
//!   (grating, exposure, objective, laser power) and the white-light image. Notes:
//!   `docs/formats/renishaw-wdf.md`.
//! - **PerkinElmer** (`perkinelmer-sp`, `.sp`): IR and UV-Vis spectra with instrument text.
//!   Notes: `docs/formats/perkinelmer-sp.md`.
//! - **JASCO Spectra Manager** (`jasco-jws`, `.jws`, `.jrs`): FT-IR, Raman, UV-Vis, circular
//!   dichroism (CD, HT voltage, absorbance channels) and fluorescence spectra, from both the
//!   compound-file and the flat container, with the instrument, sample and FT-IR acquisition
//!   parameters. Notes: `docs/formats/jasco-jws.md`.
//! - **Galactic / Thermo GRAMS SPC** (`galactic-spc`, `.spc`): single spectra and multifiles
//!   (spectrum series and maps exported by GRAMS, OMNIC, WiRE, Spectragryph and others), float
//!   or fixed-point values, with the log block's instrument settings. Notes:
//!   `docs/formats/galactic-spc.md`.
//! - **Agilent (Varian) Cary UV-Vis** (`agilent-cary`, `.dsw` Scan, `.bsw` batch Scan, `.bsk`
//!   Scanning Kinetics): absorbance spectra and baselines with their wavelengths, the
//!   instrument, application, parameter list, method log and collection times. Notes:
//!   `docs/formats/agilent-cary.md`.
//! - **PerkinElmer Spotlight images** (`perkinelmer-fsm`, `.fsm`): FT-IR images as a spectrum
//!   trace (one sweep per pixel) with stage positions and a map, with the instrument record.
//!   Notes: `docs/formats/perkinelmer-fsm.md`.
//! - **Agilent FT-IR imaging** (`agilent-fpa`, `.dat`/`.seq` with a `.bsp` header, mosaics
//!   `.dms`/`.dmd`/`.drd` with a `.dmt` header): focal-plane-array hyperspectral images from
//!   Cary 600 series microscopes (Resolutions Pro), as a spectrum trace (one sweep per pixel)
//!   and a map with one channel per wavenumber, or interferograms on their optical-path-difference
//!   axis, with the acquisition settings. Notes: `docs/formats/agilent-fpa.md`.
//! - **WITec Project / WITec Data** (`witec-project`, `.wip`, `.wid`): confocal Raman
//!   microscopy projects — single spectra, line scans, depth profiles and Raman maps with their
//!   grating calibration (Raman shift, wavelength or energy axis), the images derived from them
//!   (band sums, masks, peak positions), video images and each measurement's information text
//!   (excitation, grating, integration time, objective). Notes: `docs/formats/witec-project.md`.
//!
//! Every spectrum set is a core trace: one sweep per spectrum, the y values as channel 0, and
//! the x axis in `extra.axis` (`{quantity, unit, first, last, step, size}`: `wavenumber` in
//! `1/cm`, `raman_shift` in `1/cm`, `wavelength` in `nm` or `µm`, `points`). An axis that is not
//! evenly spaced (a grating spectrometer's calibration) is `{irregular: true, channel: 0}` and
//! the trace then has two channels: the x values and the y values. Spectra on a map grid are
//! also one image with one channel per spectral point, so `preview --channel N` draws the map
//! at band N.
//!
//! # Example
//!
//! ```
//! use openreadout_core::FormatReader;
//! use openreadout_spectro::OpusReader;
//!
//! let format = OpusReader.descriptor();
//! assert_eq!(format.id, openreadout_spectro::OPUS_FORMAT_ID);
//! println!("{} ({})", format.name, format.family);
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::FormatReader;
//! use openreadout_spectro::OpusReader;
//!
//! let mut dataset = OpusReader.open(Path::new("sample.0"))?;
//! let info = dataset.info()?; // headers only
//! let t = &info.traces[0];
//! println!("{}: {} points, axis {}", t.name.as_deref().unwrap_or(""), t.sample_count, t.extra["axis"]);
//! let spectrum = dataset.read_trace(0, 0, 0, u64::MAX)?;
//! println!("first value {:?}", spectrum.channels[0].first());
//! # Ok::<(), openreadout_core::Error>(())
//! ```
//!
//! # API stability
//!
//! The supported API is what this page documents: the readers, their format ids and
//! [`SpectroDataset`] (returned boxed by `open`).
#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![allow(clippy::many_single_char_names)] // byte-layout code: b (bytes), n, w, x, y as in the notes

mod agilent;
mod assurance;
mod cary;
mod common;
mod fsm;
mod jasco;
mod omnic;
mod omnic_srs;
mod opus;
mod pesp;
mod pesp_ascii;
mod spc;
mod wdf;
mod witec;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;

pub use common::SpectroDataset;

/// Format id of Bruker OPUS files.
pub const OPUS_FORMAT_ID: &str = "bruker-opus";
/// Format id of Thermo Fisher OMNIC `.spa`/`.spg` files.
pub const OMNIC_FORMAT_ID: &str = "thermo-omnic";
/// Format id of Renishaw WiRE `.wdf` files.
pub const WDF_FORMAT_ID: &str = "renishaw-wdf";
/// Format id of PerkinElmer `.sp` files.
pub const PESP_FORMAT_ID: &str = "perkinelmer-sp";
/// Format id of JASCO Spectra Manager `.jws`/`.jrs` files.
pub const JWS_FORMAT_ID: &str = "jasco-jws";
/// Format id of Galactic / Thermo GRAMS `.spc` files.
pub const SPC_FORMAT_ID: &str = "galactic-spc";
/// Format id of Agilent (Varian) Cary UV-Vis `.dsw`/`.bsw`/`.bsk` files.
pub const CARY_FORMAT_ID: &str = "agilent-cary";
/// Format id of PerkinElmer Spotlight `.fsm` images.
pub const FSM_FORMAT_ID: &str = "perkinelmer-fsm";
/// Format id of Agilent FT-IR imaging (focal-plane-array) files.
pub const AGILENT_FPA_FORMAT_ID: &str = "agilent-fpa";
/// Format id of WITec Project `.wip` and WITec Data `.wid` files.
pub const WITEC_FORMAT_ID: &str = "witec-project";

/// The Bruker OPUS reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpusReader;

impl FormatReader for OpusReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::BRUKER_OPUS)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: OPUS_FORMAT_ID.into(),
            name: "Bruker OPUS".into(),
            vendor: "Bruker".into(),
            extensions: vec!["0".into(), "1".into(), "2".into()],
            family: "spectroscopy".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::BRUKER_OPUS.confidence,
            known_gaps: vec![
                "Any numbered extension (.0, .1, … .999) is an OPUS file when it starts with 0A 0A FE FE; the extensions listed are the common ones".into(),
                "3D series blocks (rapid scan, time-resolved) follow brukeropus's layout; no public series file was available to validate them".into(),
                "Report blocks (quant and search results), embedded bitmaps and blocks of unknown type are listed by `info --view structure`, not decoded".into(),
                "Data blocks without a data-status block are listed, not decoded".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if head.starts_with(&opus::OPUS_MAGIC) {
            return Some(Detection {
                format_id: OPUS_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        let numbered = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            !e.is_empty() && e.len() <= 4 && e.bytes().all(|c| c.is_ascii_digit())
        });
        (numbered && !head.is_empty() && head.len() < 4).then_some(Detection {
            format_id: OPUS_FORMAT_ID,
            confidence: DetectConfidence::ExtensionOnly,
            note: Some("numbered extension but the file is too short to be OPUS".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let (f, len) = common::open_source(input)?;
        let parsed = opus::parse(&f, input.path(), len)?;
        Ok(Box::new(SpectroDataset::new(
            self.descriptor(),
            input,
            f,
            len,
            parsed,
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// The Thermo Fisher OMNIC reader (`.spa`, `.spg`).
#[derive(Debug, Default, Clone, Copy)]
pub struct OmnicReader;

impl FormatReader for OmnicReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::THERMO_OMNIC)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: OMNIC_FORMAT_ID.into(),
            name: "Thermo Fisher OMNIC".into(),
            vendor: "Thermo Fisher Scientific (Nicolet)".into(),
            extensions: vec!["spa".into(), "spg".into(), "srs".into()],
            family: "spectroscopy".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::THERMO_OMNIC.confidence,
            known_gaps: vec![
                "Series files (.srs: rapid scan, high-speed, GC-IR, TGA-IR) are read from their key table: the series spectra, their times and the backgrounds; the per-series profiles (Gram-Schmidt, chemigrams) and processing records are listed, not decoded; no licensed .srs file was available, so the series layout rests on held test files".into(),
                "OMNIC map files (.map) are not recognised".into(),
                "Resolution and the instrument serial number come from the processing-history text (English or French OMNIC); files without that text have neither".into(),
                "y-unit codes other than absorbance, transmittance, reflectance, log(1/R), single beam, Kubelka-Munk, volts, photoacoustic and Raman intensity are reported as `intensity`".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if head.starts_with(omnic::OMNIC_MAGIC) || head.starts_with(omnic::OMNIC_SERIES_MAGIC) {
            return Some(Detection {
                format_id: OMNIC_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        has_extension(path, &["spa", "spg", "srs"]).then_some(Detection {
            format_id: OMNIC_FORMAT_ID,
            confidence: DetectConfidence::ExtensionOnly,
            note: Some(
                "OMNIC extension but the file does not start with `Spectral Data File`".into(),
            ),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let (f, len) = common::open_source(input)?;
        let parsed = omnic::parse(&f, input.path(), len)?;
        Ok(Box::new(SpectroDataset::new(
            self.descriptor(),
            input,
            f,
            len,
            parsed,
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// The Renishaw WiRE reader (`.wdf`).
#[derive(Debug, Default, Clone, Copy)]
pub struct WdfReader;

impl FormatReader for WdfReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::RENISHAW_WDF)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: WDF_FORMAT_ID.into(),
            name: "Renishaw WiRE".into(),
            vendor: "Renishaw".into(),
            extensions: vec!["wdf".into()],
            family: "spectroscopy".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::RENISHAW_WDF.confidence,
            known_gaps: vec![
                "x-list and y-unit codes are named only where the corpus confirms them (Raman shift, counts); other codes are reported as numbers with an `x` axis".into(),
                "Property sets are decoded to a tree; keys WiRE does not name in the file appear as `#<number>` and only those whose meaning the corpus shows are normalized (objective, laser name, serial numbers)".into(),
                "Map pixels are placed from the stage coordinates on the WMAP grid; maps without coordinates fall back to storage order".into(),
                "Line scans and depth or time series are traces with a positions table, not images".into(),
                "Pre-WiRE-3 files, processed data blocks (baseline, cosmic-ray removal results) and embedded Z-stacks are not decoded".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if head.starts_with(wdf::WDF_MAGIC) {
            return Some(Detection {
                format_id: WDF_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        has_extension(path, &["wdf"]).then_some(Detection {
            format_id: WDF_FORMAT_ID,
            confidence: DetectConfidence::ExtensionOnly,
            note: Some("WiRE extension but the file does not start with WDF1".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let (f, len) = common::open_source(input)?;
        let parsed = wdf::parse(&f, input.path(), len)?;
        Ok(Box::new(SpectroDataset::new(
            self.descriptor(),
            input,
            f,
            len,
            parsed,
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// The PerkinElmer `.sp` reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct PeSpReader;

impl FormatReader for PeSpReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::PERKINELMER_SP)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: PESP_FORMAT_ID.into(),
            name: "PerkinElmer Spectrum (.sp)".into(),
            vendor: "PerkinElmer".into(),
            extensions: vec!["sp".into()],
            family: "spectroscopy".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::PERKINELMER_SP.confidence,
            known_gaps: vec![
                "One spectrum per file (2D constant-interval data sets); Spotlight image files (.fsm) are read by the perkinelmer-fsm reader".into(),
                "Instrument settings are named from their values in the corpus (scans, resolution, detector, source, beamsplitter, apodization, laser wavenumber); other settings are in the vendor tree by member id".into(),
                "UV-Vis (.sp from Lambda instruments, x in nm) follows the same layout by inference; no public UV-Vis .sp file was available".into(),
                "Text .sp files (`PE … ASCII PEDS`, e.g. from an LS55): only the x and y pairs and the technique code are read; the other header lines are kept unnamed in the vendor tree".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if head.starts_with(pesp::PESP_MAGIC) && !fsm::is_image_description(head) {
            return Some(Detection {
                format_id: PESP_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        if pesp_ascii::is_ascii_sp(head) {
            return Some(Detection {
                format_id: PESP_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: Some("PerkinElmer .sp saved as text (PEDS)".into()),
            });
        }
        has_extension(path, &["sp"]).then_some(Detection {
            format_id: PESP_FORMAT_ID,
            confidence: DetectConfidence::ExtensionOnly,
            note: Some("PerkinElmer extension but the file does not start with PEPE".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let (f, len) = common::open_source(input)?;
        let head = common::read_at(&f, input.path(), 0, len.min(256), len)?;
        if !head.starts_with(pesp::PESP_MAGIC) && pesp_ascii::is_ascii_sp(&head) {
            let opened = pesp_ascii::parse(&f, input.path(), len)?;
            let n = opened.values.len() as u64;
            let src = openreadout_core::source::MemSource::new("#DATA", opened.values);
            return Ok(Box::new(
                SpectroDataset::new(self.descriptor(), input, f, len, opened.parsed)
                    .with_values_source(
                        openreadout_core::source::SourceFile::new(std::sync::Arc::new(src)),
                        n,
                    ),
            ));
        }
        let parsed = pesp::parse(&f, input.path(), len)?;
        Ok(Box::new(SpectroDataset::new(
            self.descriptor(),
            input,
            f,
            len,
            parsed,
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// The JASCO Spectra Manager reader (`.jws`, `.jrs`).
#[derive(Debug, Default, Clone, Copy)]
pub struct JwsReader;

impl FormatReader for JwsReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::JASCO_JWS)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: JWS_FORMAT_ID.into(),
            name: "JASCO Spectra Manager (.jws)".into(),
            vendor: "JASCO".into(),
            extensions: vec!["jws".into(), "jrs".into()],
            family: "spectroscopy".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::JASCO_JWS.confidence,
            known_gaps: vec![
                "Validated on FT/IR-4600/4700, NRS-5100 Raman, V-630/V-730 UV-Vis, J-1500 circular dichroism (1-3 channels) and FP-8300 fluorescence files; interval/kinetics files (`.jwb`) and channel or axis codes not seen are refused or returned unnamed with a finding".into(),
                "Time-course files carry their x axis as time without a unit (the unit is not in a field that has been validated)".into(),
                "Acquisition parameters are named for FT-IR (module 9) only; the other instruments' parameter records are kept raw in the vendor tree".into(),
                "Text fields of flat files are decoded as UTF-8 or Latin-1; Japanese (Shift-JIS) titles may be garbled".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if head.starts_with(jasco::FLAT_MAGIC) {
            return Some(Detection {
                format_id: JWS_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        (has_extension(path, &["jws", "jrs"])
            && head.starts_with(&openreadout_core::cfb::CFB_MAGIC))
        .then_some(Detection {
            format_id: JWS_FORMAT_ID,
            confidence: DetectConfidence::Likely,
            note: None,
        })
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        if head.starts_with(&openreadout_core::cfb::CFB_MAGIC) && jasco::is_compound_jws(input) {
            return Some(Detection {
                format_id: JWS_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        self.sniff(head, input.path())
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let (f, len) = common::open_source(input)?;
        let opened = jasco::parse(&f, input.path(), len)?;
        let ds = SpectroDataset::new(self.descriptor(), input, f, len, opened.parsed);
        Ok(Box::new(match opened.values {
            Some(v) => {
                let n = v.len() as u64;
                let src = openreadout_core::source::MemSource::new("Y-Data", v);
                ds.with_values_source(
                    openreadout_core::source::SourceFile::new(std::sync::Arc::new(src)),
                    n,
                )
            }
            None => ds,
        }))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// The Galactic / Thermo GRAMS `.spc` reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct SpcReader;

impl FormatReader for SpcReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::GALACTIC_SPC)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: SPC_FORMAT_ID.into(),
            name: "Galactic / Thermo GRAMS SPC (.spc)".into(),
            vendor: "Thermo Fisher Scientific (Galactic Industries) GRAMS".into(),
            extensions: vec!["spc".into()],
            family: "spectroscopy".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::GALACTIC_SPC.confidence,
            known_gaps: vec![
                "Big-endian (0x4C) files, per-subfile x arrays (TXYXYS, mass spectra) and old-format (0x4D) multifiles are refused".into(),
                "16-bit fixed-point values follow the documented rule but no development file has them".into(),
                "Multifile maps are returned as spectrum series with a subfile table (z, W); the map grid is not rebuilt".into(),
                "Other programs' .spc files (Bruker EPR, Becker & Hickl, EDAX, Shimadzu UV) are not Galactic SPC and are not read by this reader".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        // The SPC header has no signature (only a version byte), so only `.spc` files are
        // claimed: other files whose bytes happen to fit (zips, plate-reader exports) are not.
        if !has_extension(path, &["spc"]) {
            return None;
        }
        if spc::looks_like_spc(head) {
            return Some(Detection {
                format_id: SPC_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        (has_extension(path, &["spc"]) && head.get(1) == Some(&0x4C)).then_some(Detection {
            format_id: SPC_FORMAT_ID,
            confidence: DetectConfidence::Likely,
            note: Some("big-endian Galactic SPC (refused on open)".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let (f, len) = common::open_source(input)?;
        let parsed = spc::parse(&f, input.path(), len)?;
        Ok(Box::new(SpectroDataset::new(
            self.descriptor(),
            input,
            f,
            len,
            parsed,
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// The WITec Project / WITec Data reader (`.wip`, `.wid`).
#[derive(Debug, Default, Clone, Copy)]
pub struct WitecReader;

impl FormatReader for WitecReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::WITEC_PROJECT)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: WITEC_FORMAT_ID.into(),
            name: "WITec Project / WITec Data".into(),
            vendor: "WITec (Oxford Instruments)".into(),
            extensions: vec!["wip".into(), "wid".into()],
            family: "spectroscopy".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::WITEC_PROJECT.confidence,
            known_gaps: vec![
                "Format versions 0-7 (WITec Control 1.6 to WITec Suite SIX); spectral calibrations of the grating and polynomial kinds are validated, linear and lookup-table x calibrations follow the published format description without a development file".into(),
                "Filter, cursor, colour-profile and mask-definition records (analysis settings) are listed, not decoded; the images they produced are read".into(),
                "Acquisition settings come from each measurement's information text (English WITec software); the version-7 Suite SIX parameter records are kept in the vendor tree only".into(),
                "The acquisition time is local time (the file holds no time zone)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if witec::looks_like_witec(head) {
            return Some(Detection {
                format_id: WITEC_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        has_extension(path, &["wip", "wid"]).then_some(Detection {
            format_id: WITEC_FORMAT_ID,
            confidence: DetectConfidence::ExtensionOnly,
            note: Some("WITec extension but the file does not start with a WIT_ magic".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let (f, len) = common::open_source(input)?;
        let parsed = witec::parse(&f, input.path(), len)?;
        Ok(Box::new(SpectroDataset::new(
            self.descriptor(),
            input,
            f,
            len,
            parsed,
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// The Agilent FT-IR imaging reader (`.dat`, `.seq`, `.dms`, `.dmd`, `.drd` data files; `.bsp`,
/// `.dmt` headers).
#[derive(Debug, Default, Clone, Copy)]
pub struct AgilentFpaReader;

impl FormatReader for AgilentFpaReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::AGILENT_FPA)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: AGILENT_FPA_FORMAT_ID.into(),
            name: "Agilent FT-IR imaging (Resolutions Pro)".into(),
            vendor: "Agilent".into(),
            extensions: vec![
                "dat".into(),
                "seq".into(),
                "dms".into(),
                "dmd".into(),
                "drd".into(),
                "bsp".into(),
                "dmt".into(),
            ],
            family: "spectroscopy".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::AGILENT_FPA.confidence,
            known_gaps: vec![
                "A mosaic is read from its whole-image .dms file (or one tile at a time); tiles are not assembled when the .dms is missing".into(),
                "Rows run top to bottom: the files store them bottom to top (inferred from how a mosaic's tiles are stacked in its .dms)".into(),
                "The pixel size is the detector pixel size times the aggregation, read from the header; stage positions are not in the files".into(),
                "Settings are read from the header's text settings (English Resolutions Pro); the selected-pixel spectrum stored in the header is not returned".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if has_extension(path, &agilent::DATA_EXTENSIONS) && agilent::looks_like_data(head) {
            return Some(Detection {
                format_id: AGILENT_FPA_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        None
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        if has_extension(input.path(), &agilent::HEADER_EXTENSIONS)
            && agilent::is_header(head, input)
        {
            return Some(Detection {
                format_id: AGILENT_FPA_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        self.sniff(head, input.path())
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let data = agilent::resolve(input)?;
        let (f, len) = common::open_source(&data)?;
        let parsed = agilent::parse(&f, &data, len)?;
        Ok(Box::new(SpectroDataset::new(
            self.descriptor(),
            &data,
            f,
            len,
            parsed,
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// The PerkinElmer Spotlight image reader (`.fsm`).
#[derive(Debug, Default, Clone, Copy)]
pub struct FsmReader;

impl FormatReader for FsmReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::PERKINELMER_FSM)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FSM_FORMAT_ID.into(),
            name: "PerkinElmer Spotlight image (.fsm)".into(),
            vendor: "PerkinElmer".into(),
            extensions: vec!["fsm".into()],
            family: "spectroscopy".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::PERKINELMER_FSM.confidence,
            known_gaps: vec![
                "Image rows are in acquisition order (stage y increasing by one step per row); which way up Spectrum IMAGE draws them is not in the file".into(),
                "Only the 4-D constant-interval layout (one float32 spectrum per pixel block) has been seen".into(),
                "Instrument settings are named as in the .sp reader (the same member ids); other settings are in the vendor tree".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if head.starts_with(pesp::PESP_MAGIC) && fsm::is_image_description(head) {
            return Some(Detection {
                format_id: FSM_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        has_extension(path, &["fsm"]).then_some(Detection {
            format_id: FSM_FORMAT_ID,
            confidence: DetectConfidence::ExtensionOnly,
            note: Some("Spotlight extension but the file does not start with PEPE and a 4D data-set description".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let (f, len) = common::open_source(input)?;
        let parsed = fsm::parse(&f, input.path(), len)?;
        Ok(Box::new(SpectroDataset::new(
            self.descriptor(),
            input,
            f,
            len,
            parsed,
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// The Agilent (Varian) Cary UV-Vis reader (`.dsw`, `.bsw`, `.bsk`).
#[derive(Debug, Default, Clone, Copy)]
pub struct CaryReader;

impl FormatReader for CaryReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::AGILENT_CARY)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: CARY_FORMAT_ID.into(),
            name: "Agilent Cary UV-Vis (.dsw, .bsw, .bsk)".into(),
            vendor: "Agilent (Varian)".into(),
            extensions: vec!["dsw".into(), "bsw".into(), "bsk".into(), "csw".into()],
            family: "spectroscopy".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::AGILENT_CARY.confidence,
            known_gaps: vec![
                "Validated against the Cary software's CSV exports on Cary 4000 Scan 3.00 files (absorbance over wavelength); Cary 60 files of Scan and Scanning Kinetics 5.0 are read the same way but have no export to check them against".into(),
                "Only the spectrum and baseline stores are decoded; graph, report, database and baseline-info stores are listed by `info --view structure`".into(),
                "Y modes other than absorbance and X modes other than nanometers have no corpus file: their spectra are returned with a finding".into(),
                "Collection times are a local clock (no time zone is stored), read month/day/year as every corpus file writes them".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if head.starts_with(cary::MAGIC) {
            return Some(Detection {
                format_id: CARY_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        (has_extension(path, &["dsw", "bsw", "bsk", "csw"]) && head.len() < cary::MAGIC.len())
            .then_some(Detection {
                format_id: CARY_FORMAT_ID,
                confidence: DetectConfidence::ExtensionOnly,
                note: Some("Cary extension but the file is too short to hold its header".into()),
            })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let (f, len) = common::open_source(input)?;
        let opened = cary::parse(&f, input.path(), len)?;
        let n = opened.values.len() as u64;
        let src = openreadout_core::source::MemSource::new("spectra", opened.values);
        Ok(Box::new(
            SpectroDataset::new(self.descriptor(), input, f, len, opened.parsed)
                .with_values_source(
                    openreadout_core::source::SourceFile::new(std::sync::Arc::new(src)),
                    n,
                ),
        ))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
