//! MRC / CCP4 / MAP reader (MRC2014, CCP-EM specification).
//!
//! Layout and vocabulary: `docs/formats/mrc.md`. Provenance: `docs/provenance/mrc.md`.
//! A file is a 1024-byte header, NSYMBT bytes of extended header, then NZ sections of NX × NY
//! samples of the type given by MODE.

mod dataset;
mod ext;
mod header;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;

pub use dataset::MrcDataset;
pub use ext::{FEI1_BLOCK_LEN, FEI2_BLOCK_LEN};
pub use header::{HEADER_LEN, IMOD_STAMP, Layout, Mode, MrcHeader, has_map_id, looks_like_mrc};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "mrc";

/// Extensions MRC-family files are written with.
pub const EXTENSIONS: [&str; 9] = [
    "mrc", "mrcs", "map", "ccp4", "rec", "st", "ali", "preali", "mrcz",
];

/// `<name>.<mrc extension>.gz`.
fn gz_name(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    name.strip_suffix(".gz")
        .is_some_and(|s| EXTENSIONS.iter().any(|e| s.ends_with(&format!(".{e}"))))
}

/// The MRC/CCP4 reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct MrcReader;

impl FormatReader for MrcReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::MRC)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "MRC / CCP4 (MRC2014)".into(),
            vendor: "CCP-EM open standard (cryo-EM, tomography, crystallography software)".into(),
            extensions: ["mrc", "mrcs", "map", "ccp4", "rec", "st", "ali", "preali"]
                .map(String::from)
                .to_vec(),
            family: "electron-microscopy".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::MRC.confidence,
            known_gaps: vec![
                "Complex modes 3 and 4 (Fourier transforms) are returned as stored (NX complex values per row), not expanded to the full transform".into(),
                "MAPC/MAPR/MAPS are reported, not applied: planes are returned in file order".into(),
                "FEI1/FEI2 extended-header fields are decoded from mrcfile's published layout; presence bitmasks are reported raw, not interpreted".into(),
                "HDF5 extended headers (EXTTYP HDF5), .mrcz and bzip2-compressed files are not read; gzip-compressed files (.map.gz, .mrc.gz) are decompressed once at open".into(),
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
        if openreadout_core::gzip::is_gzip(head) {
            let inner = openreadout_core::gzip::peek(head, HEADER_LEN)?;
            let named = gz_name(path);
            return if has_map_id(&inner) && looks_like_mrc(&inner) {
                det(DetectConfidence::Definite, Some("gzip-compressed MRC"))
            } else if named && looks_like_mrc(&inner) {
                det(
                    DetectConfidence::Likely,
                    Some("gzip-compressed MRC-style header"),
                )
            } else {
                None
            };
        }
        if has_map_id(head) && looks_like_mrc(head) {
            return det(DetectConfidence::Definite, None);
        }
        if looks_like_mrc(head) && has_extension(path, &EXTENSIONS) {
            return det(
                DetectConfidence::Likely,
                Some("MRC-style header without the 'MAP' identifier (pre-2000 MRC or CCP4 file)"),
            );
        }
        if has_extension(path, &["mrc", "mrcs", "ccp4"]) {
            return det(
                DetectConfidence::ExtensionOnly,
                Some("MRC extension, but the header does not look like MRC"),
            );
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let gz = input.open().is_ok_and(|f| {
            let mut head = [0u8; 3];
            f.read_exact_at(0, &mut head).is_ok() && openreadout_core::gzip::is_gzip(&head)
        });
        if gz {
            return Ok(Box::new(MrcDataset::open_gzip(input)?));
        }
        Ok(Box::new(MrcDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
