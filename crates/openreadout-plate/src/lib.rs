//! Reader for microplate-reader exports: the text, CSV and XLSX files that plate-reader
//! software writes for spreadsheets.
//!
//! Dialects (docs/formats/plate-readers.md): Agilent BioTek Gen5, Molecular Devices SoftMax
//! Pro (text exports and the binary `.pda`/`.sda` documents), BMG LABTECH MARS and SMART
//! Control, PerkinElmer/Revvity EnVision and Kaleido, Tecan i-control and Magellan, Thermo
//! SkanIt, and generic plate matrices. Provenance: docs/provenance/plate-readers.md.
//! Every plate read is exposed as one long-form table
//! (`well,row,col,read,wavelength_nm,time_s,value`), and [`export_asm`] writes a file as
//! Allotrope Simple Model plate-reader JSON.
//!
//! # Example
//!
//! Every reader implements [`openreadout_core::FormatReader`]: `descriptor`
//! says what it reads, `sniff` recognises a file from its first bytes, and `open` returns a
//! [`openreadout_core::Dataset`] with a header-only `info()` summary and lazy data
//! access.
//!
//! ```
//! use openreadout_core::FormatReader;
//! use openreadout_plate::PlateReader;
//!
//! let format = PlateReader.descriptor();
//! assert_eq!(format.id, openreadout_plate::FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::FormatReader;
//! use openreadout_plate::PlateReader;
//!
//! let mut dataset = PlateReader.open(Path::new("absorbance.xlsx"))?;
//! let info = dataset.info()?; // headers only
//! let table = &info.tables[0];
//! println!("{} row(s) x {} column(s)", table.row_count, table.columns.len());
//! // The first 100 rows, column-major (`columns[c][r]`).
//! let rows = dataset.read_table(0, 0, 100)?;
//! for (column, values) in table.columns.iter().zip(&rows.columns) {
//!     println!("{}: {:?}", column.name, values.first());
//! }
//! # Ok::<(), openreadout_core::Error>(())
//! ```
//!
//! Applications that accept any instrument file usually register every reader in an
//! [`openreadout_core::Registry`] and let it detect the format; that is what the
//! `openreadout` command-line tool does.
//!
//! # API stability
//!
//! The supported API is what this page documents. The parser modules are public only so that
//! tests and fuzz targets can reach them: they are hidden from this documentation and may change
//! in any release.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod asm;
mod assurance;
mod dataset;
mod datetime;
mod grid;
mod model;
mod sheet;
mod text;
mod vendors;

use std::path::Path;

use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Result};

pub use asm::{ASM_MANIFEST, AsmExportReport, export_asm};
pub use dataset::{PlateDataset, TABLE_COLUMNS};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "plate";

/// Largest export read into memory (plate-reader exports are kilobytes to a few megabytes).
const MAX_FILE_BYTES: u64 = 256 << 20;

const TEXT_EXTENSIONS: [&str; 5] = ["txt", "csv", "tsv", "tab", "asc"];
const BOOK_EXTENSIONS: [&str; 5] = ["xlsx", "xlsm", "xls", "xlsb", "ods"];
/// SoftMax Pro binary documents (`.pda` SoftMax Pro 5; `.sda`/`.pda` SoftMax Pro 6/7).
const BINARY_EXTENSIONS: [&str; 2] = ["pda", "sda"];
/// Gen5 experiment (compound file) and protocol files.
const GEN5_EXTENSIONS: [&str; 2] = ["xpt", "prt"];

/// The plate-reader export reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct PlateReader;

/// Which dialect a text head looks like, if any.
fn sniff_text(text: &str) -> Option<model::Kind> {
    use model::Kind;
    if vendors::softmax::sniff(text) {
        Some(Kind::SoftMaxPro)
    } else if vendors::gen5::sniff(text) {
        Some(Kind::Gen5)
    } else if vendors::bmg::sniff(text) {
        Some(Kind::BmgMars)
    } else if vendors::envision::sniff_envision(text) {
        Some(Kind::EnVision)
    } else if vendors::envision::sniff_kaleido(text) {
        Some(Kind::Kaleido)
    } else if vendors::tecan::sniff_icontrol(text) {
        Some(Kind::TecanIControl)
    } else {
        None
    }
}

fn sniff_book(book: &sheet::Book) -> Option<model::Kind> {
    use model::Kind;
    let first_text = |f: &dyn Fn(&str) -> bool| {
        book.sheets.iter().any(|s| {
            let head: String = (0..s.rows.len().min(80))
                .map(|r| {
                    s.rows[r]
                        .iter()
                        .map(sheet::Cell::text)
                        .collect::<Vec<_>>()
                        .join("\t")
                })
                .collect::<Vec<_>>()
                .join("\n");
            f(&head)
        })
    };
    if vendors::tecan::is_icontrol_book(book) {
        Some(Kind::TecanIControl)
    } else if vendors::skanit::is_skanit_book(book) {
        Some(Kind::SkanIt)
    } else if vendors::tecan::is_magellan_book(book) {
        Some(Kind::TecanMagellan)
    } else if vendors::bmg::is_smart_control(book) {
        Some(Kind::BmgSmartControl)
    } else if first_text(&vendors::gen5::sniff) {
        Some(Kind::Gen5)
    } else if vendors::bmg::is_mars_book(book) {
        Some(Kind::BmgMars)
    } else if vendors::generic::sniff(book) {
        Some(Kind::Generic)
    } else {
        None
    }
}

