//! One DICOM file: its meta group, transfer syntax, data set (up to the pixel data) and where
//! the pixel data is.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use openreadout_core::source::Fs;
use openreadout_core::{Error, Result};

use super::FORMAT_ID;
use super::dictionary::{Syntax, syntax};
use super::element::Lookup;
use super::parse::{self, Bytes, Element, Encoding, Item, Value};
use super::tags as t;

/// Largest inflated data set of a deflated file (transfer syntax 1.2.840.10008.1.2.1.99).
const MAX_INFLATED: usize = 2 << 30;

/// Where the pixel data of a file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PixelLoc {
    /// Native samples: `len` bytes at `offset`.
    Native { offset: u64, len: u64 },
    /// Encapsulated fragments starting with the Basic Offset Table item at `offset`.
    Encapsulated { offset: u64 },
}

/// A parsed DICOM file.
#[derive(Debug, Clone)]
pub(crate) struct DicomFile {
    pub(crate) path: PathBuf,
    /// The bytes elements and pixel offsets refer to (the inflated data set of a deflated
    /// file, otherwise the file).
    pub(crate) src: Bytes,
    pub(crate) file_len: u64,
    pub(crate) syntax: Syntax,
    /// The transfer syntax UID as written (empty without a meta group).
    pub(crate) syntax_uid: String,
    pub(crate) meta: Item,
    pub(crate) ds: Item,
    pub(crate) pixels: Option<PixelLoc>,
    /// The pixel data element's tag (7FE0,0010, or float / double float pixel data).
    pub(crate) pixel_tag: u32,
    /// The pixel data element's VR (OB or OW).
    pub(crate) pixel_vr: [u8; 2],
    pub(crate) preamble: bool,
    /// Problems found while parsing (tolerated).
    pub(crate) problems: Vec<String>,
}

impl DicomFile {
    /// Parse `path` up to its pixel data.
    pub(crate) fn open(fs: &Fs, path: &Path) -> Result<Self> {
        let file = fs.open(path).map_err(|e| Error::io(path, e))?;
        let file_len = file.metadata().map_err(|e| Error::io(path, e))?.len();
        let src = Bytes::File(file, file_len);
        Self::parse(path, src, file_len)
    }

    fn parse(path: &Path, src: Bytes, file_len: u64) -> Result<Self> {
        let head = parse::read_head(&src)?;
        let mut problems = Vec::new();
        let (meta, start, enc_guess) = if head.preamble {
            (head.meta, head.start, None)
        } else {
            let first = src.read(0, src.len().min(64))?;
            let enc = parse::guess_raw(&first).ok_or_else(|| {
                Error::corrupt_at(
                    FORMAT_ID,
                    0,
                    "neither 'DICM' at byte 128 nor a data element at the start of the file",
                )
            })?;
            problems.push("no 128-byte preamble and 'DICM' prefix (PS3.10 §7.1)".into());
            (Vec::new(), 0, Some(enc))
        };
        let syntax_uid = meta.text(t::TRANSFER_SYNTAX_UID).unwrap_or_default();
        let (ts, enc) = if syntax_uid.is_empty() {
            let enc = enc_guess.unwrap_or(Encoding::IMPLICIT_LE);
            if head.preamble {
                problems.push(
                    "the file meta group has no transfer syntax; implicit VR little endian assumed"
                        .into(),
                );
            }
            let uid = match (enc.explicit, enc.little) {
                (false, _) => "1.2.840.10008.1.2",
                (true, true) => "1.2.840.10008.1.2.1",
                (true, false) => "1.2.840.10008.1.2.2",
            };
            (syntax(uid), enc)
        } else {
            let ts = syntax(&syntax_uid);
            (
                ts,
                Encoding {
                    explicit: ts.explicit,
                    little: ts.little,
                },
            )
        };
        let (src, start) = if ts.deflated {
            let raw = src.read(start, src.len() - start.min(src.len()))?;
            let inflated = parse::inflate(&raw, MAX_INFLATED)?;
            (Bytes::Mem(Arc::new(inflated)), 0)
        } else {
            (src, start)
        };
        let mut parser = parse::Parser::new(&src, enc);
        let mut ds = parser.top(start)?;
        problems.append(&mut parser.problems);
        // Files without a meta group sometimes still carry group 0002 elements at the start.
        let mut meta = meta;
        if !head.preamble {
            let (m, rest): (Vec<Element>, Vec<Element>) =
                ds.into_iter().partition(|e| e.tag >> 16 == 0x0002);
            meta = m;
            ds = rest;
        }
        let mut pixels = None;
        let mut pixel_tag = parse::PIXEL_DATA;
        let mut pixel_vr = *b"OB";
        if let Some(e) = ds.iter().rev().find(|e| e.tag >> 16 == 0x7FE0 && e.tag & 0xFFFF >= 8)
        {
            pixel_tag = e.tag;
            pixel_vr = e.vr;
            pixels = match &e.value {
                Value::Deferred { offset, len } => Some(PixelLoc::Native {
                    offset: *offset,
                    len: *len,
                }),
                Value::Bytes(_) => {
                    // small native pixel data was read inline; locate it again
                    let len = e.bytes().map_or(0, |b| b.len() as u64);
                    Some(PixelLoc::Native {
                        offset: parser_end_offset(parser.end, len),
                        len,
                    })
                }
                Value::Encapsulated { offset } => Some(PixelLoc::Encapsulated { offset: *offset }),
                Value::Seq(_) => None,
            };
        }
        Ok(DicomFile {
            path: path.to_path_buf(),
            src,
            file_len,
            syntax: ts,
            syntax_uid,
            meta,
            ds,
            pixels,
            pixel_tag,
            pixel_vr,
            preamble: head.preamble,
            problems,
        })
    }

    /// The SOP class UID (from the data set, else the meta group).
    pub(crate) fn sop_class(&self) -> String {
        self.ds
            .text(t::SOP_CLASS_UID)
            .or_else(|| self.meta.text(t::MEDIA_STORAGE_SOP_CLASS_UID))
            .unwrap_or_default()
    }

    /// Read a value the parser left on disk.
    pub(crate) fn read_value(&self, e: &Element) -> Result<Vec<u8>> {
        match &e.value {
            Value::Bytes(b) => Ok(b.clone()),
            Value::Deferred { offset, len } => {
                if *len > openreadout_core::limits::MAX_METADATA_BYTES {
                    return Err(Error::corrupt(
                        FORMAT_ID,
                        format!("a {len}-byte value is too large to load"),
                    ));
                }
                self.src.read(*offset, *len)
            }
            _ => Err(Error::corrupt(FORMAT_ID, "not a value element")),
        }
    }
}

/// The parser stops right after an inline pixel data value, so its value starts `len` bytes
/// before where parsing ended.
fn parser_end_offset(end: u64, len: u64) -> u64 {
    end.saturating_sub(len)
}

/// Is `head` (first bytes of a file) a DICOM file? Part 10 files have `DICM` at byte 128;
/// files without a preamble must start with a group 0002 or 0008 element.
pub(crate) fn looks_like_dicom(head: &[u8]) -> bool {
    parse::has_magic(head) || parse::guess_raw(head).is_some()
}
