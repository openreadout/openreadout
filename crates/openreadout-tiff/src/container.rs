//! The TIFF container: header, IFD chain, field values. See `docs/formats/tiff.md` § Container.
//!
//! Everything here follows the published TIFF 6.0 and BigTIFF specifications. The walk never
//! trusts an offset: every read is bounds-checked against the file length, cycles are
//! detected, and problems are recorded (for `check`) instead of aborting the open.

use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use openreadout_core::bytes::Endian;
use openreadout_core::source::{Fs, SourceFile};
use openreadout_core::{Error, Result};

use crate::FORMAT_ID;

/// Classic TIFF version number (bytes 2..4).
pub const TIFF_MAGIC: u16 = 42;
/// BigTIFF version number (bytes 2..4).
pub const BIGTIFF_MAGIC: u16 = 43;

/// Largest single field value we load (ImageDescription with a large OME-XML block fits).
const MAX_FIELD_BYTES: u64 = 256 << 20;
/// Upper bound on IFDs in one chain; real files stay far below it.
const MAX_IFDS: usize = 4_000_000;

/// Byte order from the first two header bytes (`II` or `MM`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteOrder {
    Little,
    Big,
}

/// The 8-byte (classic) or 16-byte (BigTIFF) file header.
#[derive(Debug, Clone)]
pub struct TiffHeader {
    pub byte_order: ByteOrder,
    pub big_tiff: bool,
    pub first_ifd_offset: u64,
}

/// A decoded field value. Rationals are divided out to `Float`.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    Unsigned(Vec<u64>),
    Signed(Vec<i64>),
    Float(Vec<f64>),
    Ascii(String),
    Bytes(Vec<u8>),
}

/// One IFD entry.
#[derive(Debug, Clone)]
pub struct Field {
    pub tag: u16,
    pub field_type: u16,
    pub count: u64,
    /// Where an out-of-line value lives; `None` when it was stored in the entry itself.
    pub value_offset: Option<u64>,
    pub value: FieldValue,
}

/// One image file directory.
#[derive(Debug, Clone)]
pub struct Ifd {
    /// Byte offset of the directory.
    pub offset: u64,
    /// Entries sorted by tag.
    pub fields: Vec<Field>,
    /// Offset of the next IFD in the chain (0 = end).
    pub next_offset: u64,
}

/// Something wrong with the structure, recorded during the walk and reported by `check`.
#[derive(Debug, Clone)]
pub struct StructureProblem {
    /// `truncated`, `bad_ifd_offset`, `ifd_cycle`, `bad_field` ...
    pub code: &'static str,
    pub offset: u64,
    pub detail: String,
}

/// An opened TIFF: header plus the main IFD chain.
#[derive(Debug)]
pub struct TiffFile {
    pub path: PathBuf,
    pub file_len: u64,
    pub header: TiffHeader,
    /// Main-chain IFDs in file order ("pages").
    pub ifds: Vec<Ifd>,
    pub problems: Vec<StructureProblem>,
}

/// Does this look like a TIFF or BigTIFF header?
pub fn looks_like_tiff(head: &[u8]) -> bool {
    matches!(
        head.get(..4),
        Some([b'I', b'I', 42 | 43, 0] | [b'M', b'M', 0, 42 | 43])
    )
}

impl Ifd {
    pub fn field(&self, tag: u16) -> Option<&Field> {
        self.fields
            .binary_search_by_key(&tag, |f| f.tag)
            .ok()
            .map(|i| &self.fields[i])
    }

    /// All values of an unsigned field (SHORT/LONG/LONG8/IFD/BYTE).
    pub fn uints(&self, tag: u16) -> Option<Vec<u64>> {
        match &self.field(tag)?.value {
            FieldValue::Unsigned(v) => Some(v.clone()),
            FieldValue::Bytes(b) => Some(b.iter().map(|&x| u64::from(x)).collect()),
            FieldValue::Signed(v) => v.iter().map(|&x| u64::try_from(x).ok()).collect(),
            _ => None,
        }
    }

    /// First value of an unsigned field.
    pub fn uint(&self, tag: u16) -> Option<u64> {
        match &self.field(tag)?.value {
            FieldValue::Unsigned(v) => v.first().copied(),
            FieldValue::Bytes(b) => b.first().map(|&x| u64::from(x)),
            FieldValue::Signed(v) => v.first().and_then(|&x| u64::try_from(x).ok()),
            _ => None,
        }
    }

    /// First value of any numeric field as `f64`.
    pub fn float(&self, tag: u16) -> Option<f64> {
        match &self.field(tag)?.value {
            FieldValue::Unsigned(v) => v.first().map(|&x| x as f64),
            FieldValue::Signed(v) => v.first().map(|&x| x as f64),
            FieldValue::Float(v) => v.first().copied(),
            _ => None,
        }
    }

