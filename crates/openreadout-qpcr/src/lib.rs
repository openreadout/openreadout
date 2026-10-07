//! Real-time PCR (qPCR) readers: RDML (the open interchange format, read and written),
//! Applied Biosystems / Thermo Fisher `.eds` experiment documents (QuantStudio, ViiA 7,
//! StepOne, 7500), Qiagen Rotor-Gene `.rex`, Roche LightCycler 480 `.ixo` (and LightCycler 96
//! `.lc96p`, an RDML zip with the vendor's analysis), and the results exports of Applied
//! Biosystems and Bio-Rad CFX software; Bio-Rad CFX `.pcrd` is detected and refused (it is
//! encrypted).
//!
//! Every file becomes the same normalized model (`docs/formats/qpcr.md`): plate runs of
//! reactions (wells), each with the targets measured in it, the vendor-computed result (Cq,
//! threshold, baseline window, Tm, amplification status), the amplification curve (raw and
//! baseline-corrected fluorescence per cycle) and the melt curve. A dataset exposes:
//!
//! - table 0 `results`: one row per well × target (well, row, col, cq, target, sample, dye,
//!   task, quantity, …);
//! - table 1 `amplification` and table 2 `melt`: the curves in long form;
//! - traces: per run and dye, the amplification curves (raw and baseline-corrected), melt
//!   curves and their −dF/dT, one channel per well;
//! - [`qpcr_report`]: named records and the analyses (our own Cq, ΔΔCq, standard curves).
//!
//! [`export_rdml`] writes any readable file as RDML 1.3 and verifies it by reading it back.
//!
//! # Example
//!
//! ```
//! use openreadout_core::FormatReader;
//! use openreadout_qpcr::{EdsReader, RdmlReader};
//!
//! for r in [RdmlReader.descriptor(), EdsReader.descriptor()] {
//!     println!("{} ({}): .{}", r.name, r.id, r.extensions.join(", ."));
//! }
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]
// The parsers collect small tuples whose meaning is documented where they are built.
#![allow(clippy::type_complexity)]

mod analysis;
mod assurance;
mod dataset;
mod eds;
mod export;
mod ixo;
mod lc96;
mod model;
mod rdml;
mod rdml_write;
mod report;
mod rex;
mod xml;

use std::path::Path;

use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::provenance::Confidence;
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;
use openreadout_core::zip;
use openreadout_core::{Error, Result};

pub use dataset::{CQ_STATUSES, QpcrDataset, RESULT_COLUMNS};
pub use rdml_write::{RDML_WRITE_VERSION, RdmlExportReport, default_rdml_output, export_rdml};
pub use report::{
    CQ_DETERMINED, CQ_NO_RESULT, CQ_UNDETERMINED, CqComparison, CqCounts, QpcrAssayRecord,
    QpcrReport, QpcrReportRequest, RelativeQuantity, StandardCurveFit, TargetCqSummary,
    qpcr_report,
};

/// Format id of RDML files.
pub const RDML_FORMAT_ID: &str = rdml::FORMAT_ID;
/// Format id of Applied Biosystems `.eds` experiment documents.
pub const EDS_FORMAT_ID: &str = eds::FORMAT_ID;
/// Format id of Bio-Rad CFX `.pcrd` files (detected, not read).
pub const PCRD_FORMAT_ID: &str = "bio-rad-pcrd";
/// Format id of Qiagen Rotor-Gene `.rex` files.
pub const REX_FORMAT_ID: &str = rex::FORMAT_ID;
/// Format id of Roche LightCycler 480 `.ixo` experiment files.
pub const IXO_FORMAT_ID: &str = ixo::FORMAT_ID;
/// Format id of qPCR results exports (Applied Biosystems `Results` tables, Bio-Rad CFX
/// `Quantification Cq Results`).
pub const EXPORT_FORMAT_ID: &str = export::FORMAT_ID;

/// Name of the first member in a zip head (the local file header at offset 0).
fn first_member(head: &[u8]) -> Option<String> {
    Some(
        zip::first_member_name(head)?
            .replace('\\', "/")
            .to_ascii_lowercase(),
    )
}

/// General-purpose flag bit 0 of the first member (encryption).
fn first_member_encrypted(head: &[u8]) -> bool {
    zip::first_header(head)
        .and_then(|at| head.get(at + 6))
        .is_some_and(|f| f & 1 == 1)
}

