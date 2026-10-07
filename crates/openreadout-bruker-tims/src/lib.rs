//! Clean-room reader for Bruker timsTOF acquisitions (`.d` directories).
//!
//! A TDF dataset (`analysis.tdf` + `analysis.tdf_bin`) stores frames of trapped-ion-mobility
//! scans; a TSF dataset (`analysis.tsf` + `analysis.tsf_bin`) one line spectrum per frame. The
//! `.tdf`/`.tsf` files are SQLite databases, read here by a small pure-Rust reader (write-ahead
//! logs included); frame blobs are zstd-compressed. Spectra are exposed per MS1 frame (summed
//! over scans), per DDA-PASEF precursor and per DIA-PASEF window. Layout and vocabulary:
//! `docs/formats/bruker-tdf.md`. Provenance: `docs/provenance/bruker-tdf.md`.
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
//! use openreadout_bruker_tims::BrukerTimsReader;
//!
//! let format = BrukerTimsReader.descriptor();
//! assert_eq!(format.id, openreadout_bruker_tims::FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::FormatReader;
//! use openreadout_bruker_tims::BrukerTimsReader;
//!
//! let mut dataset = BrukerTimsReader.open(Path::new("run.d"))?;
//! let info = dataset.info()?; // headers only
//! let run = &info.spectra[0];
//! println!("{} spectra, MS levels {:?}", run.scan_count, run.ms_levels);
//! // Spectrum 0 of run 0: parallel m/z and intensity arrays.
//! let spectrum = dataset.read_spectrum(0, 0)?;
//! println!("MS{} at {:?} s: {} points", spectrum.ms_level, spectrum.rt_s, spectrum.mz.len());
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

mod assurance;
#[doc(hidden)]
pub mod calibration;
#[doc(hidden)]
pub mod dataset;
#[doc(hidden)]
pub mod frame;
#[doc(hidden)]
pub mod sqlite;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader};
use openreadout_core::source::Input;

#[doc(hidden)]
pub use calibration::{MobilityModel, MzCalibrationRow, MzModel, TimsCalibrationRow};
#[doc(hidden)]
pub use dataset::{
    Conversions, FrameRecord, MaldiSpot, PrecursorRecord, Selection, TimsDataset, TimsKind,
    dataset_dir, kind_of,
};
#[doc(hidden)]
pub use frame::{
    FrameError, TimsFrame, decode_tdf_frame, decode_tdf_frame_lzf, decode_tsf_profile,
    decode_tsf_spectrum, group_and_sum, read_blob,
};
#[doc(hidden)]
pub use sqlite::{SqlTable, SqlValue, SqliteDb, SqliteError};

/// Format id on the command line and in JSON.
pub const FORMAT_ID: &str = "bruker-tdf";

/// The timsTOF reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct BrukerTimsReader;

impl FormatReader for BrukerTimsReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::BRUKER_TDF)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "Bruker timsTOF (TDF/TSF .d)".into(),
            vendor: "Bruker".into(),
            extensions: vec!["d".into(), "tdf".into(), "tsf".into()],
            family: "mass-spectrometry".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::BRUKER_TDF.confidence,
            known_gaps: vec![
                "m/z and 1/K0 apply the file's MzCalibration (types 1, 2) and TimsCalibration (type 2) models, validated to <= 0.065 ppm and 5e-13 V*s/cm2 against vendor-library conversions of five files; other model types fall back to the acquisition-range approximation (tens of ppm off) and say so in notes and check".into(),
                "prm-PASEF frames (MsMsType 10) are summed whole, not split per target window (no public prm-PASEF file to validate against)".into(),
                "Spectra are raw sums of TOF bins over scans (no smoothing or centroiding); per-scan mobility arrays are not returned by `spectrum` (library: TimsDataset::read_frame)".into(),
                "TimsCompressionType 1 frames hold more scans than Frames.NumScans; all are read (NumPeaks and SummedIntensities count them; the reference conversion lists only the first NumScans)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        self.sniff_input(head, &Input::local(path))
    }

    /// A `.d` is recognised by its analysis file, looked up through the namespace.
    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        let fs = input.fs();
        let dir = dataset::dataset_dir_in(fs, input.path())?;
        let kind = dataset::kind_of_in(fs, &dir)?;
        let named_d = dir.extension().is_some_and(|e| e.eq_ignore_ascii_case("d"));
        // A directory arrives with an empty head; a file inside the .d with its own head.
        let _ = head;
        Some(Detection {
            format_id: FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: (!named_d).then(|| {
                format!(
                    "directory holds analysis.{} but is not named *.d",
                    if kind == TimsKind::Tdf { "tdf" } else { "tsf" }
                )
            }),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(TimsDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(TimsDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
