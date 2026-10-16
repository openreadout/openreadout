//! NIfTI-1, NIfTI-2 and Analyze 7.5 reader (`.nii`, `.nii.gz`, `.hdr`/`.img` pairs).
//!
//! Format notes and vocabulary: `docs/formats/nifti.md`. Provenance: `docs/provenance/nifti.md`.

mod dataset;
mod header;
pub(crate) mod write;

use std::path::{Path, PathBuf};

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader};
use openreadout_core::source::{Fs, Input};

use dataset::NiftiDataset;

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "nifti";

/// The NIfTI / Analyze reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct NiftiReader;

fn lower_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// The header and image files of a pair from either member's path: `x.hdr` ↔ `x.img`
/// (and `.hdr.gz` ↔ `.img.gz`), matched by the case the folder uses.
pub(crate) fn pair_of(fs: &Fs, path: &Path) -> Option<(PathBuf, PathBuf)> {
    let name = path.file_name()?.to_string_lossy().to_string();
    let lower = name.to_ascii_lowercase();
    let dir = path.parent().unwrap_or(Path::new(""));
    let swap = |from: &str, to: &str| -> Option<PathBuf> {
        let stem = &name[..name.len() - from.len()];
        let want = format!("{stem}{to}");
        fs.find_in_dir(dir, &want)
            .or_else(|| fs.find_in_dir(dir, &format!("{stem}{}", to.to_ascii_uppercase())))
    };
    for (h, i) in [(".hdr.gz", ".img.gz"), (".hdr", ".img")] {
        if lower.ends_with(h) {
            return Some((path.to_path_buf(), swap(h, i)?));
        }
        if lower.ends_with(i) {
            return Some((swap(i, h)?, path.to_path_buf()));
        }
    }
    None
}

impl FormatReader for NiftiReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::NIFTI)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "NIfTI-1 / NIfTI-2 / Analyze 7.5".into(),
            vendor: "NIfTI Data Format Working Group open standard (neuroimaging); Analyze 7.5 (Mayo Clinic)".into(),
            extensions: ["nii", "hdr", "img"].map(String::from).to_vec(),
            family: "medical-imaging".into(),
            can_read: true,
            can_write: true,
            confidence: crate::assurance::NIFTI.confidence,
            known_gaps: vec![
                "binary (1-bit), float128 and complex256 data types are not decoded".into(),
                "dimensions 6 and 7 are folded into T".into(),
                "header extensions are listed, not decoded (AFNI, XCEDE, DICOM extensions)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        self.sniff_input(head, &Input::local(path))
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        let det = |confidence, note: Option<&str>| {
            Some(Detection {
                format_id: FORMAT_ID,
                confidence,
                note: note.map(str::to_string),
            })
        };
        let path = input.path();
        let name = lower_name(path);
        let gz = openreadout_core::gzip::is_gzip(head);
        let inner;
        let h: &[u8] = if gz {
            inner = openreadout_core::gzip::peek(head, 544)?;
            &inner
        } else {
            head
        };
        if name.ends_with(".img") || name.ends_with(".img.gz") {
            // the data half of a pair: recognised through its header
            let (hdr, _) = pair_of(input.fs(), path)?;
            let mut buf = vec![0u8; 600];
            let f = input.fs().open(&hdr).ok()?;
            let n = f.read_at(0, &mut buf).ok()?;
            buf.truncate(n);
            let hb = if openreadout_core::gzip::is_gzip(&buf) {
                openreadout_core::gzip::peek(&buf, 544)?
            } else {
                buf
            };
            return header::probe(&hb).map(|_| Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Likely,
                note: Some("image file of a header/image pair".into()),
            });
        }
        let (kind, _) = header::probe(h)?;
        match kind {
            header::Kind::Nifti1 | header::Kind::Nifti2 => det(DetectConfidence::Definite, None),
            header::Kind::Analyze => {
                let named = name.ends_with(".hdr") || name.ends_with(".hdr.gz");
                if named && pair_of(input.fs(), path).is_some() {
                    det(
                        DetectConfidence::Likely,
                        Some("Analyze 7.5 header (no NIfTI magic) with its .img"),
                    )
                } else {
                    None
                }
            }
        }
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(NiftiDataset::open(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