    /// ASCII field as text (trailing NULs removed).
    pub fn text(&self, tag: u16) -> Option<&str> {
        match &self.field(tag)?.value {
            FieldValue::Ascii(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// BYTE/UNDEFINED field as raw bytes.
    pub fn bytes(&self, tag: u16) -> Option<&[u8]> {
        match &self.field(tag)?.value {
            FieldValue::Bytes(b) => Some(b.as_slice()),
            _ => None,
        }
    }
}

/// Where a [`ByteSource`] reads from: a file on disk, or a TIFF held in memory (a stream of a
/// container format such as an Olympus OIB compound file).
#[derive(Debug)]
enum Backing {
    File(SourceFile),
    Memory(Vec<u8>),
}

/// Random-access reader with the file's byte order.
#[derive(Debug)]
pub struct ByteSource {
    backing: Backing,
    path: PathBuf,
    pub len: u64,
    pub order: ByteOrder,
    pub big: bool,
}

impl ByteSource {
    /// Open a local file.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_in(&Fs::local(), path)
    }

    /// Open `path` in the namespace `fs`.
    pub(crate) fn open_in(fs: &Fs, path: &Path) -> Result<Self> {
        let file = fs.open(path).map_err(|e| Error::io(path, e))?;
        let len = file.metadata().map_err(|e| Error::io(path, e))?.len();
        Ok(ByteSource {
            backing: Backing::File(file),
            path: path.to_path_buf(),
            len,
            order: ByteOrder::Little,
            big: false,
        })
    }

    /// A TIFF held in memory; `label` names it in errors.
    pub fn from_bytes(label: &Path, bytes: Vec<u8>) -> Self {
        ByteSource {
            len: bytes.len() as u64,
            backing: Backing::Memory(bytes),
            path: label.to_path_buf(),
            order: ByteOrder::Little,
            big: false,
        }
    }

    /// Read exactly `len` bytes at `offset`; a clean corrupt error when that runs past the end.
    pub fn read_at(&mut self, offset: u64, len: u64) -> Result<Vec<u8>> {
        let end = offset.checked_add(len).filter(|&e| e <= self.len);
        if end.is_none() {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                offset,
                format!(
                    "{len} bytes at offset {offset} run past the end of the file ({} bytes)",
                    self.len
                ),
            ));
        }
        let n = usize::try_from(len)
            .map_err(|_| Error::corrupt_at(FORMAT_ID, offset, "read length overflows"))?;
        match &mut self.backing {
            Backing::File(file) => {
                let mut buf = vec![0u8; n];
                file.seek(SeekFrom::Start(offset))
                    .map_err(|e| Error::io(&self.path, e))?;
                file.read_exact(&mut buf)
                    .map_err(|e| Error::io(&self.path, e))?;
                Ok(buf)
            }
            Backing::Memory(bytes) => {
                // `offset + len <= self.len` was checked above
                let start = usize::try_from(offset)
                    .map_err(|_| Error::corrupt_at(FORMAT_ID, offset, "offset overflows"))?;
                bytes
                    .get(start..start + n)
                    .map(<[u8]>::to_vec)
                    .ok_or_else(|| Error::corrupt_at(FORMAT_ID, offset, "read past the end"))
            }
        }
    }

    fn endian(&self) -> Endian {
        match self.order {
            ByteOrder::Little => Endian::Little,
            ByteOrder::Big => Endian::Big,
        }
    }
    pub fn u16_of(&self, b: &[u8]) -> u16 {
        self.endian().u16(b, 0).unwrap_or(0)
    }
    pub fn u32_of(&self, b: &[u8]) -> u32 {
        self.endian().u32(b, 0).unwrap_or(0)
    }
    pub fn u64_of(&self, b: &[u8]) -> u64 {
        self.endian().u64(b, 0).unwrap_or(0)
    }
    fn offset_of(&self, b: &[u8]) -> u64 {
        if self.big {
            self.u64_of(b)
        } else {
            u64::from(self.u32_of(b))
        }
    }
}

/// Size in bytes of one value of a TIFF field type; `None` for unknown types.
pub fn type_size(field_type: u16) -> Option<u64> {
    Some(match field_type {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 | 16..=18 => 8,
        _ => return None,
    })
}

impl TiffFile {
    /// Parse the header and walk the main IFD chain.
    pub fn open(path: &Path) -> Result<(Self, ByteSource)> {
        Self::open_in(&Fs::local(), path)
    }

    /// Parse the header and walk the main IFD chain of `path` in the namespace `fs`.
    pub fn open_in(fs: &Fs, path: &Path) -> Result<(Self, ByteSource)> {
        Self::from_source(path, ByteSource::open_in(fs, path)?)
    }

