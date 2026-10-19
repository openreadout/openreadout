//! MATLAB level-5 MAT-files (the container of EEGLAB `.set` files): a 128-byte header, then
//! data elements, each a tag (type, byte count) and its bytes padded to 8; top-level variables
//! are `miMATRIX` elements, zlib-compressed into `miCOMPRESSED` elements in version 7 files.
//! Numeric arrays larger than [`MAX_MAT_INLINE`] are not read when the file is parsed: their
//! place is kept so the samples can be read later. Notes: `docs/formats/eeglab.md`.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use openreadout_core::bytes::{Endian, latin1};
use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};

use super::EEGLAB_FORMAT_ID;

/// Bytes of the file header.
pub const MAT_HEADER_LEN: u64 = 128;
/// Numeric arrays of at most this many bytes are read when the file is parsed.
pub const MAX_MAT_INLINE: u64 = 4 << 20;
/// Bytes of one character array read at most.
pub const MAX_MAT_TEXT: u64 = 64 << 20;
/// Nesting depth of cells and structs accepted at most.
pub const MAX_MAT_DEPTH: usize = 32;
/// Arrays (every cell, struct field and number array counts) accepted at most per file.
pub const MAX_MAT_NODES: usize = 2_000_000;
/// Elements of one array accepted at most.
pub const MAX_MAT_ELEMENTS: u64 = 1 << 40;

/// `miINT8` … `miUTF32`: how an element stores its values.
pub type MiType = u32;
/// `miINT8`.
pub const MI_INT8: MiType = 1;
/// `miUINT8`.
pub const MI_UINT8: MiType = 2;
/// `miINT16`.
pub const MI_INT16: MiType = 3;
/// `miUINT16`.
pub const MI_UINT16: MiType = 4;
/// `miINT32`.
pub const MI_INT32: MiType = 5;
/// `miUINT32`.
pub const MI_UINT32: MiType = 6;
/// `miSINGLE`.
pub const MI_SINGLE: MiType = 7;
/// `miDOUBLE`.
pub const MI_DOUBLE: MiType = 9;
/// `miINT64`.
pub const MI_INT64: MiType = 12;
/// `miUINT64`.
pub const MI_UINT64: MiType = 13;
/// `miMATRIX`: an array.
pub const MI_MATRIX: MiType = 14;
/// `miCOMPRESSED`: a zlib stream holding one element.
pub const MI_COMPRESSED: MiType = 15;
/// `miUTF8`.
pub const MI_UTF8: MiType = 16;
/// `miUTF16`.
pub const MI_UTF16: MiType = 17;
/// `miUTF32`.
pub const MI_UTF32: MiType = 18;

/// Bytes per value of a stored type (0 for unknown types).
pub fn mi_size(t: MiType) -> u64 {
    match t {
        MI_INT8 | MI_UINT8 | MI_UTF8 => 1,
        MI_INT16 | MI_UINT16 | MI_UTF16 => 2,
        MI_INT32 | MI_UINT32 | MI_SINGLE | MI_UTF32 => 4,
        MI_DOUBLE | MI_INT64 | MI_UINT64 => 8,
        _ => 0,
    }
}

/// The values of a numeric array: read now (`values`), or only located (`at`, an offset into
/// the variable's stream: the file, or the inflated bytes of a compressed variable).
#[derive(Debug, Clone, PartialEq)]
pub struct MatNumbers {
    /// How the values are stored (may be narrower than the array's class).
    pub stored: MiType,
    /// Number of values.
    pub count: u64,
    /// The values as f64, when the array is small enough to read at once.
    pub values: Option<Vec<f64>>,
    /// Where the stored values start in the variable's stream.
    pub at: u64,
}