/// Read and parse an export.
pub(crate) fn load(fs: &Fs, path: &Path) -> Result<model::Export> {
    let meta = fs.metadata(path).map_err(|e| Error::io(path, e))?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(Error::unsupported(
            FORMAT_ID,
            format!(
                "a {} MiB export (larger than {} MiB)",
                meta.len() >> 20,
                MAX_FILE_BYTES >> 20
            ),
            "Plate-reader exports are small; this file is probably not one. Split it or open it with the vendor software.",
        ));
    }
    let bytes = fs.read(path).map_err(|e| Error::io(path, e))?;
    if let Some(ex) = binary_document(path, &bytes)? {
        return Ok(ex);
    }
    let file_name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let mut ex = if sheet::is_workbook(&bytes) {
        let book = if fs.is_local() {
            drop(bytes);
            sheet::workbook_book(path)?
        } else {
            sheet::workbook_book_bytes(path, bytes)?
        };
        let kind = sniff_book(&book).ok_or_else(|| Error::UnknownFormat {
            path: path.to_path_buf(),
        })?;
        parse_kind(kind, &book, &file_name)
    } else {
        let book = sheet::text_book(&bytes);
        let (text, _) = text::decode(&bytes[..bytes.len().min(1 << 20)]);
        let kind = sniff_text(&text)
            .or_else(|| vendors::generic::sniff(&book).then_some(model::Kind::Generic))
            .ok_or_else(|| Error::UnknownFormat {
                path: path.to_path_buf(),
            })?;
        parse_kind(kind, &book, &file_name)
    };
    ex.finish();
    if ex.blocks.iter().all(|b| b.obs.is_empty()) && ex.kind == model::Kind::Generic {
        return Err(Error::UnknownFormat {
            path: path.to_path_buf(),
        });
    }
    Ok(ex)
}

/// SoftMax Pro binary documents: decoded when their layout is one the corpus validates,
/// otherwise refused (exit 6). `Ok(None)` for files that are not binary documents.
fn binary_document(path: &Path, bytes: &[u8]) -> Result<Option<model::Export>> {
    let gen5_hint = "Export the plate from Gen5 as text or Excel (File > Export, or a Power Export) and read that file.";
    if vendors::gen5_xpt::sniff(bytes, has_extension(path, &["xpt"])) {
        let mut ex = vendors::gen5_xpt::parse(bytes).map_err(|why| {
            Error::unsupported(
                FORMAT_ID,
                format!("Gen5 experiment file that is not decoded ({why})"),
                gen5_hint,
            )
        })?;
        ex.finish();
        if ex.blocks.iter().all(|b| b.obs.is_empty()) {
            return Err(Error::unsupported(
                FORMAT_ID,
                "Gen5 experiment file whose plate data are not decoded",
                gen5_hint,
            ));
        }
        return Ok(Some(ex));
    }
    if has_extension(path, &["prt"]) && vendors::gen5_xpt::is_protocol(bytes) {
        return Err(Error::unsupported(
            FORMAT_ID,
            "Gen5 protocol file (it holds the procedure, no plate data)",
            "Open the experiment (.xpt) run from this protocol, or its text/Excel export.",
        ));
    }
    let hint = "Export the plate data from SoftMax Pro as text (File > Export, .txt, Plate or Time format) and read that file.";
    let ex = if vendors::softmax_sda::sniff(bytes) {
        vendors::softmax_sda::parse(bytes)
    } else if vendors::softmax_pda::sniff(bytes) {
        vendors::softmax_pda::parse(bytes)
    } else if has_extension(path, &BINARY_EXTENSIONS) && !sheet::is_workbook(bytes) {
        let (text, _) = text::decode(&bytes[..bytes.len().min(4096)]);
        if vendors::softmax::sniff(&text) {
            return Ok(None); // a text export saved with a binary extension
        }
        return Err(Error::unsupported(
            FORMAT_ID,
            "SoftMax Pro binary document of an unrecognised layout",
            hint,
        ));
    } else {
        return Ok(None);
    };
    let Some(mut ex) = ex else {
        return Err(Error::unsupported(
            FORMAT_ID,
            "SoftMax Pro binary document of an unrecognised layout",
            hint,
        ));
    };
    ex.finish();
    if ex.blocks.iter().all(|b| b.obs.is_empty()) {
        let why = ex
            .sections
            .get("refused_plates")
            .and_then(serde_json::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "no plate section holds data".into());
        return Err(Error::unsupported(
            FORMAT_ID,
            format!("SoftMax Pro document whose plate data are not decoded ({why})"),
            hint,
        ));
    }
    Ok(Some(ex))
}

