//! The DICOM byte stream (PS3.5 §7, PS3.10 §7): the file preamble and meta group, data
//! elements in implicit or explicit VR and either byte order, sequences and items of defined
//! or undefined length, and where the pixel data starts.
//!
//! Parsing stops at the top-level Pixel Data element (7FE0,0010): `info` reads headers only.
//! Values up to [`MAX_INLINE`] bytes are kept; larger ones are recorded by offset and read on
//! demand.

use std::sync::Arc;

use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};

use super::FORMAT_ID;
use super::dictionary;

/// Largest value kept in memory while parsing; larger values are read when needed.
pub(crate) const MAX_INLINE: u64 = 256 << 10;
/// Deepest sequence nesting accepted (real files stay below 10).
const MAX_DEPTH: u32 = 48;
/// Most elements parsed from one file (whole-slide images with per-frame functional groups
/// reach a few million).
const MAX_ELEMENTS: u64 = 16 << 20;

/// Tags used by the parser itself.
pub(crate) const ITEM: u32 = 0xFFFE_E000;
pub(crate) const ITEM_END: u32 = 0xFFFE_E00D;
pub(crate) const SEQ_END: u32 = 0xFFFE_E0DD;
pub(crate) const PIXEL_DATA: u32 = 0x7FE0_0010;
const FLOAT_PIXEL_DATA: u32 = 0x7FE0_0008;
const DOUBLE_PIXEL_DATA: u32 = 0x7FE0_0009;
const UNDEFINED: u32 = 0xFFFF_FFFF;

/// How a data set is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Encoding {
    /// Explicit VR (each element names its VR).
    pub(crate) explicit: bool,
    /// Little-endian numbers.
    pub(crate) little: bool,
}

impl Encoding {
    pub(crate) const IMPLICIT_LE: Encoding = Encoding {
        explicit: false,
        little: true,
    };
    pub(crate) const EXPLICIT_LE: Encoding = Encoding {
        explicit: true,
        little: true,
    };
    pub(crate) const EXPLICIT_BE: Encoding = Encoding {
        explicit: true,
        little: false,
    };
}

/// An element's value.
#[derive(Debug, Clone)]
pub(crate) enum Value {
    /// The value bytes (at most [`MAX_INLINE`]).
    Bytes(Vec<u8>),
    /// A value too large to keep: where it is.
    Deferred {
        /// Offset of the first value byte.
        offset: u64,
        /// Length in bytes.
        len: u64,
    },
    /// A sequence of items.
    Seq(Vec<Item>),
    /// Encapsulated (compressed) pixel data: the offset of its first item (the Basic Offset
    /// Table).
    Encapsulated {
        /// Offset of the first item tag.
        offset: u64,
    },
}

/// One data element.
#[derive(Debug, Clone)]
pub(crate) struct Element {
    /// `(group << 16) | element`.
    pub(crate) tag: u32,
    /// Value representation as written (explicit VR) or from the dictionary (implicit VR).
    pub(crate) vr: [u8; 2],
    /// Byte order of the value's numbers.
    pub(crate) little: bool,
    pub(crate) value: Value,
}

/// A sequence item: its elements in file order.
pub(crate) type Item = Vec<Element>;

/// Random access to the bytes of a data set: a file, or a buffer (a deflated data set
/// inflated at open). Reads go through a small window so that the many short reads of a
/// header cost few positional reads.
#[derive(Debug, Clone)]
pub(crate) enum Bytes {
    File(SourceFile, u64),
    Mem(Arc<Vec<u8>>),
}

impl Bytes {
    pub(crate) fn len(&self) -> u64 {
        match self {
            Bytes::File(_, n) => *n,
            Bytes::Mem(v) => v.len() as u64,
        }
    }

    /// Exactly `n` bytes at `offset`, or a corrupt-file error.
    pub(crate) fn read(&self, offset: u64, n: u64) -> Result<Vec<u8>> {
        let end = offset
            .checked_add(n)
            .filter(|e| *e <= self.len())
            .ok_or_else(|| {
                Error::corrupt_at(
                    FORMAT_ID,
                    offset,
                    format!(
                        "an element needs {n} bytes at offset {offset}, past the end of the data ({} bytes)",
                        self.len()
                    ),
                )
            })?;
        let len = usize::try_from(n).map_err(|_| Error::corrupt(FORMAT_ID, "value too large"))?;
        match self {
            Bytes::Mem(v) => Ok(v[offset as usize..end as usize].to_vec()),
            Bytes::File(f, _) => {
                let mut buf = vec![0u8; len];
                f.read_exact_at(offset, &mut buf)
                    .map_err(|e| Error::Other(format!("read failed: {e}")))?;
                Ok(buf)
            }
        }
    }
}