/// A MATLAB array.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum MatValue {
    /// A numeric or logical array (`class` is the `mx…_CLASS` code: 6 double, 7 single, …).
    Numeric {
        /// The array class code.
        class: u8,
        /// Dimensions.
        dims: Vec<u64>,
        /// The real part.
        data: MatNumbers,
    },
    /// A character array; rows of an m×n array are joined with newlines.
    Char {
        /// Dimensions.
        dims: Vec<u64>,
        /// The text.
        text: String,
    },
    /// A cell array, items in column-major order.
    Cell {
        /// Dimensions.
        dims: Vec<u64>,
        /// The cells.
        items: Vec<MatValue>,
    },
    /// A struct array (or an object, with its class name): `values[e * fields.len() + f]` is
    /// field `f` of element `e`, elements in column-major order.
    Struct {
        /// Dimensions.
        dims: Vec<u64>,
        /// Field names.
        fields: Vec<String>,
        /// Field values.
        values: Vec<MatValue>,
        /// The class name of an object (`mxOBJECT_CLASS`).
        class_name: Option<String>,
    },
    /// An array of a class not decoded (sparse, function handle, opaque objects).
    Other {
        /// The array class code.
        class: u8,
    },
}

impl MatValue {
    /// Number of elements (the product of the dimensions).
    pub fn len(&self) -> u64 {
        let dims = match self {
            MatValue::Numeric { dims, .. }
            | MatValue::Char { dims, .. }
            | MatValue::Cell { dims, .. }
            | MatValue::Struct { dims, .. } => dims,
            MatValue::Other { .. } => return 0,
        };
        dims.iter().fold(1u64, |a, &d| a.saturating_mul(d))
    }
    /// Number of elements actually stored: a struct's or cell's values, a numeric array's read
    /// or located values, a character array's characters. Unlike [`MatValue::len`], which
    /// multiplies the declared dimensions, it cannot exceed what the file holds.
    pub fn elements(&self) -> u64 {
        match self {
            MatValue::Struct { fields, values, .. } => {
                if fields.is_empty() {
                    0
                } else {
                    (values.len() / fields.len()) as u64
                }
            }
            MatValue::Cell { items, .. } => items.len() as u64,
            MatValue::Numeric { data, .. } => data.count,
            MatValue::Char { text, .. } => text.chars().count() as u64,
            MatValue::Other { .. } => 0,
        }
    }
    /// True when the array has no elements.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// The text of a character array.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            MatValue::Char { text, .. } => Some(text),
            _ => None,
        }
    }
    /// The first value of a numeric array read at parse time.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            MatValue::Numeric { data, .. } => data.values.as_ref()?.first().copied(),
            _ => None,
        }
    }
    /// The values of a numeric array read at parse time.
    pub fn as_f64s(&self) -> Option<&[f64]> {
        match self {
            MatValue::Numeric { data, .. } => data.values.as_deref(),
            _ => None,
        }
    }
    /// Field `name` of element `e` of a struct array.
    pub fn field(&self, e: u64, name: &str) -> Option<&MatValue> {
        match self {
            MatValue::Struct { fields, values, .. } => {
                let f = fields.iter().position(|x| x == name)?;
                values.get(usize::try_from(e).ok()? * fields.len() + f)
            }
            _ => None,
        }
    }
    /// The field names of a struct array.
    pub fn field_names(&self) -> &[String] {
        match self {
            MatValue::Struct { fields, .. } => fields,
            _ => &[],
        }
    }
}

/// One top-level variable.
#[derive(Debug, Clone)]
pub struct MatVariable {
    /// Its name.
    pub name: String,
    /// Its value.
    pub value: MatValue,
    /// The file region (offset, length) of the zlib stream when the variable is compressed;
    /// `None` when its stream is the file itself.
    pub compressed: Option<(u64, u64)>,
}

/// A parsed MAT-file.
#[derive(Debug, Clone)]
pub struct MatFile {
    /// The 116-byte header text (`MATLAB 5.0 MAT-file, Platform: …, Created on: …`).
    pub header_text: String,
    /// Written big-endian (`MI`) rather than little-endian (`IM`).
    pub big_endian: bool,
    /// The variables, in file order.
    pub variables: Vec<MatVariable>,
    /// A MAT 7.3 file (HDF5), parsed by `mat73`.
    pub hdf5: bool,
}

