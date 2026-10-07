//! Clean-room readers for biophysical bench instruments.
//!
//! - **MicroCal isothermal titration calorimetry** (`microcal-itc`, `.itc`: VP-ITC, iTC200 and
//!   the MicroCal ITC software): the thermogram (differential power and cell temperature against
//!   time) as a trace, the injections (volume, duration, spacing, filter period, start) as a
//!   table, and the method (cell and syringe concentrations, cell volume, temperature, stirring,
//!   reference power) in the experiment model. Notes: `docs/formats/microcal-itc.md`.
//! - **Cytiva Biacore surface plasmon resonance** (`cytiva-biacore-blr`, `.blr` result files of
//!   the Biacore T200 Control Software): every stored sensorgram (raw flow cells and
//!   reference-subtracted curves, per cycle) as a trace, the control software's report points,
//!   the cycles and the event log as tables, chip and run in the vendor tree and experiment
//!   model. Notes: `docs/formats/cytiva-biacore.md`.
//! - **Agilent Seahorse XF** (`agilent-seahorse-asyr`, `.asyr` assay results of Wave): the O2
//!   and pH sensors' corrected emission per well at every plate reading as traces, the plate map,
//!   measurements, injections and executed protocol as tables, and the oxygen consumption,
//!   extracellular acidification and proton efflux rates computed with the published compartment
//!   model where the plate and settings are those validated against Wave's own rates. Notes:
//!   `docs/formats/agilent-seahorse.md`.
//! - **GenePix Results** (`genepix-gpr`, `.gpr` of GenePix Pro): the microarray feature table
//!   (every column GenePix wrote) and the scan settings. Notes: `docs/formats/genepix-gpr.md`.
//! - **Sartorius Octet** (`sartorius-octet-frd`, `.frd` biolayer-interferometry results): one
//!   biosensor's sensorgram through every assay step as a trace, the steps as a table, the
//!   instrument and run in the experiment model. Notes: `docs/formats/sartorius-octet.md`.
//! - **Malvern Zetasizer** (`malvern-zetasizer-dts`, `.dts` measurement files): one row per
//!   record with the sample, date and temperature and the stored zeta results (zeta potential,
//!   mobility, conductivity, peaks); size results are withheld until validated.
//!   Notes: `docs/formats/malvern-zetasizer.md`.
//!
//! # Example
//!
//! ```
//! use openreadout_core::FormatReader;
//! use openreadout_biophys::ItcReader;
//!
//! assert_eq!(ItcReader.descriptor().id, openreadout_biophys::ITC_FORMAT_ID);
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::FormatReader;
//! use openreadout_biophys::ItcReader;
//!
//! let mut dataset = ItcReader.open(Path::new("run.itc"))?;
//! let thermogram = dataset.read_trace(0, 0, 0, u64::MAX)?; // µcal/s, °C
//! let injections = dataset.read_table(0, 0, u64::MAX)?;
//! println!("{} samples, {} injections", thermogram.channels[0].len(), injections.columns[0].len());
//! # Ok::<(), openreadout_core::Error>(())
//! ```
//!
//! # API stability
//!
//! The supported API is what this page documents: the readers, their format ids and the
//! datasets they return.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod assurance;
mod biacore;
mod biacore_eval;
mod gpr;
mod itc;
mod octet;
mod seahorse;
mod seahorse_rates;
mod zetasizer;

use std::path::Path;

use openreadout_core::bytes::latin1;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::{ByteSource, Input};
use openreadout_core::{Error, Result};

pub use biacore::BiacoreDataset;
pub use gpr::GprDataset;
pub use itc::ItcDataset;
pub use seahorse::SeahorseDataset;
#[doc(hidden)]
pub use seahorse_rates::{
    OxygenModel, acidification, oxygen_consumption, oxygen_levels, ph_level, slope,
};

/// Format id of MicroCal `.itc` files.
pub const ITC_FORMAT_ID: &str = itc::FORMAT_ID;

/// Format id of GenePix Results `.gpr` files.
pub const GPR_FORMAT_ID: &str = gpr::FORMAT_ID;