/// A windowed reader over [`Bytes`].
struct Window<'a> {
    src: &'a Bytes,
    start: u64,
    buf: Vec<u8>,
}

const WINDOW: u64 = 64 << 10;

impl<'a> Window<'a> {
    fn new(src: &'a Bytes) -> Self {
        Window {
            src,
            start: 0,
            buf: Vec::new(),
        }
    }

    fn get(&mut self, offset: u64, n: usize) -> Result<&[u8]> {
        let end = offset.checked_add(n as u64).ok_or_else(|| {
            Error::corrupt_at(FORMAT_ID, offset, "element offset overflows")
        })?;
        if offset < self.start || end > self.start + self.buf.len() as u64 {
            if end > self.src.len() {
                return Err(Error::corrupt_at(
                    FORMAT_ID,
                    offset,
                    format!(
                        "the data set ends inside an element header at offset {offset} ({} bytes in all; truncated?)",
                        self.src.len()
                    ),
                ));
            }
            let want = (n as u64).max(WINDOW).min(self.src.len() - offset);
            self.buf = self.src.read(offset, want)?;
            self.start = offset;
        }
        let s = (offset - self.start) as usize;
        Ok(&self.buf[s..s + n])
    }
}

fn u16_at(b: &[u8], little: bool) -> u16 {
    let a = [b[0], b[1]];
    if little {
        u16::from_le_bytes(a)
    } else {
        u16::from_be_bytes(a)
    }
}

fn u32_at(b: &[u8], little: bool) -> u32 {
    let a = [b[0], b[1], b[2], b[3]];
    if little {
        u32::from_le_bytes(a)
    } else {
        u32::from_be_bytes(a)
    }
}

/// True for the VRs whose explicit form has two reserved bytes and a 32-bit length.
fn long_vr(vr: [u8; 2]) -> bool {
    matches!(
        &vr,
        b"OB" | b"OD" | b"OF" | b"OL" | b"OV" | b"OW" | b"SQ" | b"SV" | b"UC" | b"UN" | b"UR"
            | b"UT" | b"UV"
    )
}

fn valid_vr(vr: [u8; 2]) -> bool {
    vr.iter().all(u8::is_ascii_uppercase)
}

/// The VR of `tag` from the dictionary (implicit VR); `UN` for unknown and private tags.
pub(crate) fn dictionary_vr(tag: u32) -> [u8; 2] {
    if tag >> 16 == 0xFFFE {
        return *b"UN";
    }
    // group length elements
    if tag & 0xFFFF == 0 {
        return *b"UL";
    }
    match dictionary::lookup(tag) {
        Some((_, vr)) => {
            let b = vr.as_bytes();
            [b[0], b[1]]
        }
        None => *b"UN",
    }
}

/// Parser state for one data set.
pub(crate) struct Parser<'a> {
    win: Window<'a>,
    enc: Encoding,
    elements: u64,
    /// Stop at the top-level pixel data (headers only).
    pub(crate) stop_at_pixels: bool,
    /// Offset where parsing stopped (end of the data set or the pixel data element).
    pub(crate) end: u64,
    /// Problems found and skipped over.
    pub(crate) problems: Vec<String>,
}

impl<'a> Parser<'a> {
    pub(crate) fn new(src: &'a Bytes, enc: Encoding) -> Self {
        Parser {
            win: Window::new(src),
            enc,
            elements: 0,
            stop_at_pixels: true,
            end: 0,
            problems: Vec::new(),
        }
    }

    /// Parse the top-level data set from `offset` to the end of the data (or the pixel data).
    pub(crate) fn top(&mut self, offset: u64) -> Result<Item> {
        let end = self.win.src.len();
        let (items, pos) = self.dataset(offset, Some(end), 0, self.enc, true)?;
        self.end = pos;
        Ok(items)
    }