    /// Parse a TIFF held in memory (a stream inside a container file); `label` names it in
    /// errors and in [`TiffFile::path`].
    pub fn from_bytes(label: &Path, bytes: Vec<u8>) -> Result<(Self, ByteSource)> {
        Self::from_source(label, ByteSource::from_bytes(label, bytes))
    }

    fn from_source(path: &Path, mut src: ByteSource) -> Result<(Self, ByteSource)> {
        if src.len < 8 {
            return Err(Error::corrupt(
                FORMAT_ID,
                "file is shorter than a TIFF header",
            ));
        }
        let head = src.read_at(0, 8.min(src.len))?;
        if !looks_like_tiff(&head) {
            return Err(Error::corrupt(
                FORMAT_ID,
                "missing TIFF signature (II*\\0, MM\\0*, II+\\0 or MM\\0+)",
            ));
        }
        src.order = if head[0] == b'I' {
            ByteOrder::Little
        } else {
            ByteOrder::Big
        };
        let version = src.u16_of(&head[2..4]);
        src.big = version == BIGTIFF_MAGIC;
        let first_ifd_offset = if src.big {
            let h = src.read_at(0, 16)?;
            if src.u16_of(&h[4..6]) != 8 || src.u16_of(&h[6..8]) != 0 {
                return Err(Error::corrupt_at(
                    FORMAT_ID,
                    4,
                    "BigTIFF header: offset size must be 8 and the reserved field 0",
                ));
            }
            src.u64_of(&h[8..16])
        } else {
            u64::from(src.u32_of(&head[4..8]))
        };
        let header = TiffHeader {
            byte_order: src.order,
            big_tiff: src.big,
            first_ifd_offset,
        };
        let mut tf = TiffFile {
            path: path.to_path_buf(),
            file_len: src.len,
            header,
            ifds: Vec::new(),
            problems: Vec::new(),
        };
        tf.walk_chain(&mut src, first_ifd_offset);
        Ok((tf, src))
    }

    fn walk_chain(&mut self, src: &mut ByteSource, first: u64) {
        let mut seen = HashSet::new();
        let mut next = first;
        while next != 0 {
            if self.ifds.len() >= MAX_IFDS {
                self.problems.push(StructureProblem {
                    code: "too_many_ifds",
                    offset: next,
                    detail: format!("stopped after {MAX_IFDS} IFDs"),
                });
                break;
            }
            if !seen.insert(next) {
                self.problems.push(StructureProblem {
                    code: "ifd_cycle",
                    offset: next,
                    detail: format!(
                        "IFD chain loops back to offset {next} after {} IFDs",
                        self.ifds.len()
                    ),
                });
                break;
            }
            match read_ifd(src, next, &mut self.problems) {
                Ok(ifd) => {
                    next = ifd.next_offset;
                    self.ifds.push(ifd);
                }
                Err(e) => {
                    let truncated = next >= src.len;
                    self.problems.push(StructureProblem {
                        code: if truncated {
                            "truncated"
                        } else {
                            "bad_ifd_offset"
                        },
                        offset: next,
                        detail: format!(
                            "IFD #{} at offset {next} cannot be read ({e}); the file is {} bytes",
                            self.ifds.len(),
                            src.len
                        ),
                    });
                    break;
                }
            }
        }
    }
}

/// Read one IFD at `offset`. Field-level problems are pushed to `problems` and the field is
/// dropped; a directory that cannot be read at all is an error.
pub fn read_ifd(
    src: &mut ByteSource,
    offset: u64,
    problems: &mut Vec<StructureProblem>,
) -> Result<Ifd> {
    let (count_len, entry_len, off_len) = if src.big {
        (8u64, 20u64, 8u64)
    } else {
        (2, 12, 4)
    };
    if offset < 8 {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            offset,
            "IFD offset points into the header",
        ));
    }
    let cb = src.read_at(offset, count_len)?;
    let n = if src.big {
        src.u64_of(&cb)
    } else {
        u64::from(src.u16_of(&cb))
    };
    if n == 0 || n > 65_535 {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            offset,
            format!("implausible IFD entry count {n}"),
        ));
    }
    let table = src.read_at(offset + count_len, n * entry_len + off_len)?;
    let mut fields = Vec::with_capacity(n as usize);
    for i in 0..n as usize {
        let e = &table[i * entry_len as usize..(i + 1) * entry_len as usize];
        let tag = src.u16_of(&e[0..2]);
        let field_type = src.u16_of(&e[2..4]);
        let count = if src.big {
            src.u64_of(&e[4..12])
        } else {
            u64::from(src.u32_of(&e[4..8]))
        };
        let inline = &e[if src.big { 12 } else { 8 }..];
        let Some(size) = type_size(field_type) else {
            continue; // unknown field types are skipped, as the specification requires
        };
        let Some(total) = size.checked_mul(count).filter(|&t| t <= MAX_FIELD_BYTES) else {
            problems.push(StructureProblem {
                code: "bad_field",
                offset: offset + count_len + (i as u64) * entry_len,
                detail: format!("tag {tag}: value of {count} × {size} bytes is implausible"),
            });
            continue;
        };
        let (raw, value_offset) = if total <= off_len {
            (inline[..total as usize].to_vec(), None)
        } else {
            let at = src.offset_of(inline);
            let Ok(v) = src.read_at(at, total) else {
                problems.push(StructureProblem {
                    code: if at.saturating_add(total) > src.len {
                        "truncated"
                    } else {
                        "bad_field"
                    },
                    offset: at,
                    detail: format!(
                        "tag {tag} in IFD at {offset}: {total}-byte value at offset {at} runs past the end of the file"
                    ),
                });
                continue;
            };
            (v, Some(at))
        };
        let value = decode_value(src, field_type, &raw);
        fields.push(Field {
            tag,
            field_type,
            count,
            value_offset,
            value,
        });
    }
    fields.sort_by_key(|f| f.tag);
    fields.dedup_by_key(|f| f.tag);
    let next_offset = src.offset_of(&table[(n * entry_len) as usize..]);
    Ok(Ifd {
        offset,
        fields,
        next_offset,
    })
}

