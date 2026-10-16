//! Clean-room readers for gel and blot documentation images.
//!
//! - **Bio-Rad Image Lab** (`biorad-scn`, `.scn`): the image files ChemiDoc, Gel Doc and other
//!   imagers save through Image Lab (also scans imported into Image Lab): one 16-bit image per
//!   scan with its pixel size, imager, application (chemiluminescence, IRDye, colorimetric,
//!   stains), exposure, filters, user, dates and the audit log. Notes:
//!   `docs/formats/biorad-scn.md`.
//! - **Syngene GeneSys** (`syngene-sgd`, `.sgd`): the compound files G:Box and PXi imagers save
//!   through GeneSys: one 8- or 16-bit image per capture (every exposure of a series and the
//!   marker image) with the imager, camera, light, filter, application, exposure, time and user.
//!   Notes: `docs/formats/syngene-sgd.md`.
//!
//! # Example
//!
//! ```
//! use openreadout_core::FormatReader;
//! use openreadout_gel::ImageLabReader;
//!
//! let format = ImageLabReader.descriptor();
//! assert_eq!(format.id, openreadout_gel::IMAGE_LAB_FORMAT_ID);
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::{FormatReader, PlaneIndex};
//! use openreadout_gel::ImageLabReader;
//!
//! let mut dataset = ImageLabReader.open(Path::new("blot.scn"))?;
//! let info = dataset.info()?;
//! let plane = dataset.read_plane(0, PlaneIndex::default())?; // 16-bit little-endian
//! println!("{} × {}", info.images[0].size_x, info.images[0].size_y);
//! # Ok::<(), openreadout_core::Error>(())
//! ```
//!
//! # API stability
//!
//! The supported API is what this page documents: the readers, their format ids and
//! [`ScnDataset`] and [`SgdDataset`] (returned boxed by `open`).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod assurance;
mod mime;
mod scn;
mod sgd;

use std::path::Path;

use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;
use openreadout_core::{Error, Result};

pub use scn::ScnDataset;
pub use sgd::SgdDataset;

/// Format id of Bio-Rad Image Lab `.scn` files.
pub const IMAGE_LAB_FORMAT_ID: &str = scn::FORMAT_ID;

/// Format id of Syngene GeneSys `.sgd` files.
pub const SYNGENE_FORMAT_ID: &str = sgd::FORMAT_ID;

/// The Bio-Rad Image Lab `.scn` reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct ImageLabReader;

impl FormatReader for ImageLabReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::BIORAD_SCN)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: IMAGE_LAB_FORMAT_ID.into(),
            name: "Bio-Rad Image Lab (.scn)".into(),
            vendor: "Bio-Rad".into(),
            extensions: vec!["scn".into()],
            family: "gel-imaging".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::BIORAD_SCN.confidence,
            known_gaps: vec![
                "Validated on 13 development files from Image Lab 3.0.1 to 6.1.0 (ChemiDoc MP, ChemiDoc XRS+, Gel Doc XR+, a merged image); all hold one 16-bit scan. Scans imported from Typhoon/Molecular Dynamics `.gel` files carry a square-root encoding (`scaler/moldyn`) that is reported, not applied, and no corpus file exercises it".into(),
                "Lane and band analyses, molecular-weight calibrations and volume tools are not stored in any file seen; other parts are listed by `info --view structure` and kept in the vendor tree".into(),
                "Multi-channel (fluorescent multiplex) files have no public sample; each ScanImageTag would be its own image".into(),
                "Scans imported from Molecular Dynamics .gel keep their stored square-root-encoded counts (the scale is reported, not applied)".into(),
                "Leica SCN whole-slide files share the extension: they are TIFF and go to the TIFF reader".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        let start = head.get(..head.len().min(512))?;
        let text = String::from_utf8_lossy(start);
        if text.starts_with("MIME-Version:") && text.contains("Image Lab") {
            return Some(Detection {
                format_id: IMAGE_LAB_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        (text.starts_with("MIME-Version:") && has_extension(path, &["scn"])).then_some(Detection {
            format_id: IMAGE_LAB_FORMAT_ID,
            confidence: DetectConfidence::Likely,
            note: Some(
                "a MIME document with the .scn extension that does not name Image Lab".into(),
            ),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let file = input.open()?;
        let len = file.size().map_err(|e| Error::io(input.path(), e))?;
        Ok(Box::new(ScnDataset::open(
            self.descriptor(),
            input.path().to_path_buf(),
            file,
            len,
        )?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// The Syngene GeneSys `.sgd` reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct SyngeneReader;

/// Whether a compound file holds GeneSys captures: the capture list, or a capture's pixels.
fn is_genesys(input: &Input) -> bool {
    let Ok(mut f) = input.open() else {
        return false;
    };
    openreadout_core::cfb::Cfb::open(&mut f, input.path(), SYNGENE_FORMAT_ID).is_ok_and(|c| {
        c.entries.iter().any(|e| {
            e.is_stream
                && (e.path.eq_ignore_ascii_case("Detector/MultiImage/MultiData")
                    || (e.path.starts_with("Detector/GeneSnap/") && e.path.ends_with("/PixelData")))
        })
    })
}

impl FormatReader for SyngeneReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::SYNGENE_SGD)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: SYNGENE_FORMAT_ID.into(),
            name: "Syngene GeneSys (.sgd)".into(),
            vendor: "Syngene".into(),
            extensions: vec!["sgd".into()],
            family: "gel-imaging".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::SYNGENE_SGD.confidence,
            known_gaps: vec![
                "Validated on two development files (GeneSys 1.5.6.0 on a PXi 6, 1.6.1.0 on a G:Box mini 6); no independent reader decodes the samples, so pixel values are checked only through the correlation with one depositor's TIFF export".into(),
                "The exposure is reported in seconds, an inference from how the signal grows across one file's exposure series".into(),
                "Annotations, band and spot-blot analyses, the view and rotation settings and the thumbnail are listed by `info --view structure`, not decoded".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        (has_extension(path, &["sgd"]) && head.starts_with(&openreadout_core::cfb::CFB_MAGIC))
            .then_some(Detection {
                format_id: SYNGENE_FORMAT_ID,
                confidence: DetectConfidence::Likely,
                note: Some("compound file with the .sgd extension".into()),
            })
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        if head.starts_with(&openreadout_core::cfb::CFB_MAGIC) && is_genesys(input) {
            return Some(Detection {
                format_id: SYNGENE_FORMAT_ID,
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
        let file = input.open()?;
        let len = file.size().map_err(|e| Error::io(input.path(), e))?;
        Ok(Box::new(SgdDataset::open(
            self.descriptor(),
            input.path().to_path_buf(),
            file,
            len,
        )?))
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
