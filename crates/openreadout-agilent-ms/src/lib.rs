//! Clean-room reader for Agilent MassHunter `.d` mass-spectrometry directories (Q-TOF and
//! triple-quadrupole acquisitions).
//!
//! Derived from public MetaboLights data sets compared spectrum by spectrum against the
//! depositors' own mzML conversions, and from the `MSScan.xsd` schema every data directory
//! carries. No vendor library is used, linked or consulted; see
//! `docs/provenance/agilent-masshunter.md`.
//!
//! Layout and vocabulary: `docs/formats/agilent-masshunter.md`.
//!
//! # Example
//!
//! ```
//! use openreadout_core::FormatReader;
//! use openreadout_agilent_ms::AgilentMsReader;
//!
//! let format = AgilentMsReader.descriptor();
//! assert_eq!(format.id, openreadout_agilent_ms::FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a data directory (pass the `.d` directory itself):
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::FormatReader;
//! use openreadout_agilent_ms::AgilentMsReader;
//!
//! let mut dataset = AgilentMsReader.open(Path::new("run.d"))?;
//! let info = dataset.info()?; // the scan index only
//! let run = &info.spectra[0];
//! println!("{} spectra, MS levels {:?}", run.scan_count, run.ms_levels);
//! let spectrum = dataset.read_spectrum(0, 0)?;
//! println!("MS{} at {} s: {} points", spectrum.ms_level, spectrum.rt_s, spectrum.mz.len());
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
use openreadout_core::source::{Fs, Input};

#[doc(hidden)]
pub use dataset::{AgilentDataset, Device, acq_dir, dataset_dir};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "agilent-masshunter";

/// The Agilent MassHunter `.d` reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct AgilentMsReader;

/// True when `dir/AcqData/MSScan.bin` exists (names matched case-insensitively).
fn is_masshunter_dir(fs: &Fs, dir: &Path) -> bool {
    dataset::acq_dir_in(fs, dir).is_some_and(|a| {
        fs.is_file(&a.join("MSScan.bin"))
            || fs.read_dir(&a).is_ok_and(|rd| {
                rd.flatten()
                    .any(|e| e.file_name().eq_ignore_ascii_case("MSScan.bin"))
            })
    })
}

impl FormatReader for AgilentMsReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::AGILENT_MASSHUNTER)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "Agilent MassHunter (.d)".into(),
            vendor: "Agilent Technologies".into(),
            extensions: vec!["d".into()],
            family: "mass-spectrometry".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::AGILENT_MASSHUNTER.confidence,
            known_gaps: vec![
                "Validated on MSScan layouts 5 (6400-series triple quadrupole 2006-2014: MRM, dynamic MRM, full, product-ion, neutral-loss and SIM scans, quadrupole profiles) and 6 (6200 TOF and 6500-series Q-TOF, acquisition software B.05 to B.10), and on the 6560 ion-mobility layout (drift-bin profiles, all-ions frames); other layouts are read through the file's own MSScan.xsd but are unverified".into(),
                "Q-TOF profile spectra (MSProfile.bin, LZF) are checked against the scan index only (counts sum to the recorded TIC; the base-peak bin's m/z equals the recorded base-peak m/z): no export holds them; profiles are returned without the zero runs between peaks".into(),
                "Precursor m/z is each MS/MS scan's own recorded value; ProteoWizard exports report a value averaged over the scans that share a precursor (differences up to 1e-5 relative)".into(),
                "Ion-mobility data: drift-bin spectra only (the frame-summed spectra are not listed); all-ions high-energy spectra carry no precursor or collision energy (the frame method's energy ramp is shown by dump); compressed drift-bin profiles and CCS values are not supported".into(),
                "Precursor-ion scans have not been seen; neutral-loss scans report the loss as extra.neutral_loss_mz, not as a precursor".into(),
                "MSPeriodicActuals.bin, MSActualDefs.xml, the acquisition method's device settings and MSTree2.bin are listed but not decoded; ion-mode codes are reported as numbers".into(),
                "DAD spectra (DAD1.sd/.sp) are listed but not decoded; DAD and LC-module signals (*.cd/*.cg) are exposed as traces".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        self.sniff_input(head, &Input::local(path))
    }

    /// A `.d` is recognised by `AcqData/MSScan.bin`, looked up through the namespace.
    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        let _ = head;
        let fs = input.fs();
        let dir = dataset::dataset_dir_in(fs, input.path())?;
        if !is_masshunter_dir(fs, &dir) {
            return None;
        }
        let named_d = dir.extension().is_some_and(|e| e.eq_ignore_ascii_case("d"));
        Some(Detection {
            format_id: FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: (!named_d)
                .then(|| "directory holds AcqData/MSScan.bin but is not named *.d".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(AgilentDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(AgilentDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