fn decode_value(src: &ByteSource, field_type: u16, raw: &[u8]) -> FieldValue {
    match field_type {
        1 | 7 => FieldValue::Bytes(raw.to_vec()),
        2 => {
            let end = raw.iter().rposition(|&b| b != 0).map_or(0, |p| p + 1);
            FieldValue::Ascii(String::from_utf8_lossy(&raw[..end]).into_owned())
        }
        3 => FieldValue::Unsigned(
            raw.as_chunks::<2>()
                .0
                .iter()
                .map(|c| u64::from(src.u16_of(c)))
                .collect(),
        ),
        4 | 13 => FieldValue::Unsigned(
            raw.as_chunks::<4>()
                .0
                .iter()
                .map(|c| u64::from(src.u32_of(c)))
                .collect(),
        ),
        16 | 18 => FieldValue::Unsigned(
            raw.as_chunks::<8>()
                .0
                .iter()
                .map(|c| src.u64_of(c))
                .collect(),
        ),
        6 => FieldValue::Signed(raw.iter().map(|&b| i64::from(b as i8)).collect()),
        8 => FieldValue::Signed(
            raw.as_chunks::<2>()
                .0
                .iter()
                .map(|c| i64::from(src.u16_of(c) as i16))
                .collect(),
        ),
        9 => FieldValue::Signed(
            raw.as_chunks::<4>()
                .0
                .iter()
                .map(|c| i64::from(src.u32_of(c) as i32))
                .collect(),
        ),
        17 => FieldValue::Signed(
            raw.as_chunks::<8>()
                .0
                .iter()
                .map(|c| src.u64_of(c) as i64)
                .collect(),
        ),
        5 => FieldValue::Float(
            raw.as_chunks::<8>()
                .0
                .iter()
                .map(|c| {
                    let (n, d) = (src.u32_of(&c[..4]), src.u32_of(&c[4..]));
                    if d == 0 {
                        0.0
                    } else {
                        f64::from(n) / f64::from(d)
                    }
                })
                .collect(),
        ),
        10 => FieldValue::Float(
            raw.as_chunks::<8>()
                .0
                .iter()
                .map(|c| {
                    let (n, d) = (src.u32_of(&c[..4]) as i32, src.u32_of(&c[4..]) as i32);
                    if d == 0 {
                        0.0
                    } else {
                        f64::from(n) / f64::from(d)
                    }
                })
                .collect(),
        ),
        11 => FieldValue::Float(
            raw.as_chunks::<4>()
                .0
                .iter()
                .map(|c| f64::from(f32::from_bits(src.u32_of(c))))
                .collect(),
        ),
        12 => FieldValue::Float(
            raw.as_chunks::<8>()
                .0
                .iter()
                .map(|c| f64::from_bits(src.u64_of(c)))
                .collect(),
        ),
        _ => FieldValue::Bytes(raw.to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature() {
        assert!(looks_like_tiff(b"II*\0...."));
        assert!(looks_like_tiff(b"MM\0*...."));
        assert!(looks_like_tiff(b"II+\0...."));
        assert!(!looks_like_tiff(b"II\0*...."));
        assert!(!looks_like_tiff(b"PK\x03\x04"));
    }

    #[test]
    fn type_sizes() {
        assert_eq!(type_size(3), Some(2));
        assert_eq!(type_size(16), Some(8));
        assert_eq!(type_size(99), None);
    }
}