impl MatFile {
    /// The variable called `name`.
    pub fn variable(&self, name: &str) -> Option<&MatVariable> {
        self.variables.iter().find(|v| v.name == name)
    }
    /// The byte order of every value in the file.
    pub fn endian(&self) -> Endian {
        if self.big_endian {
            Endian::Big
        } else {
            Endian::Little
        }
    }
}

/// The MAT-file version a header text names: `5.0` (levels 5 to 7, this container) or `7.3`
/// (HDF5); `None` when the text is not a MAT-file header.
pub fn mat_version(head: &[u8]) -> Option<&'static str> {
    let t = latin1(head.get(..head.len().min(116)).unwrap_or(head));
    if t.starts_with("MATLAB 7.3 MAT-file") {
        Some("7.3")
    } else if t.starts_with("MATLAB 5.0 MAT-file") {
        Some("5.0")
    } else {
        None
    }
}

fn corrupt(at: u64, msg: impl Into<String>) -> Error {
    Error::corrupt_at(EEGLAB_FORMAT_ID, at, msg)
}

/// A byte stream with a position: a file region, or an inflated zlib stream.
trait Stream {
    fn read_into(&mut self, buf: &mut [u8]) -> Result<()>;
    fn skip(&mut self, n: u64) -> Result<()>;
    fn pos(&self) -> u64;
}

struct FileStream<'a> {
    f: &'a mut SourceFile,
    path: &'a Path,
    pos: u64,
    end: u64,
}

impl Stream for FileStream<'_> {
    fn read_into(&mut self, buf: &mut [u8]) -> Result<()> {
        let n = buf.len() as u64;
        if self.pos.checked_add(n).is_none_or(|e| e > self.end) {
            return Err(corrupt(
                self.pos,
                "an element runs past the end of the file",
            ));
        }
        self.f
            .seek(SeekFrom::Start(self.pos))
            .map_err(|e| Error::io(self.path, e))?;
        self.f
            .read_exact(buf)
            .map_err(|e| Error::io(self.path, e))?;
        self.pos += n;
        Ok(())
    }
    fn skip(&mut self, n: u64) -> Result<()> {
        let to = self.pos.checked_add(n).filter(|e| *e <= self.end);
        self.pos =
            to.ok_or_else(|| corrupt(self.pos, "an element runs past the end of the file"))?;
        Ok(())
    }
    fn pos(&self) -> u64 {
        self.pos
    }
}

struct ZStream<R: Read> {
    z: flate2::read::ZlibDecoder<R>,
    pos: u64,
    origin: u64,
}

impl<R: Read> Stream for ZStream<R> {
    fn read_into(&mut self, buf: &mut [u8]) -> Result<()> {
        self.z.read_exact(buf).map_err(|_| {
            corrupt(
                self.origin,
                format!(
                    "the compressed variable ends or fails to inflate at byte {}",
                    self.pos
                ),
            )
        })?;
        self.pos += buf.len() as u64;
        Ok(())
    }
    fn skip(&mut self, n: u64) -> Result<()> {
        let got = std::io::copy(&mut (&mut self.z).take(n), &mut std::io::sink())
            .map_err(|_| corrupt(self.origin, "the compressed variable fails to inflate"))?;
        if got < n {
            return Err(corrupt(
                self.origin,
                format!("the compressed variable ends at byte {}", self.pos + got),
            ));
        }
        self.pos += n;
        Ok(())
    }
    fn pos(&self) -> u64 {
        self.pos
    }
}

struct Tag {
    ty: MiType,
    len: u64,
    small: Option<[u8; 4]>,
}

struct Parser {
    endian: Endian,
    nodes: usize,
}

