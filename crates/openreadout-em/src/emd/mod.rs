//! EMD (HDF5-based) reader: Thermo Fisher Velox EMD images through the pure-Rust `hdf5-pure`
//! crate. Other EMD flavours (NCEM/Berkeley EMD 0.2) are recognised and exit 6.
//!
//! Layout and vocabulary: `docs/formats/emd.md`. Provenance: `docs/provenance/emd.md`.

mod berkeley;
mod dataset;
#[doc(hidden)]
pub mod spectra;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;

pub use berkeley::{BerkeleyDataset, EmdArray, EmdDim};
pub use dataset::{EmdDataset, VeloxImage};
#[doc(hidden)]
pub use spectra::{EventTable, VeloxSpectrum, VeloxStream};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "emd";

/// The 8-byte HDF5 signature.
pub const HDF5_SIGNATURE: [u8; 8] = [0x89, b'H', b'D', b'F', b'\r', b'\n', 0x1a, b'\n'];

/// HDF5 signature at byte 0 or at a user-block boundary (512, 1024, 2048, …) within `head`.
pub fn looks_like_hdf5(head: &[u8]) -> bool {
    let mut at = 0usize;
    loop {
        match head.get(at..at + 8) {
            Some(s) if s == HDF5_SIGNATURE => return true,
            Some(_) => {}
            None => return false,
        }
        at = if at == 0 { 512 } else { at * 2 };
    }
}

/// The EMD reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct EmdReader;

impl FormatReader for EmdReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::EMD)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "EMD (Velox, HDF5)".into(),
            vendor: "Thermo Fisher Scientific (Velox); HDF5-based".into(),
            extensions: vec!["emd".into()],
            family: "electron-microscopy".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::EMD.confidence,
            known_gaps: vec![
                "Velox images, EDS spectra (traces), EDS spectrum images assembled from uint16 event streams and STEM-EELS spectrum images (Data/EelsSpectrumImage, validated on one dual-EELS file) are read; line profiles, EELS point spectra and the Data/SpectrumImage blob are listed, not decoded; other stream encodings are not read".into(),
                "EDS spectrum images are summed over detector segments and frames (per-segment and per-frame cubes are not exposed); the first plane read decodes every event into memory (at most 400 million events)".into(),
                "Berkeley EMD (0.2 and 1.0) data groups are read as images: X is each array's last dimension, Y the one before, leading dimensions are T; metadata groups are copied, not normalised beyond the microscope model and voltage".into(),
                "Per-frame metadata of multi-frame Velox images is not exposed".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if !has_extension(path, &["emd"]) {
            return None;
        }
        Some(if looks_like_hdf5(head) {
            Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: Some("HDF5 file with the .emd extension".into()),
            }
        } else {
            Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::ExtensionOnly,
                note: Some("EMD extension, but no HDF5 signature".into()),
            }
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        match EmdDataset::open_input(input) {
            Ok(d) => Ok(Box::new(d)),
            // no Velox data: a Berkeley EMD file, or nothing we read
            Err(e @ openreadout_core::Error::Unsupported { .. }) => match berkeley::open(input) {
                Ok(b) => Ok(Box::new(b)),
                Err(b) if e.to_string().contains("Berkeley") => Err(b),
                Err(_) => Err(e),
            },
            Err(e) => Err(e),
        }
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_at_user_block_offsets() {
        let mut b = vec![0u8; 1100];
        assert!(!looks_like_hdf5(&b));
        b[512..520].copy_from_slice(&HDF5_SIGNATURE);
        assert!(looks_like_hdf5(&b));
        assert!(looks_like_hdf5(&HDF5_SIGNATURE));
    }
}
