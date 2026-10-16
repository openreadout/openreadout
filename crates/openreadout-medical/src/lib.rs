//! Clean-room readers for medical and preclinical imaging, registered as separate formats:
//!
//! - [`DicomReader`]: DICOM (NEMA PS3, an open standard), format id `dicom`: single files,
//!   series and study folders, DICOMDIR media, enhanced multi-frame images and whole-slide
//!   microscopy pyramids;
//! - [`NiftiReader`]: NIfTI-1, NIfTI-2 and Analyze 7.5 (`.nii`, `.nii.gz`, `.hdr`/`.img`),
//!   format id `nifti`;
//! - [`ParavisionReader`]: Bruker ParaVision MRI (`2dseq` with `visu_pars`), format id
//!   `bruker-paravision`.
//!
//! Vocabulary: `docs/formats/{dicom,nifti,bruker-paravision}.md`. Provenance:
//! `docs/provenance/{dicom,nifti,bruker-paravision}.md`.
//!
//! # Example
//!
//! ```
//! use openreadout_core::FormatReader;
//! use openreadout_medical::DicomReader;
//!
//! let format = DicomReader.descriptor();
//! assert_eq!(format.id, "dicom");
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a CT series folder:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::{FormatReader, PlaneIndex};
//! use openreadout_medical::DicomReader;
//!
//! let mut dataset = DicomReader.open(Path::new("CT_series/"))?;
//! let info = dataset.info()?; // headers only
//! let ct = &info.images[0];
//! println!("{} slices of {} x {} px", ct.size_z, ct.size_x, ct.size_y);
//! // Planes have the modality rescale applied: CT values are Hounsfield units.
//! let slice = dataset.read_plane(0, PlaneIndex { c: 0, z: ct.size_z / 2, t: 0 })?;
//! assert_eq!(slice.data.len(), slice.expected_len());
//! # Ok::<(), openreadout_core::Error>(())
//! ```
//!
//! # API stability
//!
//! The supported API is what this page documents.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod assurance;
mod dicom;
mod nifti;
mod paravision;
mod util;

pub use dicom::DicomReader;
pub use nifti::NiftiReader;
pub use nifti::write::{NiftiExportOptions, NiftiExportReport, default_nifti_output, export_nifti};
pub use paravision::ParavisionReader;