    /// Elements from `pos` until `end` (defined length) or an item delimiter (undefined).
    /// Returns them with the offset after the last one read.
    fn dataset(
        &mut self,
        mut pos: u64,
        end: Option<u64>,
        depth: u32,
        enc: Encoding,
        top: bool,
    ) -> Result<(Item, u64)> {
        if depth > MAX_DEPTH {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                pos,
                format!("sequences nest deeper than {MAX_DEPTH} levels"),
            ));
        }
        let mut out: Item = Vec::new();
        loop {
            if let Some(e) = end
                && pos >= e
            {
                break;
            }
            if end.is_none() && pos >= self.win.src.len() {
                return Err(Error::corrupt_at(
                    FORMAT_ID,
                    pos,
                    "an item of undefined length runs to the end of the file without an item delimiter",
                ));
            }
            // A few bytes of trailing padding after the last element are tolerated.
            let left = end.unwrap_or(self.win.src.len()).saturating_sub(pos);
            if top && left < 8 {
                if left > 0 {
                    self.problems
                        .push(format!("{left} trailing bytes after the last element"));
                }
                pos += left;
                break;
            }
            let h = self.win.get(pos, 8)?.to_vec();
            let tag = (u32::from(u16_at(&h[0..2], enc.little)) << 16)
                | u32::from(u16_at(&h[2..4], enc.little));
            if tag == ITEM_END {
                return Ok((out, pos + 8));
            }
            if tag == SEQ_END {
                // A sequence delimiter where an element should be: end of this data set.
                self.problems.push(format!(
                    "a sequence delimiter at offset {pos} ends a data set early"
                ));
                return Ok((out, pos + 8));
            }
            self.elements += 1;
            if self.elements > MAX_ELEMENTS {
                return Err(Error::corrupt(
                    FORMAT_ID,
                    format!("more than {MAX_ELEMENTS} data elements"),
                ));
            }
            let vr_bytes = [h[4], h[5]];
            let (vr, len, value_at) = if enc.explicit && valid_vr(vr_bytes) {
                if long_vr(vr_bytes) {
                    let b = self.win.get(pos + 8, 4)?;
                    (vr_bytes, u32_at(b, enc.little), pos + 12)
                } else {
                    (vr_bytes, u32::from(u16_at(&h[6..8], enc.little)), pos + 8)
                }
            } else {
                if enc.explicit {
                    self.problems.push(format!(
                        "element ({:04X},{:04X}) at offset {pos} has no valid VR; read as implicit VR",
                        tag >> 16,
                        tag & 0xFFFF
                    ));
                }
                (dictionary_vr(tag), u32_at(&h[4..8], enc.little), pos + 8)
            };
            let is_pixels = matches!(tag, PIXEL_DATA | FLOAT_PIXEL_DATA | DOUBLE_PIXEL_DATA);
            if len == UNDEFINED {
                if is_pixels {
                    out.push(Element {
                        tag,
                        vr,
                        little: enc.little,
                        value: Value::Encapsulated { offset: value_at },
                    });
                    if top && self.stop_at_pixels {
                        return Ok((out, pos));
                    }
                    pos = self.skip_items(value_at)?;
                    continue;
                }
                // SQ, or UN / an unknown implicit tag holding an implicit-VR sequence
                // (PS3.5 §6.2.2).
                let inner = if &vr == b"SQ" {
                    enc
                } else {
                    Encoding::IMPLICIT_LE
                };
                let (items, next) = self.items(value_at, None, depth + 1, inner)?;
                out.push(Element {
                    tag,
                    vr: *b"SQ",
                    little: enc.little,
                    value: Value::Seq(items),
                });
                pos = next;
                continue;
            }
            let len = u64::from(len);
            let next = value_at.checked_add(len).ok_or_else(|| {
                Error::corrupt_at(FORMAT_ID, pos, "element length overflows")
            })?;
            if let Some(e) = end
                && next > e
            {
                if top && is_pixels {
                    // truncated pixel data: report it, keep what is there
                    out.push(Element {
                        tag,
                        vr,
                        little: enc.little,
                        value: Value::Deferred {
                            offset: value_at,
                            len,
                        },
                    });
                    self.problems.push(format!(
                        "the pixel data declares {len} bytes but the file ends {} bytes after it starts",
                        e.saturating_sub(value_at)
                    ));
                    return Ok((out, e));
                }
                return Err(Error::corrupt_at(
                    FORMAT_ID,
                    pos,
                    format!(
                        "element ({:04X},{:04X}) declares {len} bytes, past the end of its {} ({} bytes left)",
                        tag >> 16,
                        tag & 0xFFFF,
                        if top { "file" } else { "item" },
                        e.saturating_sub(value_at)
                    ),
                ));
            }
            if &vr == b"SQ" {
                let (items, _) = self.items(value_at, Some(next), depth + 1, enc)?;
                out.push(Element {
                    tag,
                    vr,
                    little: enc.little,
                    value: Value::Seq(items),
                });
            } else if is_pixels || len > MAX_INLINE {
                out.push(Element {
                    tag,
                    vr,
                    little: enc.little,
                    value: Value::Deferred {
                        offset: value_at,
                        len,
                    },
                });
                if is_pixels && top && self.stop_at_pixels {
                    return Ok((out, next));
                }
            } else {
                let bytes = self.win.get(value_at, len as usize)?.to_vec();
                out.push(Element {
                    tag,
                    vr,
                    little: enc.little,
                    value: Value::Bytes(bytes),
                });
            }
            pos = next;
        }
        Ok((out, pos))
    }

    /// The items of a sequence starting at `pos`: until `end` (defined length) or a sequence
    /// delimiter (undefined).
    fn items(
        &mut self,
        mut pos: u64,
        end: Option<u64>,
        depth: u32,
        enc: Encoding,
    ) -> Result<(Vec<Item>, u64)> {
        let mut out = Vec::new();
        loop {
            if let Some(e) = end
                && pos >= e
            {
                return Ok((out, pos));
            }
            let h = self.win.get(pos, 8)?;
            let tag = (u32::from(u16_at(&h[0..2], enc.little)) << 16)
                | u32::from(u16_at(&h[2..4], enc.little));
            let len = u32_at(&h[4..8], enc.little);
            match tag {
                SEQ_END => return Ok((out, pos + 8)),
                ITEM => {
                    let start = pos + 8;
                    if len == UNDEFINED {
                        let (item, next) = self.dataset(start, None, depth, enc, false)?;
                        out.push(item);
                        pos = next;
                    } else {
                        let stop = start.checked_add(u64::from(len)).ok_or_else(|| {
                            Error::corrupt_at(FORMAT_ID, pos, "item length overflows")
                        })?;
                        if end.is_some_and(|e| stop > e) || stop > self.win.src.len() {
                            return Err(Error::corrupt_at(
                                FORMAT_ID,
                                pos,
                                format!("an item of {len} bytes runs past the end of its sequence"),
                            ));
                        }
                        let (item, _) = self.dataset(start, Some(stop), depth, enc, false)?;
                        out.push(item);
                        pos = stop;
                    }
                    self.elements += 1;
                    if self.elements > MAX_ELEMENTS {
                        return Err(Error::corrupt(
                            FORMAT_ID,
                            format!("more than {MAX_ELEMENTS} data elements"),
                        ));
                    }
                }
                _ => {
                    return Err(Error::corrupt_at(
                        FORMAT_ID,
                        pos,
                        format!(
                            "expected a sequence item, found ({:04X},{:04X})",
                            tag >> 16,
                            tag & 0xFFFF
                        ),
                    ));
                }
            }
        }
    }

    /// Skip encapsulated pixel data starting at `pos` (its items up to the sequence delimiter).
    fn skip_items(&mut self, mut pos: u64) -> Result<u64> {
        loop {
            let h = self.win.get(pos, 8)?;
            let tag = (u32::from(u16_at(&h[0..2], true)) << 16) | u32::from(u16_at(&h[2..4], true));
            let len = u64::from(u32_at(&h[4..8], true));
            match tag {
                SEQ_END => return Ok(pos + 8),
                ITEM => pos = pos + 8 + len,
                _ => {
                    return Err(Error::corrupt_at(
                        FORMAT_ID,
                        pos,
                        "encapsulated pixel data holds something other than items",
                    ));
                }
            }
        }
    }
}