impl Parser {
    fn u32(&self, b: [u8; 4]) -> u32 {
        match self.endian {
            Endian::Little => u32::from_le_bytes(b),
            Endian::Big => u32::from_be_bytes(b),
        }
    }

    fn tag(&self, s: &mut dyn Stream) -> Result<Tag> {
        let mut b = [0u8; 8];
        s.read_into(&mut b)?;
        let first = self.u32([b[0], b[1], b[2], b[3]]);
        let second = [b[4], b[5], b[6], b[7]];
        if first >> 16 != 0 {
            // small data element: the byte count in the upper half, the data in the next 4
            return Ok(Tag {
                ty: first & 0xFFFF,
                len: u64::from(first >> 16).min(4),
                small: Some(second),
            });
        }
        Ok(Tag {
            ty: first,
            len: u64::from(self.u32(second)),
            small: None,
        })
    }

    /// The bytes of a tag's data, padding skipped.
    fn data(&self, s: &mut dyn Stream, t: &Tag, max: u64) -> Result<Vec<u8>> {
        if let Some(b) = t.small {
            return Ok(b[..t.len as usize].to_vec());
        }
        if t.len > max {
            return Err(corrupt(
                s.pos(),
                format!("an element of {} bytes is too large here", t.len),
            ));
        }
        let v = read_vec(s, t.len)?;
        s.skip(pad(t.len))?;
        Ok(v)
    }

    fn values(&self, ty: MiType, b: &[u8]) -> Vec<f64> {
        let n = mi_size(ty).max(1) as usize;
        let le = matches!(self.endian, Endian::Little);
        b.chunks_exact(n)
            .map(|x| match ty {
                MI_INT8 => f64::from(x[0] as i8),
                MI_UINT8 | MI_UTF8 => f64::from(x[0]),
                MI_INT16 => {
                    let a = [x[0], x[1]];
                    f64::from(if le {
                        i16::from_le_bytes(a)
                    } else {
                        i16::from_be_bytes(a)
                    })
                }
                MI_UINT16 | MI_UTF16 => {
                    let a = [x[0], x[1]];
                    f64::from(if le {
                        u16::from_le_bytes(a)
                    } else {
                        u16::from_be_bytes(a)
                    })
                }
                MI_INT32 => {
                    let a = [x[0], x[1], x[2], x[3]];
                    f64::from(if le {
                        i32::from_le_bytes(a)
                    } else {
                        i32::from_be_bytes(a)
                    })
                }
                MI_UINT32 | MI_UTF32 => {
                    let a = [x[0], x[1], x[2], x[3]];
                    f64::from(if le {
                        u32::from_le_bytes(a)
                    } else {
                        u32::from_be_bytes(a)
                    })
                }
                MI_SINGLE => {
                    let a = [x[0], x[1], x[2], x[3]];
                    f64::from(if le {
                        f32::from_le_bytes(a)
                    } else {
                        f32::from_be_bytes(a)
                    })
                }
                _ => {
                    let a: [u8; 8] = x.try_into().unwrap_or([0; 8]);
                    match ty {
                        MI_DOUBLE => {
                            if le {
                                f64::from_le_bytes(a)
                            } else {
                                f64::from_be_bytes(a)
                            }
                        }
                        MI_INT64 => {
                            (if le {
                                i64::from_le_bytes(a)
                            } else {
                                i64::from_be_bytes(a)
                            }) as f64
                        }
                        _ => {
                            (if le {
                                u64::from_le_bytes(a)
                            } else {
                                u64::from_be_bytes(a)
                            }) as f64
                        }
                    }
                }
            })
            .collect()
    }