fn parse_kind(kind: model::Kind, book: &sheet::Book, file_name: &str) -> model::Export {
    use model::Kind;
    match kind {
        Kind::Gen5 => vendors::gen5::parse(book, file_name),
        Kind::SoftMaxPro => vendors::softmax::parse(book),
        Kind::BmgMars => vendors::bmg::parse(book, false),
        Kind::BmgSmartControl => vendors::bmg::parse(book, true),
        Kind::EnVision => vendors::envision::parse_envision(book),
        Kind::Kaleido => vendors::envision::parse_kaleido(book),
        Kind::TecanIControl => vendors::tecan::parse_icontrol(book),
        Kind::TecanMagellan => vendors::tecan::parse_magellan(book),
        Kind::SkanIt => vendors::skanit::parse(book),
        Kind::Generic => vendors::generic::parse(book),
    }
}

impl FormatReader for PlateReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::PLATE)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "Plate-reader exports".into(),
            vendor: "Agilent BioTek, Molecular Devices, BMG LABTECH, Revvity/PerkinElmer, Tecan, Thermo Fisher Scientific".into(),
            extensions: TEXT_EXTENSIONS
                .iter()
                .chain(BOOK_EXTENSIONS.iter())
                .chain(BINARY_EXTENSIONS.iter())
                .chain(GEN5_EXTENSIONS.iter())
                .map(|s| (*s).to_string())
                .collect(),
            family: "plate-reader".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::PLATE.confidence,
            known_gaps: vec![
                "SoftMax Pro binary documents: SoftMax Pro 5 .pda plate sections (absorbance, endpoint or kinetic, one wavelength, 96 wells; 4 documents from 2 labs validated value-for-value against their text exports) and SoftMax Pro 6/7 .sda endpoint plates (absorbance, fluorescence, luminescence; one wavelength; 6 documents validated) are decoded; kinetic, spectrum and well-scan sections of 6/7 documents, several wavelengths, other plate sizes and cuvette sets are refused (exit 6: export them as text)".into(),
                "Gen5 experiment files (.xpt): endpoint and kinetic reads decoded from the plate archives (validated on Gen5 2.08, 3.04, 3.11 and 3.15 files from three labs against their exports); the detection mode comes from the read name (number, ex,em, Lum), other names are `unknown`; flagged cells (OVRFLW etc.) are NaN without their text; area scans, spectra, well scans and protocol files (.prt) are refused".into(),
                "Gen5 area scans and multi-plate kinetic exports, SoftMax Pro well scans and fluorescence spectra are not decoded".into(),
                "Values calculated by the vendor software are kept as `calculated` reads; curve fits and group tables are kept verbatim only (`openreadout analyze assay` recomputes standard curves, IC50s and kinetics from the reads). SoftMax Pro 5 document templates give the layout (samples, groups, `Blank` group as blanks); SoftMax Pro text-export group tables and SoftMax Pro 6/7 group sections are not used as layouts, and document-stored reduced values are not read".into(),
                "Day/month order of slash dates is assumed month-first when ambiguous (flagged)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if vendors::softmax_sda::sniff_document(head)
            || (vendors::softmax_sda::sniff(head) && has_extension(path, &BINARY_EXTENSIONS))
        {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: Some("SoftMax Pro 6/7 binary document".into()),
            });
        }
        if vendors::gen5_xpt::sniff(head, has_extension(path, &["xpt"])) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: Some("Gen5 experiment file (compound file)".into()),
            });
        }
        if has_extension(path, &["prt"]) && vendors::gen5_xpt::is_protocol(head) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Likely,
                note: Some("Gen5 protocol file: holds no plate data (refused on open)".into()),
            });
        }
        if vendors::softmax_pda::sniff(head) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: Some("SoftMax Pro 5 binary document".into()),
            });
        }
        if has_extension(path, &BINARY_EXTENSIONS) && !head.is_empty() && head.contains(&0) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::ExtensionOnly,
                note: Some(
                    "SoftMax Pro binary document of an unrecognised layout: refused on open".into(),
                ),
            });
        }
        if sheet::is_workbook(head) {
            return has_extension(path, &BOOK_EXTENSIONS).then(|| Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Likely,
                note: Some(
                    "spreadsheet workbook; the exporter is identified from its sheets on open"
                        .into(),
                ),
            });
        }
        if head.is_empty()
            || head.iter().take(4096).filter(|b| **b == 0).count() > 64
                && !head.starts_with(&[0xFF, 0xFE])
                && !head.starts_with(&[0xFE, 0xFF])
        {
            return None;
        }
        let (text, _) = text::decode(head);
        if let Some(kind) = sniff_text(&text) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: Some(format!("{} export", kind.id())),
            });
        }
        if has_extension(path, &TEXT_EXTENSIONS) && vendors::generic::sniff(&sheet::text_book(head))
        {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Likely,
                note: Some("plate matrix (8×12, 16×24, …) with no known exporter signature".into()),
            });
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(PlateDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(PlateDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