/// One fragment of encapsulated pixel data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fragment {
    pub(crate) offset: u64,
    pub(crate) len: u64,
}

/// The items of encapsulated pixel data at `offset` (PS3.5 A.4): the Basic Offset Table
/// (first item) and the fragments, up to the sequence delimiter or the end of the file.
pub(crate) fn fragments(src: &Bytes, offset: u64) -> Result<(Vec<u8>, Vec<Fragment>, bool)> {
    let mut pos = offset;
    let mut bot = None;
    let mut frags = Vec::new();
    let mut complete = false;
    let len = src.len();
    while pos + 8 <= len {
        let h = src.read(pos, 8)?;
        let tag = (u32::from(u16_at(&h[0..2], true)) << 16) | u32::from(u16_at(&h[2..4], true));
        let n = u64::from(u32_at(&h[4..8], true));
        if tag == SEQ_END {
            complete = true;
            break;
        }
        if tag != ITEM {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                pos,
                format!(
                    "encapsulated pixel data: expected an item, found ({:04X},{:04X})",
                    tag >> 16,
                    tag & 0xFFFF
                ),
            ));
        }
        let start = pos + 8;
        if n == u64::from(UNDEFINED) || start + n > len {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                pos,
                format!("a pixel-data fragment of {n} bytes runs past the end of the file"),
            ));
        }
        if bot.is_none() {
            bot = Some(if n > 0 && n <= 64 << 20 {
                src.read(start, n)?
            } else {
                Vec::new()
            });
        } else {
            frags.push(Fragment { offset: start, len: n });
            if frags.len() as u64 > MAX_ELEMENTS {
                return Err(Error::corrupt(FORMAT_ID, "too many pixel-data fragments"));
            }
        }
        pos = start + n;
    }
    Ok((bot.unwrap_or_default(), frags, complete))
}

