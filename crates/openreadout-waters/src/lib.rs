//! Clean-room reader for Waters MassLynx `.raw` directories: full-scan spectra of TOF
//! instruments (SYNAPT, Xevo, Vion) with the m/z calibration applied, MS/MS precursors and
//! lock-spray reference functions, MRM/SIR intensities of triple quadrupoles, per-scan
//! statistics and analog channels.
//!
//! Derived from public MetaboLights and PRIDE acquisitions compared spectrum by spectrum
//! against the depositors' own mzML conversions, and from rainbow's public documentation pages
//! for the 2-, 6- and 8-byte value layouts. No vendor library is used, linked or consulted; see
//! `docs/provenance/waters-raw.md`.
//!
//! Layout and vocabulary: `docs/formats/waters-raw.md`.
//!
//! # Example
//!
//! ```
//! use openreadout_core::FormatReader;
//! use openreadout_waters::WatersRawReader;
//!
//! let format = WatersRawReader.descriptor();
//! assert_eq!(format.id, openreadout_waters::WATERS_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading an acquisition (pass the `.raw` directory itself):
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::FormatReader;
//! use openreadout_waters::WatersRawReader;
//!
//! let mut dataset = WatersRawReader.open(Path::new("run.raw"))?;
//! let info = dataset.info()?; // headers and scan indexes only
//! if let Some(run) = info.spectra.first() {
//!     println!("{} spectra, MS levels {:?}", run.scan_count, run.ms_levels);
//!     let spectrum = dataset.read_spectrum(0, 0)?;
//!     println!("MS{} at {} s: {} peaks", spectrum.ms_level, spectrum.rt_s, spectrum.mz.len());
//! }
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
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;

#[doc(hidden)]
pub use dataset::{
    AnalogChannel, WatersFunction, WatersRawDataset, is_waters_dir, is_waters_dir_in,
};
#[doc(hidden)]
pub use layout::{
    CHRO_HEADER, Calibration, CalibrationKind, FLAG_LOCK_MASS_PEAK, FUNCTION_BLOCK,
    FUNCTION_DAUGHTER, FUNCTION_MRM, FUNCTION_PDA, FUNCTION_TOF_MS, FUNCTION_TOF_MSMS,
    FunctionBlock, IDX_RECORD, IDX_RECORD_WIDE, STAT_SET_MASS, ScanIndex, StatsLayout,
    decode_intensity12, decode_mass27, decode_packed16, decode_packed32, decode_value6,
    decode_value8, decode_value12, idx_record_len_for, parse_extern, parse_functions,
    parse_header_txt, parse_idx, parse_stats_layout, value12_flags,
};

/// Format id of the Waters MassLynx `.raw` reader.
pub const WATERS_ID: &str = "waters-raw";

/// Waters MassLynx `.raw` directories.
#[derive(Debug, Default, Clone, Copy)]
pub struct WatersRawReader;

impl FormatReader for WatersRawReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::WATERS_RAW)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: WATERS_ID.into(),
            name: "Waters MassLynx .raw directory".into(),
            vendor: "Waters".into(),
            extensions: vec!["raw".into()],
            family: "chromatography".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::WATERS_RAW.confidence,
            known_gaps: vec![
                "Spectra are validated point for point (m/z within 0.06 ppm, intensities equal) against vendor-library conversions of 13 acquisitions (SYNAPT G2/G2-Si/XS, Synapt MS, Xevo TQ-MS/G2-XS, ACQUITY SQD; 12-, 8- and 6-byte layouts, centroid and continuum, DDA, MSe, product-ion scans); the index flag 0x100 means values stored calibrated, otherwise the `Cal Function n` polynomial (T0, T1) is applied".into(),
                "Lock-mass (lock-spray) correction is not applied: each reference scan's flagged lock-mass peak is reported (`lock_mass_peak_mz`); files MassLynx already corrected are read as stored".into(),
                "Ion-mobility (HDMS/HDMSe/HDDDA/HD-MRM) and SONAR functions: the drift-resolved bins (_funcNNN.cdt) are not decoded; their stored drift-summed spectra are returned and say so".into(),
                "MRM intensities (4-byte layout) are scaled so each scan sums to the TIC stored in _FUNCnnn.IDX (rainbow reports twice these values) and exposed as tables/traces, not spectra".into(),
                "Photodiode-array functions are a trace of stored absorbance counts (unscaled); binary .EE/.CMP files and the third word of 12-byte values are listed, not decoded".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        self.sniff_input(head, &Input::local(path))
    }

    /// A `.raw` directory is recognised by its files, listed through the input's namespace.
    fn sniff_input(&self, _head: &[u8], input: &Input) -> Option<Detection> {
        let path = input.path();
        if is_waters_dir_in(input.fs(), path) {
            return Some(Detection {
                format_id: WATERS_ID,
                confidence: if has_extension(path, &["raw"]) {
                    DetectConfidence::Definite
                } else {
                    DetectConfidence::Likely
                },
                note: None,
            });
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(WatersRawDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(WatersRawDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
