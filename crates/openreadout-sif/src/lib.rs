//! Clean-room reader for Andor SIF files (Andor Solis: iDus, Newton, iXon and other cameras on
//! spectrographs and microscopes).
//!
//! A SIF file holds a signal block and, when they were recorded, reference, background, live and
//! source blocks. Each block is frames of float32 values from one detector area (one or more
//! sub-images with a binning). A block whose frames are single rows (full vertical binning, the
//! usual spectrum) is a trace with one sweep per frame and, when the file holds a wavelength
//! calibration, the calibrated axis as its first channel; any other block is an image whose frames
//! are time points.
//!
//! Derived from hex dumps of corpus files and the documentation of `sif_parser` (BSD-3-Clause,
//! Keisuke Fujii); `sif_parser` and `sifreader` (MIT) are run as black-box oracles. Layout and
//! vocabulary: `docs/formats/andor-sif.md`. Provenance: `docs/provenance/andor-sif.md`.
//!
//! # Example
//!
//! ```
//! use openreadout_core::FormatReader;
//! use openreadout_sif::SifReader;
//!
//! let format = SifReader.descriptor();
//! assert_eq!(format.id, openreadout_sif::FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a spectrum:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::FormatReader;
//! use openreadout_sif::SifReader;
//!
//! let mut dataset = SifReader.open(Path::new("raman.sif"))?;
//! let info = dataset.info()?; // headers only
//! let spectrum = dataset.read_trace(0, 0, 0, u64::MAX)?;
//! // channel 0: wavelength (nm) when the file is calibrated, then the counts
//! println!("{} points", spectrum.channels[0].len());
//! # Ok::<(), openreadout_core::Error>(())
//! ```
//!
//! # API stability
//!
//! The supported API is what this page documents. The parser modules are public only so that
//! tests and fuzz targets can reach them: they are hidden from this documentation and may change
//! in any release.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod assurance;
#[doc(hidden)]
pub mod dataset;
#[doc(hidden)]
pub mod layout;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader};

#[doc(hidden)]
pub use dataset::SifDataset;
#[doc(hidden)]
pub use layout::{SIF_MAGIC, looks_like_sif};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "andor-sif";

/// The Andor SIF reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct SifReader;

impl FormatReader for SifReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::ANDOR_SIF)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "Andor SIF".into(),
            vendor: "Andor Technology (Oxford Instruments)".into(),
            extensions: vec!["sif".into()],
            family: "spectroscopy".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::ANDOR_SIF.confidence,
            known_gaps: vec![
                "Derived without a specification from corpus files of SIF versions 65555, 65559, 65564 and 65567".into(),
                "Blocks after the signal are named by their position (reference, background, live, source); which data each one holds is not confirmed".into(),
                "The calibration polynomial is applied to the 1-based column number of the frame, as sif_parser does; for a sub-image that does not start at the first detector column the axis may be offset".into(),
                "Spooled acquisitions (.sifx with .dat files) are not read".into(),
                "Most settings on the header lines are listed as found, not named".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], _path: &Path) -> Option<Detection> {
        looks_like_sif(head).then_some(Detection {
            format_id: FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(SifDataset::open(path)?))
    }

    fn open_input(&self, input: &openreadout_core::source::Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(SifDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