/// Where a Part 10 file's data set starts, and how its meta group says it is encoded.
#[derive(Debug, Clone)]
pub(crate) struct Head {
    /// The file meta group (group 0002), empty for files without one.
    pub(crate) meta: Item,
    /// Offset of the first data-set element.
    pub(crate) start: u64,
    /// True when the file has the 128-byte preamble and `DICM`.
    pub(crate) preamble: bool,
}

/// True when `head` is a Part 10 file (`DICM` after the 128-byte preamble).
pub(crate) fn has_magic(head: &[u8]) -> bool {
    head.get(128..132) == Some(b"DICM")
}

/// How a file without preamble looks like it is encoded (PS3.10 allows none in other media,
/// and old ACR-NEMA style files start with group 0008): `None` when it does not look like a
/// data set at all.
pub(crate) fn guess_raw(head: &[u8]) -> Option<Encoding> {
    if head.len() < 16 {
        return None;
    }
    let g_le = u16::from_le_bytes([head[0], head[1]]);
    let g_be = u16::from_be_bytes([head[0], head[1]]);
    let el_le = u16::from_le_bytes([head[2], head[3]]);
    let explicit = valid_vr([head[4], head[5]]) && dictionary_vr_known(&[head[4], head[5]]);
    if matches!(g_le, 0x0002 | 0x0008) && el_le < 0x0100 {
        return Some(if explicit {
            Encoding::EXPLICIT_LE
        } else {
            // implicit: a sane 32-bit length
            let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]);
            if len > 1 << 20 {
                return None;
            }
            Encoding::IMPLICIT_LE
        });
    }
    if matches!(g_be, 0x0002 | 0x0008) && explicit {
        return Some(Encoding::EXPLICIT_BE);
    }
    None
}

fn dictionary_vr_known(vr: &[u8; 2]) -> bool {
    matches!(
        vr,
        b"AE" | b"AS" | b"AT" | b"CS" | b"DA" | b"DS" | b"DT" | b"FL" | b"FD" | b"IS" | b"LO"
            | b"LT" | b"OB" | b"OD" | b"OF" | b"OL" | b"OV" | b"OW" | b"PN" | b"SH" | b"SL"
            | b"SQ" | b"SS" | b"ST" | b"SV" | b"TM" | b"UC" | b"UI" | b"UL" | b"UN" | b"UR"
            | b"US" | b"UT" | b"UV"
    )
}

/// Read the preamble and meta group (always explicit VR little endian). Files without a
/// preamble start their data set at 0.
pub(crate) fn read_head(src: &Bytes) -> Result<Head> {
    let first = src.read(0, src.len().min(132))?;
    if !has_magic(&first) {
        return Ok(Head {
            meta: Vec::new(),
            start: 0,
            preamble: false,
        });
    }
    // Meta elements: group 0002 elements in explicit VR little endian, until another group.
    let mut p = Parser::new(src, Encoding::EXPLICIT_LE);
    p.stop_at_pixels = true;
    let mut pos = 132u64;
    let mut meta = Vec::new();
    while pos + 8 <= src.len() {
        let h = src.read(pos, 2)?;
        if u16::from_le_bytes([h[0], h[1]]) != 0x0002 {
            break;
        }
        let (mut one, next) = p.one(pos)?;
        meta.append(&mut one);
        pos = next;
    }
    Ok(Head {
        meta,
        start: pos,
        preamble: true,
    })
}