fn descriptor(
    id: &str,
    name: &str,
    vendor: &str,
    exts: &[&str],
    can_read: bool,
    can_write: bool,
    confidence: Confidence,
    gaps: &[&str],
) -> FormatDescriptor {
    FormatDescriptor {
        id: id.into(),
        name: name.into(),
        vendor: vendor.into(),
        extensions: exts.iter().map(|s| (*s).to_string()).collect(),
        family: "qpcr".into(),
        can_read,
        can_write,
        confidence,
        known_gaps: gaps.iter().map(|s| (*s).to_string()).collect(),
    }
}

/// RDML (Real-time PCR Data Markup Language) 1.0-1.4, zipped (`.rdml`, `.rdm`) or bare XML.
#[derive(Debug, Default, Clone, Copy)]
pub struct RdmlReader;

impl FormatReader for RdmlReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::RDML)
    }

    fn descriptor(&self) -> FormatDescriptor {
        descriptor(
            RDML_FORMAT_ID,
            "RDML (Real-time PCR Data Markup Language)",
            "RDML consortium (open standard); written by Bio-Rad CFX Maestro, Roche LightCycler 96/480, Applied Biosystems StepOne, qPCR analysis tools",
            &["rdml", "rdm", "lc96p", "xml"],
            true,
            true,
            assurance::RDML.confidence,
            &[
                "Digital PCR partitions (RDML 1.3 `partitions`, partition tables) are counted, not read",
                "Documentation, xRef, cDNA synthesis and commercial-assay elements are kept only in the vendor tree",
                "LightCycler 96 (.lc96p, or RDML with its app_data.xml/calculated_data.xml): the software's calls, replicate Cq mean/SD and exclusions are attached; an RDML cq under a call other than Positive is not a Cq, even when it is a plausible number (kept as cq_stored; the software's analyses list none for such graphs, and rdmlpython and other RDML readers report the stored number). Validated against the software's results table for 3 files (108 Cqs) and its absolute-quantification analysis in a fourth (56 Cqs, 8 Negative graphs without one); melting-peak Tm of LightCycler 96 analyses is not read",
            ],
        )
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if zip::is_zip(head) {
            let first = first_member(head).unwrap_or_default();
            if first == "rdml_data.xml" {
                return Some(Detection {
                    format_id: RDML_FORMAT_ID,
                    confidence: DetectConfidence::Definite,
                    note: None,
                });
            }
            return has_extension(path, &["rdml", "rdm", "lc96p"]).then_some(Detection {
                format_id: RDML_FORMAT_ID,
                confidence: DetectConfidence::Likely,
                note: Some("zip archive with an .rdml (or LightCycler 96 .lc96p) extension".into()),
            });
        }
        let text = String::from_utf8_lossy(&head[..head.len().min(4096)]);
        rdml::looks_like_rdml(&text).then_some(Detection {
            format_id: RDML_FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: Some("uncompressed RDML XML".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(QpcrDataset::open_as(input, RDML_FORMAT_ID)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// Applied Biosystems / Thermo Fisher experiment documents (`.eds`).
#[derive(Debug, Default, Clone, Copy)]
pub struct EdsReader;

impl FormatReader for EdsReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::APPLIED_BIOSYSTEMS_EDS)
    }

    fn descriptor(&self) -> FormatDescriptor {
        descriptor(
            EDS_FORMAT_ID,
            "Applied Biosystems experiment document (.eds)",
            "Thermo Fisher Scientific / Applied Biosystems (QuantStudio 1-7 Pro, 12K Flex, ViiA 7, StepOne, StepOnePlus, 7500)",
            &["eds"],
            true,
            false,
            assurance::APPLIED_BIOSYSTEMS_EDS.confidence,
            &[
                "Raw optical images (`apldbio/sds/images/*.tiff`, `quant/*.quant`) are listed, not decoded; files that keep only images have no curves",
                "Genotyping (allelic discrimination): the vendor's calls are the table genotypes (SDS layout; codes mapped from 1000 Genomes genotypes of 4 reference samples at 2 markers of one QuantStudio 7 run); JSON-layout (Design & Analysis 2) genotyping results are not read",
                "Relative-quantification results (RQ, ΔΔCt) computed by the vendor are not parsed; `analyze qpcr --ddcq` computes them",
                "SDS/7500 layouts: the threshold comes from the analysis protocol (equal to the exported Ct Threshold in 8 StepOnePlus runs); an automatic baseline window is recovered per well from Rn − ΔRn (572 of 574 wells equal to the export) or left empty, never reported as the setting",
            ],
        )
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if !zip::is_zip(head) {
            return None;
        }
        let first = first_member(head).unwrap_or_default();
        let known = first.starts_with("apldbio/")
            || [
                "manifest.mf",
                "summary.json",
                "checksum",
                "setup/",
                "primary/",
                "run/",
                "calibrations/",
                "extensions/",
            ]
            .iter()
            .any(|p| first.starts_with(p));
        if known && has_extension(path, &["eds", "edt"]) {
            return Some(Detection {
                format_id: EDS_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        if first.starts_with("apldbio/sds/") {
            return Some(Detection {
                format_id: EDS_FORMAT_ID,
                confidence: DetectConfidence::Likely,
                note: Some("apldbio/sds/ zip without an .eds extension".into()),
            });
        }
        has_extension(path, &["eds"]).then_some(Detection {
            format_id: EDS_FORMAT_ID,
            confidence: DetectConfidence::Likely,
            note: Some("zip archive with an .eds extension".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(QpcrDataset::open_as(input, EDS_FORMAT_ID)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// Bio-Rad CFX Maestro data files (`.pcrd`): recognised and refused (encrypted container).
#[derive(Debug, Default, Clone, Copy)]
pub struct PcrdReader;

/// Hint for `.pcrd` files.
const PCRD_HINT: &str = "Bio-Rad .pcrd files are encrypted with a key only the vendor software has. Open the file in CFX Maestro (or CFX Manager) and use File > Export > RDML File (or Export > All Data Sheets), then read the .rdml with openreadout.";

impl FormatReader for PcrdReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::BIO_RAD_PCRD)
    }

    fn descriptor(&self) -> FormatDescriptor {
        descriptor(
            PCRD_FORMAT_ID,
            "Bio-Rad CFX data file (.pcrd)",
            "Bio-Rad Laboratories (CFX96, CFX384, CFX Opus; CFX Maestro / CFX Manager)",
            &["pcrd"],
            false,
            false,
            assurance::BIO_RAD_PCRD.confidence,
            &[
                "Encrypted container (a zip whose single member is encrypted): detected and refused with exit 6; export RDML from CFX Maestro instead",
            ],
        )
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if !zip::is_zip(head) {
            return None;
        }
        let first = first_member(head).unwrap_or_default();
        if first == "datafile.pcrd" {
            return Some(Detection {
                format_id: PCRD_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: Some(if first_member_encrypted(head) {
                    "encrypted Bio-Rad CFX data file (not readable; export RDML from CFX Maestro)"
                        .into()
                } else {
                    "Bio-Rad CFX data file".into()
                }),
            });
        }
        has_extension(path, &["pcrd"]).then_some(Detection {
            format_id: PCRD_FORMAT_ID,
            confidence: DetectConfidence::Likely,
            note: Some("zip archive with a .pcrd extension".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        // Look at the container so a truncated file is reported as corrupt, not unsupported.
        let z = zip::ZipIndex::open(input.fs(), input.path(), PCRD_FORMAT_ID)?;
        let encrypted = z.members.iter().any(|m| m.encrypted);
        Err(Error::Unsupported {
            format: PCRD_FORMAT_ID,
            feature: if encrypted {
                format!(
                    "reading an encrypted Bio-Rad CFX data file ({} encrypted zip member(s))",
                    z.members.iter().filter(|m| m.encrypted).count()
                )
            } else {
                "reading Bio-Rad CFX data files".into()
            },
            hint: Some(PCRD_HINT.into()),
        })
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// Qiagen Rotor-Gene Q / 6000 run files (`.rex`, XML).
#[derive(Debug, Default, Clone, Copy)]
pub struct RexReader;

impl FormatReader for RexReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::ROTOR_GENE_REX)
    }

    fn descriptor(&self) -> FormatDescriptor {
        descriptor(
            REX_FORMAT_ID,
            "Qiagen Rotor-Gene run file (.rex)",
            "Qiagen (Rotor-Gene Q, Rotor-Gene 6000; Rotor-Gene Q Series Software)",
            &["rex"],
            true,
            false,
            assurance::ROTOR_GENE_REX.confidence,
            &[
                "No Cq values: Rotor-Gene files keep raw channel readings and the run setup; the analysis lives in the vendor software (`analyze qpcr --cq` computes Cq)",
                "Melt channels are read when the file records them as readings of a melt step; gain optimisation and other auxiliary readings are listed only",
            ],
        )
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        let text = String::from_utf8_lossy(&head[..head.len().min(4096)]);
        if text.contains("<RexHeader>") || (text.contains("<Experiment>") && text.contains("REX "))
        {
            return Some(Detection {
                format_id: REX_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        (has_extension(path, &["rex"]) && text.trim_start().starts_with('<')).then_some(Detection {
            format_id: REX_FORMAT_ID,
            confidence: DetectConfidence::Likely,
            note: Some("XML file with a .rex extension".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(QpcrDataset::open_as(input, REX_FORMAT_ID)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// Roche LightCycler 480 experiment files (`.ixo`, an XML object stream).
#[derive(Debug, Default, Clone, Copy)]
pub struct IxoReader;

impl FormatReader for IxoReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::ROCHE_LIGHTCYCLER_IXO)
    }

    fn descriptor(&self) -> FormatDescriptor {
        descriptor(
            IXO_FORMAT_ID,
            "Roche LightCycler 480 experiment (.ixo)",
            "Roche Diagnostics (LightCycler 480, LightCycler 480 II; LightCycler 480 software 1.5)",
            &["ixo"],
            true,
            false,
            assurance::ROCHE_LIGHTCYCLER_IXO.confidence,
            &[
                "Read: run and instrument, thermal protocol, detection channels, sample names and per-channel target/type/concentration, raw fluorescence per cycle and melt readings (instrument units, no color compensation), and the vendor's absolute-quantification results (Cp and positive/negative call). Validated on 4 LightCycler 480 QC runs (software 1.5.0 and 1.5.1): every vendor-positive well rises and every negative stays flat in the analysed channel",
                "Only `Legacy Absolute Quantification Analysis` results are read (one channel, no ratio); Tm calling, genotyping, relative quantification and other analyses are listed in the vendor tree, not read; a call other than 0 or 2 gives no Cq",
                "Vendor melt smoothing and derivative arrays (DARZ/FORM binary properties) and the temperature log are not decoded; -dF/dT is computed from the raw melt readings",
                "Our own Cq (`analyze qpcr --cq`) is not the LightCycler 480's Cp algorithm (not public); use the vendor Cp in `cq`",
                "The closing checksum line is kept (`vendor.trailer`), not verified",
            ],
        )
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        let text = String::from_utf8_lossy(&head[..head.len().min(4096)]);
        if text.contains("<objectstream") && text.contains("signature=\"IXOS\"") {
            return Some(Detection {
                format_id: IXO_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        (has_extension(path, &["ixo"]) && text.trim_start().starts_with('<')).then_some(Detection {
            format_id: IXO_FORMAT_ID,
            confidence: DetectConfidence::Likely,
            note: Some("XML file with an .ixo extension".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(QpcrDataset::open_as(input, IXO_FORMAT_ID)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// Results exports of qPCR software: an Applied Biosystems `Results` table (StepOne, 7500,
/// QuantStudio, ViiA 7 software; `.xls`, `.xlsx`, text) or a Bio-Rad CFX `Quantification Cq
/// Results` table (`.csv`, `.txt`, `.xlsx`).
#[derive(Debug, Default, Clone, Copy)]
pub struct ExportReader;

/// File extensions of results exports.
const EXPORT_EXTENSIONS: [&str; 5] = ["xls", "xlsx", "csv", "txt", "tsv"];

impl FormatReader for ExportReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::QPCR_RESULTS_EXPORT)
    }

    fn descriptor(&self) -> FormatDescriptor {
        descriptor(
            EXPORT_FORMAT_ID,
            "qPCR results export (Applied Biosystems, Bio-Rad CFX)",
            "Applied Biosystems / Thermo Fisher (StepOne, 7500, QuantStudio, ViiA 7 software); Bio-Rad (CFX Manager, CFX Maestro)",
            &EXPORT_EXTENSIONS,
            true,
            false,
            assurance::QPCR_RESULTS_EXPORT.confidence,
            &[
                "Applied Biosystems: the Results table (Cq or Undetermined, Cq mean and SD, threshold, baseline window, Amp Status, Tm, quantities, RQ and ΔCq), and amplification (Rn, ΔRn) and melt curves when the export holds those sheets; Sample Setup, Multicomponent Data and Raw Data sheets are not read",
                "Bio-Rad CFX: the Quantification Cq Results table only (no curves); NaN and N/A Cq are read as no Cq; the export holds no plate size",
                "Other exports (Amplification Results, Melt Curve Peak Results, End Point Results, Allelic Discrimination) are not recognised",
            ],
        )
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        // text exports: the header row is in the first kilobytes
        if export::is_workbook(head) || !has_extension(path, &EXPORT_EXTENSIONS) {
            return None;
        }
        let (_, sheets) = export::read_sheets(head.to_vec()).ok()?;
        export::classify(&sheets).map(|layout| Detection {
            format_id: EXPORT_FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: Some(format!("{} results export", layout.name())),
        })
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        let path = input.path();
        if !has_extension(path, &EXPORT_EXTENSIONS) {
            return None;
        }
        if !export::is_workbook(head) {
            return self.sniff(head, path);
        }
        // a workbook: its cells decide (exports are small; larger workbooks are not looked into)
        let size = input.fs().metadata(path).ok()?.len();
        if size > 16 << 20 {
            return None;
        }
        let bytes = input.fs().read(path).ok()?;
        let (_, sheets) = export::read_sheets(bytes).ok()?;
        export::classify(&sheets).map(|layout| Detection {
            format_id: EXPORT_FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: Some(format!("{} results export (workbook)", layout.name())),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(QpcrDataset::open_as(input, EXPORT_FORMAT_ID)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// Open a qPCR file through a registry: detection decides, so a `.pcrd` gets its own refusal
/// (exit 6 with a hint) and a file of another format says what `analyze qpcr` reads.
pub fn open_qpcr(reg: &openreadout_core::Registry, path: &Path) -> Result<QpcrDataset> {
    let (reader, det) = reg.detect(path)?;
    match det.format_id {
        RDML_FORMAT_ID | EDS_FORMAT_ID | REX_FORMAT_ID | IXO_FORMAT_ID => QpcrDataset::open(path),
        EXPORT_FORMAT_ID => QpcrDataset::open_as(&Input::local(path), EXPORT_FORMAT_ID),
        other => {
            // `.pcrd` explains itself when opened
            reader.open(path)?;
            Err(Error::unsupported(
                "qpcr",
                format!("qPCR analysis of a {other} file"),
                "qPCR analysis reads real-time PCR files: RDML (.rdml, LightCycler 96 .lc96p), Applied Biosystems (.eds), Rotor-Gene (.rex), LightCycler 480 (.ixo), and the results exports of Applied Biosystems and Bio-Rad CFX software. Use `info` to see what this file holds.",
            ))
        }
    }
}

/// The descriptor of a qPCR format id (for datasets).
pub(crate) fn descriptor_of(id: &str) -> FormatDescriptor {
    match id {
        RDML_FORMAT_ID => RdmlReader.descriptor(),
        REX_FORMAT_ID => RexReader.descriptor(),
        IXO_FORMAT_ID => IxoReader.descriptor(),
        PCRD_FORMAT_ID => PcrdReader.descriptor(),
        EXPORT_FORMAT_ID => ExportReader.descriptor(),
        _ => EdsReader.descriptor(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs() {
        let rdml = zip::zip_bytes(&[("rdml_data.xml", b"<rdml/>")]).unwrap();
        let d = RdmlReader.sniff(&rdml, Path::new("x.zip")).unwrap();
        assert_eq!(d.confidence, DetectConfidence::Definite);
        assert!(EdsReader.sniff(&rdml, Path::new("x.zip")).is_none());
        let eds = zip::zip_bytes(&[("apldbio/sds/experiment.xml", b"<Experiment/>")]).unwrap();
        assert_eq!(
            EdsReader
                .sniff(&eds, Path::new("a.eds"))
                .unwrap()
                .confidence,
            DetectConfidence::Definite
        );
        assert!(RdmlReader.sniff(&eds, Path::new("a.eds")).is_none());
        let pcrd = zip::zip_bytes(&[("datafile.pcrd", b"x")]).unwrap();
        assert_eq!(
            PcrdReader
                .sniff(&pcrd, Path::new("a.pcrd"))
                .unwrap()
                .format_id,
            PCRD_FORMAT_ID
        );
        assert!(
            RexReader
                .sniff(
                    b"<?xml version=\"1.0\"?>\n<Experiment>\n<RexHeader>REX 3.15</RexHeader>",
                    Path::new("r.rex")
                )
                .is_some()
        );
        assert!(
            RdmlReader
                .sniff(b"<rdml version=\"1.2\">", Path::new("r.xml"))
                .is_some()
        );
        assert!(RdmlReader.sniff(b"<html>", Path::new("r.xml")).is_none());
    }

    #[test]
    fn pcrd_is_refused_with_exit_6() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.pcrd");
        std::fs::write(&p, zip::zip_bytes(&[("datafile.pcrd", b"x")]).unwrap()).unwrap();
        let err = PcrdReader.open(&p).map(|_| ()).unwrap_err();
        assert_eq!(err.exit_code(), 6);
        assert!(err.hint().unwrap().contains("RDML"));
        std::fs::write(&p, b"PK\x03\x04truncated").unwrap();
        assert_eq!(PcrdReader.open(&p).map(|_| ()).unwrap_err().exit_code(), 4);
    }
}
