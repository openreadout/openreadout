//! TIA / ES Vision series files (`.ser`) and their `.emi` sidecars.
//!
//! Layout and vocabulary: `docs/formats/ser.md`. Provenance: `docs/provenance/ser.md`.
//! A `.ser` holds a header, a dimension array (the scan), offset arrays, then data elements
//! (1-D spectra or 2-D images) and per-element tags (time, optionally position).

mod dataset;
mod emi;
mod file;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;

pub use dataset::{SerDataset, SerSeries};
pub use emi::{EmiInfo, series_of, sidecar_of};
pub use file::{
    AxisCalibration, BYTE_ORDER_LE, ELEMENTS_1D, ELEMENTS_2D, ElementHeader, ElementTag, SERIES_ID,
    SerDataType, SerDimension, SerHeader, TAG_TIME, TAG_TIME_POSITION, looks_like_ser,
};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "ser";

/// The TIA series reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct SerReader;

impl FormatReader for SerReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::SER)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "FEI TIA / ES Vision series (SER + EMI)".into(),
            vendor: "FEI / Thermo Fisher Scientific (TIA)".into(),
            extensions: vec!["ser".into(), "emi".into()],
            family: "electron-microscopy".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::SER.confidence,
            known_gaps: vec![
                "Complex element data (types 9, 10) is described but not decoded".into(),
                "Series whose elements differ in size are listed but not read".into(),
                "Pixel-size units are not stored in the .ser; deltas below 1 mm are taken as metres".into(),
                "Only the <ObjectInfo> XML of the .emi is read (display layout and data copies are not)".into(),
                "Line and area scans are exposed as T (elements in file order), not as scan axes".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        let det = |confidence, note: Option<&str>| {
            Some(Detection {
                format_id: FORMAT_ID,
                confidence,
                note: note.map(str::to_string),
            })
        };
        if looks_like_ser(head) {
            return det(DetectConfidence::Definite, None);
        }
        if has_extension(path, &["emi"]) {
            return det(
                DetectConfidence::Likely,
                Some("TIA .emi sidecar: images are read from the <name>_<n>.ser files next to it"),
            );
        }
        if has_extension(path, &["ser"]) {
            return det(
                DetectConfidence::ExtensionOnly,
                Some("SER extension, but the file does not start with the series-file header"),
            );
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(SerDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(SerDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