    fn text(&self, ty: MiType, b: &[u8]) -> Vec<char> {
        match ty {
            MI_UTF8 => String::from_utf8_lossy(b).chars().collect(),
            MI_UINT16 | MI_UTF16 | MI_INT16 => {
                let units = self.values(MI_UINT16, b).into_iter().map(|v| v as u16);
                char::decode_utf16(units)
                    .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
                    .collect()
            }
            MI_UINT32 | MI_UTF32 | MI_INT32 => self
                .values(MI_UINT32, b)
                .into_iter()
                .map(|v| char::from_u32(v as u32).unwrap_or(char::REPLACEMENT_CHARACTER))
                .collect(),
            _ => latin1(b).chars().collect(),
        }
    }

    /// An `miMATRIX` element's body of `len` bytes.
    fn matrix(&mut self, s: &mut dyn Stream, len: u64, depth: usize) -> Result<(String, MatValue)> {
        self.nodes += 1;
        if self.nodes > MAX_MAT_NODES || depth > MAX_MAT_DEPTH {
            return Err(Error::unsupported(
                EEGLAB_FORMAT_ID,
                "MAT-files nested this deeply or with this many arrays",
                "The file may be damaged; the limits are in docs/formats/eeglab.md.",
            ));
        }
        let start = s.pos();
        let end = start
            .checked_add(len)
            .ok_or_else(|| corrupt(start, "an array's size overflows"))?;
        if len == 0 {
            let empty = MatNumbers {
                stored: MI_DOUBLE,
                count: 0,
                values: Some(Vec::new()),
                at: start,
            };
            return Ok((
                String::new(),
                MatValue::Numeric {
                    class: 6,
                    dims: vec![0, 0],
                    data: empty,
                },
            ));
        }
        let flags_tag = self.tag(s)?;
        let flags = self.data(s, &flags_tag, 64)?;
        if flags.len() < 8 {
            return Err(corrupt(start, "an array's flags are cut short"));
        }
        let fc = self.u32([flags[0], flags[1], flags[2], flags[3]]);
        let class = (fc & 0xFF) as u8;
        let complex = fc & 0x800 != 0;
        let dims_tag = self.tag(s)?;
        let db = self.data(s, &dims_tag, 1 << 16)?;
        let dims: Vec<u64> = self
            .values(MI_INT32, &db)
            .into_iter()
            .map(|d| if d < 0.0 { 0 } else { d as u64 })
            .collect();
        let count = dims
            .iter()
            .try_fold(1u64, |a, &d| a.checked_mul(d))
            .filter(|n| *n <= MAX_MAT_ELEMENTS)
            .ok_or_else(|| corrupt(start, "an array's dimensions are too large"))?;
        let name_tag = self.tag(s)?;
        let name = latin1(&self.data(s, &name_tag, 1 << 16)?)
            .trim_end_matches('\0')
            .to_string();
        let value = match class {
            6..=15 => {
                let t = self.tag(s)?;
                let size = mi_size(t.ty);
                if size == 0 {
                    return Err(corrupt(
                        s.pos(),
                        format!("array {name:?} stores values as type {}", t.ty),
                    ));
                }
                let at = s.pos();
                let n = t.len / size;
                let values = if t.small.is_some() || t.len <= MAX_MAT_INLINE {
                    let b = self.data(s, &t, MAX_MAT_INLINE)?;
                    Some(self.values(t.ty, &b))
                } else {
                    s.skip(t.len)?;
                    s.skip(pad(t.len))?;
                    None
                };
                if complex && s.pos() < end {
                    let t = self.tag(s)?;
                    if t.small.is_none() {
                        s.skip(t.len)?;
                        s.skip(pad(t.len))?;
                    }
                }
                MatValue::Numeric {
                    class,
                    dims,
                    data: MatNumbers {
                        stored: t.ty,
                        count: n,
                        values,
                        at,
                    },
                }
            }
            4 => {
                let t = self.tag(s)?;
                let b = self.data(s, &t, MAX_MAT_TEXT)?;
                let chars = self.text(t.ty, &b);
                // rows only as many as the characters stored allow (a declared row count is not trusted)
                let rows = usize::try_from(dims.first().copied().unwrap_or(1))
                    .unwrap_or(1)
                    .clamp(1, chars.len().max(1));
                let text = if rows == 1 {
                    chars.into_iter().collect()
                } else {
                    let cols = chars.len() / rows;
                    (0..rows)
                        .map(|r| {
                            (0..cols)
                                .filter_map(|c| chars.get(r + rows * c))
                                .collect::<String>()
                                .trim_end()
                                .to_string()
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                MatValue::Char { dims, text }
            }
            1 => {
                let mut items = Vec::new();
                for _ in 0..count {
                    items.push(self.child(s, depth)?);
                }
                MatValue::Cell { dims, items }
            }
            2 | 3 => {
                let class_name = if class == 3 {
                    let t = self.tag(s)?;
                    Some(
                        latin1(&self.data(s, &t, 1 << 16)?)
                            .trim_end_matches('\0')
                            .to_string(),
                    )
                } else {
                    None
                };
                let lt = self.tag(s)?;
                let lb = self.data(s, &lt, 64)?;
                let flen = self.values(MI_INT32, &lb).first().copied().unwrap_or(0.0) as usize;
                let nt = self.tag(s)?;
                let nb = self.data(s, &nt, 1 << 24)?;
                let fields: Vec<String> = if flen == 0 {
                    Vec::new()
                } else {
                    nb.chunks(flen)
                        .map(|c| latin1(c).trim_end_matches('\0').to_string())
                        .collect()
                };
                let mut values = Vec::new();
                if !fields.is_empty() {
                    for _ in 0..count {
                        for _ in 0..fields.len() {
                            values.push(self.child(s, depth)?);
                        }
                    }
                }
                MatValue::Struct {
                    dims,
                    fields,
                    values,
                    class_name,
                }
            }
            c => MatValue::Other { class: c },
        };
        let here = s.pos();
        if here > end {
            return Err(corrupt(
                start,
                format!("array {name:?} runs past its element"),
            ));
        }
        s.skip(end - here)?;
        Ok((name, value))
    }

    fn child(&mut self, s: &mut dyn Stream, depth: usize) -> Result<MatValue> {
        let t = self.tag(s)?;
        if t.ty != MI_MATRIX || t.small.is_some() {
            return Err(corrupt(
                s.pos(),
                format!("expected an array element, found type {}", t.ty),
            ));
        }
        let (_, v) = self.matrix(s, t.len, depth + 1)?;
        Ok(v)
    }
}

/// Read `n` bytes, growing the buffer as bytes arrive, so a byte count from the file cannot
/// reserve memory the file does not hold.
fn read_vec(s: &mut dyn Stream, n: u64) -> Result<Vec<u8>> {
    const STEP: u64 = 1 << 20;
    let mut v = Vec::with_capacity(n.min(STEP) as usize);
    let mut left = n;
    while left > 0 {
        let k = left.min(STEP) as usize;
        let at = v.len();
        v.resize(at + k, 0);
        s.read_into(&mut v[at..])?;
        left -= k as u64;
    }
    Ok(v)
}

fn pad(len: u64) -> u64 {
    (8 - len % 8) % 8
}

/// Parse a level-5 MAT-file's header and every variable (large numeric arrays only located).
#[allow(clippy::many_single_char_names)]
pub fn parse_mat(f: &mut SourceFile, path: &Path, file_len: u64) -> Result<MatFile> {
    let mut head = vec![0u8; MAT_HEADER_LEN as usize];
    if file_len < MAT_HEADER_LEN {
        return Err(corrupt(0, "the file is shorter than a MAT-file header"));
    }
    f.seek(SeekFrom::Start(0)).map_err(|e| Error::io(path, e))?;
    f.read_exact(&mut head).map_err(|e| Error::io(path, e))?;
    match mat_version(&head) {
        Some("5.0") => {}
        Some(v) => {
            return Err(Error::unsupported(
                EEGLAB_FORMAT_ID,
                format!("MAT-file version {v} (HDF5) in the level-5 parser"),
                "Open the file with the EEGLAB reader, which reads MAT 7.3 files through HDF5.",
            ));
        }
        None => {
            return Err(corrupt(
                0,
                "the file does not start with a MATLAB 5.0 MAT-file header",
            ));
        }
    }
    let big_endian = match &head[126..128] {
        b"IM" => false,
        b"MI" => true,
        _ => {
            return Err(corrupt(
                126,
                "the MAT-file endian indicator is neither IM nor MI",
            ));
        }
    };
    let mut p = Parser {
        endian: if big_endian {
            Endian::Big
        } else {
            Endian::Little
        },
        nodes: 0,
    };
    let mut variables = Vec::new();
    let mut s = FileStream {
        f,
        path,
        pos: MAT_HEADER_LEN,
        end: file_len,
    };
    while s.pos + 8 <= file_len {
        let at = s.pos;
        let t = p.tag(&mut s)?;
        match t.ty {
            MI_MATRIX if t.small.is_none() => {
                let (name, value) = p.matrix(&mut s, t.len, 0)?;
                variables.push(MatVariable {
                    name,
                    value,
                    compressed: None,
                });
            }
            MI_COMPRESSED if t.small.is_none() => {
                let body = s.pos;
                let end = body
                    .checked_add(t.len)
                    .filter(|e| *e <= file_len)
                    .ok_or_else(|| {
                        corrupt(at, "a compressed variable runs past the end of the file")
                    })?;
                let (name, value) = {
                    let mut z = open_z(s.f, path, body, t.len)?;
                    let inner = p.tag(&mut z)?;
                    if inner.ty != MI_MATRIX {
                        return Err(corrupt(at, "a compressed element does not hold an array"));
                    }
                    p.matrix(&mut z, inner.len, 0)?
                };
                variables.push(MatVariable {
                    name,
                    value,
                    compressed: Some((body, t.len)),
                });
                s.pos = end;
            }
            _ => {
                if t.small.is_none() {
                    s.skip(t.len)?;
                    s.skip(pad(t.len))?;
                }
            }
        }
    }
    Ok(MatFile {
        header_text: latin1(&head[..116])
            .trim_end_matches(['\0', ' '])
            .to_string(),
        big_endian,
        variables,
        hdf5: false,
    })
}

fn open_z<'a>(
    f: &'a mut SourceFile,
    path: &Path,
    at: u64,
    len: u64,
) -> Result<ZStream<std::io::Take<&'a mut SourceFile>>> {
    f.seek(SeekFrom::Start(at))
        .map_err(|e| Error::io(path, e))?;
    Ok(ZStream {
        z: flate2::read::ZlibDecoder::new(f.take(len)),
        pos: 0,
        origin: at,
    })
}

/// Read `n` stored values of type `ty` at `at` in a variable's stream as raw bytes.
pub fn read_stream(
    f: &mut SourceFile,
    path: &Path,
    file_len: u64,
    var: &MatVariable,
    at: u64,
    bytes: u64,
) -> Result<Vec<u8>> {
    if usize::try_from(bytes).is_err() {
        return Err(corrupt(at, "a read is too large for memory"));
    }
    match var.compressed {
        None => {
            let mut s = FileStream {
                f,
                path,
                pos: at,
                end: file_len,
            };
            read_vec(&mut s, bytes)
        }
        Some((off, len)) => {
            let mut z = open_z(f, path, off, len)?;
            z.skip(at)?;
            read_vec(&mut z, bytes)
        }
    }
}

/// Decode raw stored values (as [`read_stream`] returns them) to f64.
pub fn decode_values(big_endian: bool, ty: MiType, b: &[u8]) -> Vec<f64> {
    let p = Parser {
        endian: if big_endian {
            Endian::Big
        } else {
            Endian::Little
        },
        nodes: 0,
    };
    p.values(ty, b)
}
