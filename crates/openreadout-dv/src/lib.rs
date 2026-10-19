//! Clean-room reader for DeltaVision files (`.dv`, `.r3d`): the images of GE / Applied
//! Precision DeltaVision and OMX microscopes and of the softWoRx deconvolution software.
//!
//! A DeltaVision file is an MRC-like 1024-byte header marked by an identifier at byte 96, an
//! extended header with per-section values (time stamp, stage position, exposure, excitation and
//! emission wavelength), then the sections: wavelengths × Z planes × time points in one of three
//! orders. The file is one image.
//!
//! Derived from hex dumps of corpus files and the documentation of the `mrc` Python package
//! (BSD-3-Clause, Sebastian Haase and Talley Lambert); `mrc` and Bio-Formats (GPL) are run only
//! as black-box oracles. Layout and vocabulary: `docs/formats/deltavision.md`. Provenance:
//! `docs/provenance/deltavision.md`.
//!
//! # Example
//!
//! ```
//! use openreadout_core::FormatReader;
//! use openreadout_dv::DvReader;
//!
//! let format = DvReader.descriptor();
//! assert_eq!(format.id, openreadout_dv::FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a plane:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::{FormatReader, PlaneIndex};
//! use openreadout_dv::DvReader;
//!
//! let mut dataset = DvReader.open(Path::new("cells_R3D_D3D.dv"))?;
//! let info = dataset.info()?; // headers only
//! let image = &info.images[0];
//! println!("{} x {} px, {} Z, {} channels", image.size_x, image.size_y, image.size_z, image.size_c);
//! let plane = dataset.read_plane(0, PlaneIndex { c: 1, z: 10, t: 0 })?;
//! assert_eq!(plane.data.len(), plane.expected_len());
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
pub mod header;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader};

#[doc(hidden)]
pub use dataset::DvDataset;
#[doc(hidden)]
pub use header::{DvHeader, looks_like_dv};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "deltavision";

/// The DeltaVision reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct DvReader;

impl FormatReader for DvReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::DELTAVISION)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "DeltaVision".into(),
            vendor: "GE Healthcare / Applied Precision (DeltaVision, softWoRx)".into(),
            extensions: vec!["dv".into(), "r3d".into()],
            family: "microscopy".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::DELTAVISION.confidence,
            known_gaps: vec![
                "The lens id is reported as a number; the objective's name and numerical aperture are not looked up".into(),
                "The `.log` and `_log.txt` files softWoRx writes next to a file are not read (acquisition date, objective, filters)".into(),
                "Multi-panel (tiled) acquisitions are read as one image of the stored sections".into(),
                "Pixel mode 3 (complex 16-bit) is refused".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], _path: &Path) -> Option<Detection> {
        let h = DvHeader::parse(head)?;
        let plausible = h.width > 0 && h.height > 0 && h.sections > 0 && (0..=7).contains(&h.mode);
        plausible.then_some(Detection {
            format_id: FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(DvDataset::open(path)?))
    }

    fn open_input(&self, input: &openreadout_core::source::Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(DvDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