impl Parser<'_> {
    /// Parse one element at `pos` (meta group).
    fn one(&mut self, pos: u64) -> Result<(Item, u64)> {
        let h = self.win.get(pos, 8)?.to_vec();
        let vr = [h[4], h[5]];
        let (len, at) = if long_vr(vr) {
            (u64::from(u32_at(self.win.get(pos + 8, 4)?, true)), pos + 12)
        } else {
            (u64::from(u16_at(&h[6..8], true)), pos + 8)
        };
        let end = at.checked_add(len).filter(|e| *e <= self.win.src.len());
        let Some(end) = end else {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                pos,
                "a file meta element runs past the end of the file",
            ));
        };
        let (items, next) = self.dataset(pos, Some(end), 0, Encoding::EXPLICIT_LE, false)?;
        Ok((items, next))
    }
}

/// Inflate a deflated data set (transfer syntax 1.2.840.10008.1.2.1.99: raw DEFLATE, no zlib
/// header), at most `limit` bytes.
pub(crate) fn inflate(data: &[u8], limit: usize) -> Result<Vec<u8>> {
    miniz_oxide::inflate::decompress_to_vec_with_limit(data, limit).map_err(|e| {
        Error::corrupt(
            FORMAT_ID,
            format!("the deflated data set does not inflate ({:?})", e.status),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(tag: u32, vr: &[u8; 2], v: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&((tag >> 16) as u16).to_le_bytes());
        b.extend_from_slice(&((tag & 0xFFFF) as u16).to_le_bytes());
        b.extend_from_slice(vr);
        if long_vr(*vr) {
            b.extend_from_slice(&[0, 0]);
            b.extend_from_slice(&(v.len() as u32).to_le_bytes());
        } else {
            b.extend_from_slice(&(v.len() as u16).to_le_bytes());
        }
        b.extend_from_slice(v);
        b
    }

    #[test]
    fn parses_nested_sequences_of_undefined_length() {
        let mut ds = el(0x0010_0020, b"LO", b"ID01");
        // (0008,1140) SQ undefined length, one undefined-length item holding (0008,1150)
        ds.extend_from_slice(&[0x08, 0x00, 0x40, 0x11, b'S', b'Q', 0, 0, 0xFF, 0xFF, 0xFF, 0xFF]);
        ds.extend_from_slice(&[0xFE, 0xFF, 0x00, 0xE0, 0xFF, 0xFF, 0xFF, 0xFF]);
        ds.extend(el(0x0008_1150, b"UI", b"1.2\0"));
        ds.extend_from_slice(&[0xFE, 0xFF, 0x0D, 0xE0, 0, 0, 0, 0]);
        ds.extend_from_slice(&[0xFE, 0xFF, 0xDD, 0xE0, 0, 0, 0, 0]);
        ds.extend(el(0x0028_0010, b"US", &4u16.to_le_bytes()));
        let src = Bytes::Mem(Arc::new(ds));
        let mut p = Parser::new(&src, Encoding::EXPLICIT_LE);
        let items = p.top(0).unwrap();
        assert_eq!(items.len(), 3);
        let Value::Seq(seq) = &items[1].value else {
            panic!("not a sequence")
        };
        assert_eq!(seq.len(), 1);
        assert_eq!(seq[0][0].tag, 0x0008_1150);
        assert_eq!(items[2].tag, 0x0028_0010);
    }

    #[test]
    fn truncated_and_hostile_lengths_are_errors() {
        let mut ds = el(0x0010_0020, b"LO", b"ID01");
        ds.extend_from_slice(&[0x10, 0x00, 0x30, 0x00, b'O', b'B', 0, 0, 0xF0, 0xFF, 0xFF, 0x7F]);
        let src = Bytes::Mem(Arc::new(ds));
        assert!(Parser::new(&src, Encoding::EXPLICIT_LE).top(0).is_err());
        // deep nesting is refused rather than overflowing the stack
        let mut deep = Vec::new();
        for _ in 0..200 {
            deep.extend_from_slice(&[0x08, 0x00, 0x40, 0x11, b'S', b'Q', 0, 0, 0xFF, 0xFF, 0xFF, 0xFF]);
            deep.extend_from_slice(&[0xFE, 0xFF, 0x00, 0xE0, 0xFF, 0xFF, 0xFF, 0xFF]);
        }
        let src = Bytes::Mem(Arc::new(deep));
        assert!(Parser::new(&src, Encoding::EXPLICIT_LE).top(0).is_err());
    }
}
