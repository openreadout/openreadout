//! Clean-room reader for Thermo Fisher `.raw` mass-spectrometry files.
//!
//! Derived from public corpus files (MetaboLights, PRIDE) compared against the depositors' own
//! mzML/mzXML conversions, plus the permissively licensed or public format notes listed in
//! `docs/provenance/thermo-raw.md`. No vendor library is used, linked or consulted.
//!
//! Layout and vocabulary: `docs/formats/thermo-raw.md`.
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
//! use openreadout_thermo::ThermoRawReader;
//!
//! let format = ThermoRawReader.descriptor();
//! assert_eq!(format.id, openreadout_thermo::FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::FormatReader;
//! use openreadout_thermo::ThermoRawReader;
//!
//! let mut dataset = ThermoRawReader.open(Path::new("run.raw"))?;
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
pub mod bytes;
#[doc(hidden)]
pub mod dataset;
#[doc(hidden)]
pub mod detectors;
#[doc(hidden)]
pub mod event;
mod gradient;
#[doc(hidden)]
pub mod layout;
#[doc(hidden)]
pub mod packet;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader};
use openreadout_core::source::Input;

#[doc(hidden)]
pub use bytes::adler32;
#[doc(hidden)]
pub use dataset::{MethodDocument, ScanSummary, ThermoDataset};
#[doc(hidden)]
pub use event::{Activation, Analyzer, Ionization, Polarity};
#[doc(hidden)]
pub use layout::{
    AuditStamp, AutosamplerInfo, CHECKSUM_OFFSET, CHECKSUM_SPAN, ControllerRef, ErrorLogEntry,
    FILE_HEADER_LEN, FileHeader, FileInfoBlock, GenericField, GenericHeader, InjectionRecord,
    InstrumentId, MethodTable, Reaction, RunHeader, SUPPORTED_VERSIONS, ScanEvent, ScanIndexEntry,
    ScanTemplate, SequenceRow, StreamAddresses,
};
#[doc(hidden)]
pub use packet::{
    AcquisitionWindow, Centroid, FLAG_EXCLUDED, MAX_WINDOWS, MONOTONIC_NUDGE, MzScale,
    PROFILE_PAD_BINS, Packet, PacketHeader, PeakDescriptor, Profile, ProfileChunk, WINDOW_LEN,
    WINDOW_PEAK_LEN, WINDOW_RECORD_KIND, WindowRecord,
};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "thermo-raw";

/// First 18 bytes of every `.raw` file: `01 A1` then `Finnigan` in UTF-16LE.
pub const SIGNATURE: [u8; 18] = [
    0x01, 0xA1, b'F', 0, b'i', 0, b'n', 0, b'n', 0, b'i', 0, b'g', 0, b'a', 0, b'n', 0,
];

/// The Thermo `.raw` format reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct ThermoRawReader;

impl FormatReader for ThermoRawReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::THERMO_RAW)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "Thermo RAW".into(),
            vendor: "Thermo Fisher Scientific".into(),
            extensions: vec!["raw".into()],
            family: "mass-spectrometry".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::THERMO_RAW.confidence,
            known_gaps: vec![
                "Validated file versions: 63, 64, 66; other versions open with a note and may misparse".into(),
                "Detector controllers (UV/DAD channels, PDA field, analog pressure/temperature/A-D) are traces. An Accela PDA file (v63) is validated against an independent conversion (GNPS/MassIVE msconvert mzML of MTBLS773): all 10,501 PDA spectra × 401 wavelengths and their times exact, channel A equal to its `UV 1` chromatogram (that conversion times it one 0.1 s sample later; our grid lines up with the PDA field). The Vanquish DAD/CAD (v66) and the analog channels are validated by internal consistency only (DAD channels equal the PDA field at their wavelength; stored per-scan values equal the samples). Units are inferred (mAU; no unit is stored) or taken from the method text; file versions before 63 expose only the mass spectrometer".into(),
                "SRM scans (TSQ window records) are validated against the depositor's chromatograms, not against spectra; their per-peak flag bytes are not interpreted, and full-scan quadrupole (Q1MS/Q3MS) scans have no corpus file".into(),
                "Scan filter text covers analyzer, polarity, data kind, ionization, in-source CID (sid=), turbo scan rate (t), dependent flag, multiplexing (msx), scan type, MS level, precursors with activation (CID, HCD, ETD) and energy, SPS MS3, and scan ranges (SRM product windows included); multi-range SIM scans list every range; rarer filter tokens (supplemental activation, FAIMS) are not composed".into(),
                "Ion-trap (m/z-grid) profiles are validated against vendor-library conversions (LTQ Velos, Orbitrap ion-trap MS2/MS3 of ProteoWizard's test data); four-coefficient ion-cyclotron profiles are decoded from prior-art notes but have no corpus file to validate against".into(),
                "The embedded instrument-method container is read for its stream list and the per-device method text; binary method data and the tune block are not decoded".into(),
                "Centroid lists keep the peaks the file flags as reference/background ions, as conversions with the current vendor library do; conversions made before October 2020 (ProteoWizard up to 3.0.20239) left them out, so their peak counts are lower".into(),
                "Orbitrap Exploris packets end in a per-peak annotation block (isotope-cluster marks, inferred) that is skipped; Orbitrap Astral and Ascend files have no corpus file".into(),
                "Precursor charge is what the file records (`Charge State`); ion-trap (LTQ) and triple-quadrupole SRM scans record none, so none is reported".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], _path: &Path) -> Option<Detection> {
        layout::has_signature(head).then_some(Detection {
            format_id: FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(ThermoDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(ThermoDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
