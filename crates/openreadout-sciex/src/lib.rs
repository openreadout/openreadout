//! Clean-room reader for legacy Sciex `.wiff` files with their `.wiff.scan` companions: the
//! acquisition method (experiments, MRM transitions with their scheduling windows), sample
//! information, the scan index, MRM intensities and TripleTOF time-of-flight scans with the
//! per-scan calibration and data-dependent precursors. `.wiff2` files are recognised and
//! refused: they are encrypted (see `docs/legal/clean-room-policy.md`).
//!
//! Derived from public MetaboLights data sets compared against the depositors' own
//! conversions. The `.wiff` container is read with `openreadout-core`'s compound-file reader.
//! No vendor library is used, linked or consulted; see `docs/provenance/sciex-wiff.md`.
//!
//! Layout and vocabulary: `docs/formats/sciex-wiff.md`.
//!
//! # Example
//!
//! ```
//! use openreadout_core::FormatReader;
//! use openreadout_sciex::SciexWiffReader;
//!
//! let format = SciexWiffReader.descriptor();
//! assert_eq!(format.id, openreadout_sciex::FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file (the `.wiff.scan` must sit beside the `.wiff`):
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::FormatReader;
//! use openreadout_sciex::SciexWiffReader;
//!
//! let mut dataset = SciexWiffReader.open(Path::new("run.wiff"))?;
//! let info = dataset.info()?; // method, sample and scan index only
//! let run = &info.spectra[0];
//! println!("{} spectra, MS levels {:?}", run.scan_count, run.ms_levels);
//! let spectrum = dataset.read_spectrum(0, 0)?;
//! println!("MS{} at {:?} s: {} points", spectrum.ms_level, spectrum.rt_s, spectrum.mz.len());
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

use openreadout_core::cfb::CFB_MAGIC;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;
use openreadout_core::{Error, Result};

#[doc(hidden)]
pub use dataset::{Experiment, Sample, SciexDataset, wiff_path};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "sciex-wiff";

/// The Sciex `.wiff` reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct SciexWiffReader;

fn is_wiff2(path: &Path) -> bool {
    has_extension(path, &["wiff2"])
}

fn is_scan_companion(path: &Path) -> bool {
    path.file_name().is_some_and(|n| {
        n.to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(".wiff.scan")
    })
}

impl FormatReader for SciexWiffReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::SCIEX_WIFF)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "Sciex WIFF (.wiff + .wiff.scan)".into(),
            vendor: "SCIEX".into(),
            extensions: vec!["wiff".into(), "scan".into(), "wiff2".into()],
            family: "mass-spectrometry".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::SCIEX_WIFF.confidence,
            known_gaps: vec![
                ".wiff2 (SCIEX OS) files are encrypted and are refused (exit 6); convert them with the vendor's software".into(),
                "Validated on MRM files (QTRAP 6500+ scheduled MRM, Analyst 1.7.2: 165 SRM chromatograms bit-exact; QTRAP 6500; a 2007 ten-sample Analyst file with one value per transition per cycle) and LC device traces, and on TripleTOF 6600 and 5600+ data-dependent files (Analyst TF; scan times, MS levels, polarity, precursors, TIC and base-peak m/z against the export); other scan types (Q1/Q3 scans, enhanced product ion, MRM³, SWATH variable windows) are listed but not decoded".into(),
                "TOF spectra are the stored time-to-digital histograms (counts per bin, calibrated per scan); the vendor library's peak picking, which depositor exports hold, is not reproduced".into(),
                "ZenoTOF (SCIEX OS) .wiff + .wiff.scan files open, but their precursor charges and rolling collision energies are not read (reported absent); not validated against an export".into(),
                "TripleTOF 5600 files: the index TIC is not the sum of the stored counts (median +0.6 %, from -64 % on the most intense scans to +38 % on the weakest); every scan's largest count equals the index's base-peak intensity at the index's base-peak m/z, so the counts are returned as stored and `total_ion_current` is the index value".into(),
                "The first half of every 2N-value MRM cycle (zero in the corpus files) is not interpreted; transitions are assigned to cycles by their scheduling windows, and a boundary cycle whose values are all zero is left out of the transitions that start or end there (inferred from one file)".into(),
                "Multi-period methods: only period 0's experiments are described; multi-sample files expose one run per sample but only sample 1's acquisition time".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if is_wiff2(path) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: Some(".wiff2 is encrypted and not readable".into()),
            });
        }
        if is_scan_companion(path) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Likely,
                note: Some("the .wiff.scan companion: the .wiff beside it is opened".into()),
            });
        }
        if !has_extension(path, &["wiff"]) {
            return None;
        }
        let cfb = head.starts_with(&CFB_MAGIC);
        Some(Detection {
            format_id: FORMAT_ID,
            confidence: if cfb {
                DetectConfidence::Definite
            } else {
                DetectConfidence::ExtensionOnly
            },
            note: (!cfb).then(|| ".wiff extension but no compound-file signature".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let path = input.path();
        if is_wiff2(path) {
            return Err(Error::unsupported(
                FORMAT_ID,
                ".wiff2 (SCIEX OS) files",
                "WIFF2 data are encrypted; this project does not circumvent that. Export mzML from SCIEX OS or convert with the vendor's tools, then open the export.",
            ));
        }
        Ok(Box::new(SciexDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
