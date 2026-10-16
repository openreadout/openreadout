//! DICOM reader (NEMA PS3, an open standard): single files, series and study folders, DICOMDIR
//! media, classic and enhanced multi-frame images, and whole-slide microscopy pyramids.
//!
//! Format notes and vocabulary: `docs/formats/dicom.md`. Provenance: `docs/provenance/dicom.md`.

mod dataset;
mod dictionary;
mod dictionary_table;
mod element;
mod file;
mod parse;
mod phi;
mod pixels;
mod series;
mod tags;
mod values;
mod vendor;
mod wsi;

use std::path::{Path, PathBuf};

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::{Fs, Input};

use dataset::DicomDataset;

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "dicom";

/// Most files examined when deciding whether a folder is DICOM.
const SNIFF_FILES: usize = 24;

/// The DICOM reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct DicomReader;

fn is_dicomdir_name(p: &Path) -> bool {
    p.file_name()
        .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("DICOMDIR"))
}

/// The first bytes of `p` (up to 132).
fn head_of(fs: &Fs, p: &Path) -> Option<Vec<u8>> {
    let f = fs.open(p).ok()?;
    let n = f.size().ok()?.min(256);
    let mut buf = vec![0u8; usize::try_from(n).ok()?];
    f.read_exact_at(0, &mut buf).ok()?;
    Some(buf)
}

/// Is this directory a DICOM folder? A DICOMDIR in it, or DICOM files among the first files
/// found (three levels deep at most).
fn sniff_dir(fs: &Fs, dir: &Path) -> Option<DetectConfidence> {
    let mut stack = vec![(dir.to_path_buf(), 0u32)];
    let mut seen = 0usize;
    let mut dicom = 0usize;
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = fs.read_dir(&d) else { continue };
        let mut entries: Vec<_> = rd.filter_map(std::result::Result::ok).collect();
        entries.sort_by_key(openreadout_core::source::DirEntry::file_name);
        for e in entries {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let is_dir = e.metadata().is_ok_and(|m| m.is_dir());
            if is_dir {
                if depth < 3 {
                    stack.push((p, depth + 1));
                }
                continue;
            }
            if depth == 0 && is_dicomdir_name(&p) {
                return Some(DetectConfidence::Definite);
            }
            seen += 1;
            if head_of(fs, &p).is_some_and(|h| parse::has_magic(&h)) {
                dicom += 1;
            }
            if seen >= SNIFF_FILES {
                break;
            }
        }
        if seen >= SNIFF_FILES {
            break;
        }
    }
    (dicom > 0 && dicom * 2 >= seen).then_some(DetectConfidence::Likely)
}

impl FormatReader for DicomReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::DICOM)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "DICOM".into(),
            vendor: "NEMA open standard (CT, MR, PET, ultrasound, X-ray, whole-slide microscopy)".into(),
            extensions: ["dcm", "dicom", "ima"].map(String::from).to_vec(),
            family: "medical-imaging".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::DICOM.confidence,
            known_gaps: vec![
                "Video transfer syntaxes (MPEG-2, MPEG-4, HEVC) are not decoded".into(),
                "Colour JPEG-LS and segmented palette lookup tables are not decoded".into(),
                "Whole-slide images: only the first focal plane and optical path are read".into(),
                "Structured reports, RT structure sets and plans, presentation states and other objects without pixel data are listed, not interpreted".into(),
                "VOI LUT sequences and presentation states are reported, not applied; previews use the window centre and width".into(),
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
        if input.fs().is_dir(path) {
            return match sniff_dir(input.fs(), path)? {
                DetectConfidence::Definite => {
                    det(DetectConfidence::Definite, Some("folder with a DICOMDIR"))
                }
                c => det(c, Some("folder of DICOM files")),
            };
        }
        if parse::has_magic(head) {
            return det(DetectConfidence::Definite, None);
        }
        if parse::guess_raw(head).is_some() {
            let named = has_extension(path, &["dcm", "dicom", "ima"])
                || path.extension().is_none()
                || is_dicomdir_name(path);
            return if named {
                det(
                    DetectConfidence::Likely,
                    Some("DICOM data set without the 128-byte preamble and 'DICM'"),
                )
            } else {
                None
            };
        }
        if has_extension(path, &["dcm", "dicom"]) {
            return det(
                DetectConfidence::ExtensionOnly,
                Some("DICOM extension, but the file does not start like DICOM"),
            );
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(DicomDataset::open(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// The files of a folder that start like DICOM, depth-first in name order (at most
/// `max`), skipping hidden files.
pub(crate) fn dicom_files_in(fs: &Fs, dir: &Path, max: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0u32)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = fs.read_dir(&d) else { continue };
        let mut entries: Vec<_> = rd.filter_map(std::result::Result::ok).collect();
        entries.sort_by_key(openreadout_core::source::DirEntry::file_name);
        let mut subdirs = Vec::new();
        for e in entries {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || is_dicomdir_name(&p) {
                continue;
            }
            if e.metadata().is_ok_and(|m| m.is_dir()) {
                if depth < 8 {
                    subdirs.push(p);
                }
                continue;
            }
            if head_of(fs, &p).is_some_and(|h| file::looks_like_dicom(&h)) {
                out.push(p);
                if out.len() >= max {
                    return out;
                }
            }
        }
        for s in subdirs.into_iter().rev() {
            stack.push((s, depth + 1));
        }
    }
    out
}