/// The GenePix Results reader (`.gpr`).
#[derive(Debug, Default, Clone, Copy)]
pub struct GprReader;

impl FormatReader for GprReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::GENEPIX_GPR)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: GPR_FORMAT_ID.into(),
            name: "GenePix Results (.gpr)".into(),
            vendor: "Molecular Devices (Axon GenePix)".into(),
            extensions: vec!["gpr".into()],
            family: "microarray".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::GENEPIX_GPR.confidence,
            known_gaps: vec![
                "GenePix array lists (.gal), settings (.gps) and the scan TIFF images are not read by this reader (the TIFF reader opens the images)".into(),
                "Values are the text GenePix wrote; no normalization, background correction or flag filtering is applied".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if gpr::atf_type(head).is_some_and(|t| t.starts_with("GenePix Results")) {
            return Some(Detection {
                format_id: GPR_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        None.or_else(|| {
            (has_extension(path, &["gpr"]) && head.starts_with(b"ATF")).then_some(Detection {
                format_id: GPR_FORMAT_ID,
                confidence: DetectConfidence::Likely,
                note: None,
            })
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let (bytes, len) = read_all(input, GPR_FORMAT_ID, gpr::MAX_BYTES)?;
        let text = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => latin1(&e.into_bytes()),
        };
        let g = gpr::parse(&text)?;
        Ok(Box::new(GprDataset::new(
            self.descriptor(),
            input.path().to_path_buf(),
            len,
            g,
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// Format id of Sartorius (ForteBio) Octet `.frd` biolayer-interferometry result files.
pub const OCTET_FORMAT_ID: &str = octet::FORMAT_ID;

/// The Sartorius Octet `.frd` reader (one biosensor's sensorgram and assay steps).
#[derive(Debug, Default, Clone, Copy)]
pub struct OctetReader;

impl FormatReader for OctetReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::SARTORIUS_OCTET_FRD)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: OCTET_FORMAT_ID.into(),
            name: "Sartorius Octet BLI result (.frd)".into(),
            vendor: "Sartorius (ForteBio)".into(),
            extensions: vec!["frd".into()],
            family: "binding-kinetics".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::SARTORIUS_OCTET_FRD.confidence,
            known_gaps: vec![
                "One file is one biosensor; the experiment's other sensors are separate .frd files (read them together with batch)".into(),
                "Kinetic fits (kon, koff, KD) are made by the Octet analysis software and are not in .frd files".into(),
                "Only kinetics experiments (KineticsData) are read".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        octet::looks_like(head).then_some(Detection {
            format_id: OCTET_FORMAT_ID,
            confidence: if has_extension(path, &["frd"]) {
                DetectConfidence::Definite
            } else {
                DetectConfidence::Likely
            },
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let bytes = openreadout_core::series::read_all(input, OCTET_FORMAT_ID, octet::MAX_BYTES)?;
        let xml = String::from_utf8(bytes).map_err(|_| {
            Error::corrupt(OCTET_FORMAT_ID, "the result document is not UTF-8 text")
        })?;
        let file = octet::parse(&xml)?;
        Ok(Box::new(openreadout_core::series::SeriesDataset::new(
            self.descriptor(),
            input.path().to_path_buf(),
            xml.len() as u64,
            file,
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// Format id of Agilent Seahorse XF `.asyr` assay result files.
pub const SEAHORSE_FORMAT_ID: &str = seahorse::FORMAT_ID;

/// The Agilent Seahorse XF assay-result reader (`.asyr`).
#[derive(Debug, Default, Clone, Copy)]
pub struct SeahorseReader;

impl FormatReader for SeahorseReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::AGILENT_SEAHORSE_ASYR)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: SEAHORSE_FORMAT_ID.into(),
            name: "Agilent Seahorse XF assay result (.asyr)".into(),
            vendor: "Agilent (Seahorse)".into(),
            extensions: vec!["asyr".into()],
            family: "cell-metabolism".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::AGILENT_SEAHORSE_ASYR.confidence,
            known_gaps: vec![
                "OCR, ECAR and PER are not stored; they are computed (published compartment model, the file's constants) only for standard 96-well plates with Wave's AKOS settings, where they agree with Wave's own rates (OCR within 0.2 pmol/min, ECAR within 0.001 mpH/min); other plates (24-well, spheroid) return the emissions and a note saying why".into(),
                "PER uses kVol 1.6 (not stored; what Wave prints for the same plate) and is withheld by --strict".into(),
                "Raw LED on/off emission and reference arrays and the calibration arrays are not exposed; templates (.asyt) are not read".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if !openreadout_core::gzip::is_gzip(head) {
            return None;
        }
        if openreadout_core::gzip::peek(head, 4096).is_some_and(|x| seahorse::looks_like(&x)) {
            return Some(Detection {
                format_id: SEAHORSE_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        has_extension(path, &["asyr"]).then_some(Detection {
            format_id: SEAHORSE_FORMAT_ID,
            confidence: DetectConfidence::Likely,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let src = input
            .fs()
            .source(input.path())
            .map_err(|e| Error::io(input.path(), e))?;
        let size = src.size().map_err(|e| Error::io(input.path(), e))?;
        let gz = openreadout_core::gzip::GzipSource::open(
            src,
            input.path().display().to_string(),
            SEAHORSE_FORMAT_ID,
        )?;
        let len = gz.size().map_err(|e| Error::io(input.path(), e))?;
        if len > seahorse::MAX_XML {
            return Err(Error::unsupported(
                SEAHORSE_FORMAT_ID,
                format!("a {} MiB assay document", len >> 20),
                "Assay documents this large are not read; the largest seen is 17 MB.",
            ));
        }
        let n =
            usize::try_from(len).map_err(|_| Error::Other("document larger than memory".into()))?;
        let mut buf = vec![0u8; n];
        gz.read_exact_at(0, &mut buf)
            .map_err(|e| Error::io(input.path(), e))?;
        if let Some(p) = gz.problem() {
            return Err(Error::corrupt(
                SEAHORSE_FORMAT_ID,
                format!("the gzip stream is damaged ({p}); the assay XML is incomplete"),
            ));
        }
        let xml = String::from_utf8(buf).map_err(|_| {
            Error::corrupt(SEAHORSE_FORMAT_ID, "the assay document is not UTF-8 text")
        })?;
        let assay = seahorse::parse(&xml)?;
        Ok(Box::new(SeahorseDataset::new(
            self.descriptor(),
            input.path().to_path_buf(),
            size,
            assay,
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// Format id of Cytiva Biacore `.blr` result files.
pub const BIACORE_FORMAT_ID: &str = biacore::FORMAT_ID;

/// The Cytiva Biacore result-file reader (`.blr`).
#[derive(Debug, Default, Clone, Copy)]
pub struct BiacoreReader;

impl FormatReader for BiacoreReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::CYTIVA_BIACORE_BLR)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: BIACORE_FORMAT_ID.into(),
            name: "Cytiva Biacore result file (.blr)".into(),
            vendor: "Cytiva (Biacore)".into(),
            extensions: vec!["blr".into()],
            family: "spr".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::CYTIVA_BIACORE_BLR.confidence,
            known_gaps: vec![
                "Validated on Biacore T200 Control Software 2.0.1-2.0.2 result files (kinetics/affinity and immobilization wizards, manual runs); other Biacore systems (3000, X100, S200, 8K, Insight) and evaluation files (`.bme`, `.bie`) are not read".into(),
                "Sensorgrams and the reference-subtracted curves are returned as stored; kinetic or affinity fits (ka, kd, KD) are the evaluation software's and are not computed".into(),
                "Report points are the control software's; at 1 Hz storage they average data the file does not hold".into(),
                "Event-log codes, the quality and time-correction streams and the wizard template are kept or listed, not interpreted".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        (has_extension(path, &["blr"]) && head.starts_with(&openreadout_core::cfb::CFB_MAGIC))
            .then_some(Detection {
                format_id: BIACORE_FORMAT_ID,
                confidence: DetectConfidence::Likely,
                note: None,
            })
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        match biacore::sniff_input(head, input) {
            Some(biacore::Kind::Result) => Some(Detection {
                format_id: BIACORE_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            }),
            // an evaluation file is the evaluation reader's
            Some(biacore::Kind::Evaluation) => None,
            None => self.sniff(head, input.path()),
        }
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(BiacoreDataset::open_input(
            self.descriptor(),
            input,
            biacore::Kind::Result,
        )?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// The MicroCal ITC reader (`.itc`).
#[derive(Debug, Default, Clone, Copy)]
pub struct ItcReader;

/// The whole file, capped.
fn read_all(input: &Input, format: &'static str, cap: u64) -> Result<(Vec<u8>, u64)> {
    let f = input.open()?;
    let len = f.size().map_err(|e| Error::io(input.path(), e))?;
    if len > cap {
        return Err(Error::unsupported(
            format,
            format!("a {} MiB file", len >> 20),
            "Files this large are not read; no instrument writes them.",
        ));
    }
    let n = usize::try_from(len).map_err(|_| Error::Other("file larger than memory".into()))?;
    let mut buf = vec![0u8; n];
    f.read_exact_at(0, &mut buf)
        .map_err(|e| Error::io(input.path(), e))?;
    Ok((buf, len))
}

impl FormatReader for ItcReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::MICROCAL_ITC)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: ITC_FORMAT_ID.into(),
            name: "MicroCal ITC (.itc)".into(),
            vendor: "Malvern Panalytical (MicroCal)".into(),
            extensions: vec!["itc".into()],
            family: "calorimetry".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::MICROCAL_ITC.confidence,
            known_gaps: vec![
                "Validated on VP-ITC (VPViewer2000 1.4.8-1.30), iTC200 (1.25-1.26) and MicroCal ITC software 1.29 files; PEAQ-ITC `.apitc` and analysis projects (`.apj`) are not read".into(),
                "Time and differential power are validated against Origin's raw exports; the cell temperature column is inferred; further data columns (7- and 9-column files) are exposed unnamed".into(),
                "Integrated injection heats are the analysis software's (baseline and integration) and are not stored in the file; they are not computed".into(),
                "Calibration lines (`%`) are kept verbatim in the vendor tree, not interpreted".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if itc::looks_like(head) {
            return Some(Detection {
                format_id: ITC_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        (has_extension(path, &["itc"]) && head.is_empty()).then_some(Detection {
            format_id: ITC_FORMAT_ID,
            confidence: DetectConfidence::ExtensionOnly,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let (bytes, len) = read_all(input, ITC_FORMAT_ID, itc::MAX_BYTES)?;
        // Windows text: Latin-1 when it is not UTF-8
        let text = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => latin1(&e.into_bytes()),
        };
        let parsed = itc::parse(&text)?;
        Ok(Box::new(ItcDataset::new(
            self.descriptor(),
            input.path().to_path_buf(),
            len,
            parsed,
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// Provenance for the experiment fields every reader of this crate sets from the format itself
/// (vendor, instrument kind, technique) and for each measurement, where the builder did not
/// record one.
pub(crate) fn complete_provenance(e: &mut openreadout_core::experiment::Experiment) {
    use openreadout_core::experiment::Origin;
    use openreadout_core::provenance::Source;
    let o = |from: &str| Origin {
        source: Source::Inferred,
        from: from.into(),
    };
    for k in 0..e.measurements.len() {
        e.provenance
            .entry(format!("measurements[{k}]"))
            .or_insert_with(|| o("the reader's data layout (traces, tables, images)"));
    }
    if let Some(i) = &e.instrument {
        if i.vendor.is_some() {
            e.provenance
                .entry("instrument.vendor".into())
                .or_insert_with(|| o("the file format"));
        }
        if i.kind.is_some() {
            e.provenance
                .entry("instrument.kind".into())
                .or_insert_with(|| o("the file format"));
        }
    }
    if e.method.as_ref().is_some_and(|m| m.technique.is_some()) {
        e.provenance
            .entry("method.technique".into())
            .or_insert_with(|| o("the file format and the recorded mode"));
    }
}

/// Format id of Malvern Zetasizer `.dts` measurement files.
pub const ZETASIZER_FORMAT_ID: &str = zetasizer::FORMAT_ID;

/// The Malvern Zetasizer `.dts` reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct ZetasizerReader;

impl FormatReader for ZetasizerReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::MALVERN_ZETASIZER_DTS)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: ZETASIZER_FORMAT_ID.into(),
            name: "Malvern Zetasizer measurement file (.dts)".into(),
            vendor: "Malvern Panalytical".into(),
            extensions: vec!["dts".into()],
            family: "particle-sizing".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::MALVERN_ZETASIZER_DTS.confidence,
            known_gaps: vec![
                "Size and zeta results are returned as stored; the size distributions, correlation functions and phase plots are not decoded".into(),
                "Size peak widths and the number and volume peaks are stored but withheld: no Zetasizer export in the corpus holds them to check against".into(),
                "Size results are confirmed on Zetasizer software 7.10 and 7.12 only".into(),
                "Number and volume means and the diffusion coefficient the software exports are computed at export and are not returned".into(),
                "Molecular-weight, protein-mobility and other record kinds are listed, not decoded".into(),
                "ZS Xplorer .zmes files are not read".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        (zetasizer::looks_like(head) && has_extension(path, &["dts"])).then_some(Detection {
            format_id: ZETASIZER_FORMAT_ID,
            confidence: DetectConfidence::Likely,
            note: Some("a compound file named .dts".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let bytes =
            openreadout_core::series::read_all(input, ZETASIZER_FORMAT_ID, zetasizer::MAX_BYTES)?;
        let file = zetasizer::parse(&bytes, input.path())?;
        Ok(Box::new(openreadout_core::series::SeriesDataset::new(
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

/// Format id of Cytiva Biacore T200 evaluation files (`.bme`).
pub const BIACORE_BME_FORMAT_ID: &str = biacore::BME_FORMAT_ID;

/// The Cytiva Biacore T200 evaluation-file reader (`.bme`): the sensorgrams of the result files
/// it holds, and the evaluation software's fits.
#[derive(Debug, Default, Clone, Copy)]
pub struct BiacoreEvaluationReader;

impl FormatReader for BiacoreEvaluationReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::CYTIVA_BIACORE_BME)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: BIACORE_BME_FORMAT_ID.into(),
            name: "Cytiva Biacore T200 evaluation file (.bme)".into(),
            vendor: "Cytiva (Biacore)".into(),
            extensions: vec!["bme".into()],
            family: "spr".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::CYTIVA_BIACORE_BME.confidence,
            known_gaps: vec![
                "Fits (kinetics, affinity) are the evaluation software's, returned as stored with their parameters, standard errors and Chi²; they are not recomputed".into(),
                "The evaluation's own processed curves (aligned, blank-subtracted, in `EvaluationItemNBinary`), plots and report-point tables are not decoded; the sensorgrams are the result files' as the evaluation copied them".into(),
                "Concentration-analysis items (calibration curves) are listed without their results".into(),
                "Biacore 8K, S200 and Insight evaluation files are not read".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        (has_extension(path, &["bme"]) && head.starts_with(&openreadout_core::cfb::CFB_MAGIC))
            .then_some(Detection {
                format_id: BIACORE_BME_FORMAT_ID,
                confidence: DetectConfidence::Likely,
                note: None,
            })
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        match biacore::sniff_input(head, input) {
            Some(biacore::Kind::Evaluation) => Some(Detection {
                format_id: BIACORE_BME_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            }),
            Some(biacore::Kind::Result) => None,
            None => self.sniff(head, input.path()),
        }
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(BiacoreDataset::open_input(
            self.descriptor(),
            input,
            biacore::Kind::Evaluation,
        )?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
