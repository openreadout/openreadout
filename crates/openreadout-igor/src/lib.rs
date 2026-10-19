//! Clean-room reader for WaveMetrics Igor Pro files: binary waves (`.ibw`) and packed
//! experiments (`.pxp`, `.pxt`).
//!
//! An Igor wave is an array of up to four dimensions with a name, units, a scaling per dimension
//! and an optional note. A binary wave file holds one wave; a packed experiment holds many, in
//! data folders, with variables, procedures and the command history. Every numeric wave becomes
//! a trace (one dimension, or two dimensions whose columns carry labels: one channel per column)
//! or an image (two to four dimensions: rows are X, columns Y, labelled layers channels and other
//! layers Z, chunks time points). Text waves, variables, notes and procedures are vendor
//! metadata.
//!
//! Derived from WaveMetrics' public technical notes TN003 (Igor binary file format) and PTN003
//! (packed files) and hex dumps of corpus files; `igor2` (LGPL) is run only as a black-box
//! oracle. Layout and vocabulary: `docs/formats/igor.md`. Provenance: `docs/provenance/igor.md`.
//!
//! # Example
//!
//! ```
//! use openreadout_core::FormatReader;
//! use openreadout_igor::IgorReader;
//!
//! let format = IgorReader.descriptor();
//! assert_eq!(format.id, openreadout_igor::FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::FormatReader;
//! use openreadout_igor::IgorReader;
//!
//! let mut dataset = IgorReader.open(Path::new("experiment.pxp"))?;
//! let info = dataset.info()?; // headers only
//! for t in &info.traces {
//!     println!("{}: {} points", t.name.as_deref().unwrap_or("?"), t.sample_count);
//! }
//! let first = dataset.read_trace(0, 0, 0, 100)?;
//! println!("{} values", first.channels[0].len());
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
pub mod packed;
#[doc(hidden)]
pub mod wave;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};

#[doc(hidden)]
pub use dataset::IgorDataset;

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "igor";

/// True when `head` starts with an Igor binary wave whose header checksum is zero.
pub fn looks_like_wave(head: &[u8]) -> bool {
    let Some((e, v)) = wave::wave_endian(head) else {
        return false;
    };
    let n = usize::try_from(wave::checksum_len(v)).unwrap_or(usize::MAX);
    head.get(..n).is_some_and(|b| wave::checksum_zero(b, e))
        && head.get(2..n).is_some_and(|b| b.iter().any(|x| *x != 0))
}

/// The Igor Pro reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct IgorReader;

impl FormatReader for IgorReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::IGOR)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "Igor Pro binary wave / packed experiment".into(),
            vendor: "WaveMetrics (Igor Pro)".into(),
            extensions: vec!["ibw".into(), "bwav".into(), "pxp".into(), "pxt".into()],
            family: "data-analysis".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::IGOR.confidence,
            known_gaps: vec![
                "Each wave is one trace or one image: how an instrument's software grouped waves (sweeps of one recording, channels of one image) is not reconstructed".into(),
                "16-, 32- and 64-bit integer, unsigned and complex waves follow the published type bits; no corpus file holds one yet".into(),
                "Version-1 and version-3 binary waves follow the published sizes; no corpus file has one".into(),
                "Dependent variables, packed notebooks, procedure files and window recreation are not decoded".into(),
                "HDF5 packed experiments (.h5xp) are HDF5 files, not this format".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        let known = |c| Detection {
            format_id: FORMAT_ID,
            confidence: c,
            note: None,
        };
        let wave_ext = has_extension(path, &["ibw", "bwav"]);
        let packed_ext = has_extension(path, &["pxp", "pxt"]);
        if looks_like_wave(head) {
            return Some(known(if wave_ext {
                DetectConfidence::Definite
            } else {
                DetectConfidence::Likely
            }));
        }
        // the head is the whole file when it is shorter than the sniff window
        let file_len = if head.len() < 64 * 1024 {
            head.len() as u64
        } else {
            u64::MAX / 2
        };
        if packed::looks_like_packed(head, file_len) {
            return Some(known(if packed_ext {
                DetectConfidence::Definite
            } else {
                DetectConfidence::Likely
            }));
        }
        (wave_ext || packed_ext).then(|| Detection {
            format_id: FORMAT_ID,
            confidence: DetectConfidence::ExtensionOnly,
            note: Some(
                "Igor extension, but no binary-wave header with a valid checksum and no packed-experiment record chain"
                    .into(),
            ),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(IgorDataset::open(path)?))
    }

    fn open_input(&self, input: &openreadout_core::source::Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(IgorDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
