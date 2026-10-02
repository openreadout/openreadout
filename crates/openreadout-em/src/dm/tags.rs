//! The DM3/DM4 tag tree: header, tag groups and data tags (see `docs/formats/dm.md`).
//!
//! Structure words (version, lengths, counts, names, info arrays) are big-endian; tag values use
//! the byte order the header's byte-order word gives (1 = little-endian).

use serde_json::{Map, Value};

use openreadout_core::bytes::{Endian, be_u16, be_u32, be_u64, latin1};

use crate::util::num;

/// Deepest group nesting we follow.
pub const MAX_DEPTH: usize = 64;
/// Arrays with at most this many elements are decoded into the tree; longer ones are referenced.
pub const INLINE_ARRAY_LIMIT: u64 = 4096;

/// Encoded type of a tag value (the first word of a data tag's info array).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagType {
    I16,
    I32,
    U16,
    U32,
    F32,
    F64,
    Bool,
    Char,
    Octet,
    I64,
    U64,
    Struct,
    Text,
    Array,
    Unknown(i64),
}

impl TagType {
    pub fn from_code(c: i64) -> TagType {
        match c {
            2 => TagType::I16,
            3 => TagType::I32,
            4 => TagType::U16,
            5 => TagType::U32,
            6 => TagType::F32,
            7 => TagType::F64,
            8 => TagType::Bool,
            9 => TagType::Char,
            10 => TagType::Octet,
            11 => TagType::I64,
            12 => TagType::U64,
            15 => TagType::Struct,
            18 => TagType::Text,
            20 => TagType::Array,
            other => TagType::Unknown(other),
        }
    }

    /// Bytes of one scalar value; `None` for compound and unknown types.
    pub fn width(self) -> Option<u64> {
        Some(match self {
            TagType::Bool | TagType::Char | TagType::Octet => 1,
            TagType::I16 | TagType::U16 => 2,
            TagType::I32 | TagType::U32 | TagType::F32 => 4,
            TagType::F64 | TagType::I64 | TagType::U64 => 8,
            _ => return None,
        })
    }
}

/// Element type of an array tag: a scalar type or a struct of scalar fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElementType {
    Scalar(TagType),
    Struct(Vec<TagType>),
}

impl ElementType {
    pub fn width(&self) -> Option<u64> {
        match self {
            ElementType::Scalar(t) => t.width(),
            ElementType::Struct(f) => f.iter().try_fold(0u64, |a, t| Some(a + t.width()?)),
        }
    }
}

/// The value of a data tag.
#[derive(Debug, Clone, PartialEq)]
pub enum TagValue {
    /// A scalar or a decoded small array/struct/string.
    Json(Value),
    /// An array: element type, element count, absolute byte offset and byte length in the file,
    /// and its decoded value when it has at most `INLINE_ARRAY_LIMIT` elements.
    Array {
        element: ElementType,
        count: u64,
        offset: u64,
        length: u64,
        inline: Option<Value>,
    },
}

/// A tag directory entry.
#[derive(Debug, Clone, PartialEq)]
pub enum Tag {
    Group(TagGroup),
    Data(TagValue),
}

/// A tag group (directory): entries in file order, names empty when unnamed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TagGroup {
    pub entries: Vec<(String, Tag)>,
}

impl TagGroup {
    /// First entry named `name`.
    pub fn get(&self, name: &str) -> Option<&Tag> {
        self.entries.iter().find(|(n, _)| n == name).map(|(_, t)| t)
    }

    /// Sub-group named `name`.
    pub fn group(&self, name: &str) -> Option<&TagGroup> {
        match self.get(name)? {
            Tag::Group(g) => Some(g),
            Tag::Data(_) => None,
        }
    }

    /// Follow a `.`-separated path of group names ending in a data tag or group.
    pub fn path(&self, path: &str) -> Option<&Tag> {
        let mut g = self;
        let mut parts = path.split('.').peekable();
        while let Some(p) = parts.next() {
            let t = g.get(p)?;
            if parts.peek().is_none() {
                return Some(t);
            }
            g = match t {
                Tag::Group(sub) => sub,
                Tag::Data(_) => return None,
            };
        }
        None
    }

    /// JSON value of the data tag at `path`, if it was decoded.
    pub fn value(&self, path: &str) -> Option<&Value> {
        match self.path(path)? {
            Tag::Data(TagValue::Json(v)) => Some(v),
            Tag::Data(TagValue::Array {
                inline: Some(v), ..
            }) => Some(v),
            _ => None,
        }
    }

    /// Unnamed sub-groups, in order (e.g. the entries of `ImageList`).
    pub fn children(&self) -> Vec<&TagGroup> {
        self.entries
            .iter()
            .filter_map(|(_, t)| match t {
                Tag::Group(g) => Some(g),
                Tag::Data(_) => None,
            })
            .collect()
    }

    /// The tree as JSON: groups with names become objects, groups of unnamed entries arrays.
    pub fn to_json(&self) -> Value {
        let unnamed = !self.entries.is_empty() && self.entries.iter().all(|(n, _)| n.is_empty());
        let conv = |t: &Tag| match t {
            Tag::Group(g) => g.to_json(),
            Tag::Data(TagValue::Json(v)) => v.clone(),
            Tag::Data(TagValue::Array {
                inline: Some(v), ..
            }) => v.clone(),
            Tag::Data(TagValue::Array {
                element,
                count,
                offset,
                length,
                inline: None,
            }) => serde_json::json!({
                "array_of": element_name(element), "count": count, "offset": offset, "bytes": length
            }),
        };
        if unnamed {
            return Value::Array(self.entries.iter().map(|(_, t)| conv(t)).collect());
        }
        let mut m = Map::new();
        for (k, (n, t)) in self.entries.iter().enumerate() {
            let key = if n.is_empty() {
                format!("#{k}")
            } else {
                n.clone()
            };
            m.entry(key).or_insert_with(|| conv(t));
        }
        Value::Object(m)
    }
}

fn element_name(e: &ElementType) -> String {
    match e {
        ElementType::Scalar(t) => format!("{t:?}").to_lowercase(),
        ElementType::Struct(f) => format!(
            "struct({})",
            f.iter()
                .map(|t| format!("{t:?}").to_lowercase())
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

/// The DM file header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DmHeader {
    /// 3 or 4.
    pub version: u32,
    /// Root-length field: bytes of the root tag group (file length − header − 8 end bytes).
    pub root_length: u64,
    /// Byte-order word: 1 = little-endian tag values.
    pub little_endian: bool,
    /// Bytes before the root tag group (12 for DM3, 16 for DM4).
    pub header_len: u64,
}

impl DmHeader {
    pub fn parse(b: &[u8]) -> Option<DmHeader> {
        let version = be_u32(b, 0)?;
        let (root_length, order_at, header_len) = match version {
            3 => (u64::from(be_u32(b, 4)?), 8, 12),
            4 => (be_u64(b, 4)?, 12, 16),
            _ => return None,
        };
        let order = be_u32(b, order_at)?;
        if order > 1 {
            return None;
        }
        Some(DmHeader {
            version,
            root_length,
            little_endian: order == 1,
            header_len,
        })
    }
}

/// Structural problems found while parsing (reported by `check`).
#[derive(Debug, Clone, PartialEq)]
pub struct ParseIssue {
    pub offset: u64,
    pub message: String,
}

/// Random access to the file for the parser (a `Blob`, or bytes in memory for tests).
pub(crate) trait ByteSource {
    /// Up to `n` bytes at `offset` (fewer at the end of the data).
    fn read_upto(&mut self, offset: u64, n: u64) -> Option<Vec<u8>>;
}

impl ByteSource for crate::util::Blob {
    fn read_upto(&mut self, offset: u64, n: u64) -> Option<Vec<u8>> {
        crate::util::Blob::read_upto(self, offset, n).ok()
    }
}

impl ByteSource for &[u8] {
    fn read_upto(&mut self, offset: u64, n: u64) -> Option<Vec<u8>> {
        let s = usize::try_from(offset).ok()?.min(self.len());
        let e = s.saturating_add(usize::try_from(n).ok()?).min(self.len());
        Some(self[s..e].to_vec())
    }
}

/// Bytes fetched per refill of the parser's window.
const WINDOW: u64 = 1 << 20;

/// Streaming parser for the tag directory: reads through a window, skips large arrays by offset.
pub(crate) struct Parser<'a, S: ByteSource> {
    src: &'a mut S,
    buf: Vec<u8>,
    buf_start: u64,
    pub(crate) pos: u64,
    dm4: bool,
    e: Endian,
    pub(crate) issues: Vec<ParseIssue>,
    pub(crate) complete: bool,
}

impl<'a, S: ByteSource> Parser<'a, S> {
    pub(crate) fn new(src: &'a mut S, h: &DmHeader) -> Self {
        Parser {
            src,
            buf: Vec::new(),
            buf_start: 0,
            pos: h.header_len,
            dm4: h.version == 4,
            e: if h.little_endian {
                Endian::Little
            } else {
                Endian::Big
            },
            issues: Vec::new(),
            complete: true,
        }
    }

    fn fail(&mut self, msg: impl Into<String>) {
        self.issues.push(ParseIssue {
            offset: self.pos,
            message: msg.into(),
        });
        self.complete = false;
    }

    /// The next `n` bytes, advancing past them; `None` at the end of the file.
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let n64 = n as u64;
        let end = self.pos.checked_add(n64)?;
        let have = self.pos >= self.buf_start && end <= self.buf_start + self.buf.len() as u64;
        if !have {
            self.buf = self.src.read_upto(self.pos, n64.max(WINDOW))?;
            self.buf_start = self.pos;
            if (self.buf.len() as u64) < n64 {
                return None;
            }
        }
        let s = usize::try_from(self.pos - self.buf_start).ok()?;
        self.pos = end;
        self.buf.get(s..s + n)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    fn be_u16(&mut self) -> Option<u16> {
        self.take(2).and_then(|b| be_u16(b, 0))
    }

    /// A structure word: u32 in DM3, u64 in DM4 (big-endian).
    fn word(&mut self) -> Option<u64> {
        if self.dm4 {
            self.take(8).and_then(|b| be_u64(b, 0))
        } else {
            self.take(4).and_then(|b| be_u32(b, 0)).map(u64::from)
        }
    }

    /// Parse the root group.
    pub(crate) fn root(&mut self) -> TagGroup {
        self.group_body(0).unwrap_or_default()
    }

    fn group_body(&mut self, depth: usize) -> Option<TagGroup> {
        let _sorted = self.u8()?;
        let _open = self.u8()?;
        let n = self.word()?;
        let mut g = TagGroup::default();
        for _ in 0..n {
            if !self.complete {
                break;
            }
            let start = self.pos;
            let Some(kind) = self.u8() else {
                self.fail("tag directory ends early (file truncated?)");
                break;
            };
            if kind == 0 {
                self.pos = start;
                self.fail(format!(
                    "end marker inside a group that declares {n} entries"
                ));
                break;
            }
            let Some(name) = self.name() else {
                self.fail("tag name runs past the end of the file");
                break;
            };
            let tag_len = if self.dm4 {
                let Some(l) = self.word() else {
                    self.fail("tag length runs past the end of the file");
                    break;
                };
                Some(l)
            } else {
                None
            };
            let body = self.pos;
            let tag = match kind {
                20 if depth >= MAX_DEPTH => {
                    self.fail("tag groups nested too deeply");
                    break;
                }
                20 => match self.group_body(depth + 1) {
                    Some(sub) if self.complete => Tag::Group(sub),
                    Some(sub) => {
                        g.entries.push((name, Tag::Group(sub)));
                        break;
                    }
                    None => {
                        self.fail(format!("tag group '{name}' is incomplete"));
                        break;
                    }
                },
                21 => {
                    let Some(v) = self.data() else {
                        self.pos = start;
                        self.fail(format!("data tag '{name}' is malformed or truncated"));
                        break;
                    };
                    Tag::Data(v)
                }
                other => {
                    self.pos = start;
                    self.fail(format!("unknown tag kind {other}"));
                    break;
                }
            };
            // DM4 data tags carry their length: check it, and resynchronise on disagreement.
            if kind == 21
                && let Some(len) = tag_len
                && let Some(end) = body.checked_add(len)
                && end != self.pos
            {
                self.issues.push(ParseIssue {
                    offset: start,
                    message: format!(
                        "tag '{name}' declares {len} bytes but its content spans {}",
                        self.pos - body
                    ),
                });
                self.pos = end;
            }
            g.entries.push((name, tag));
        }
        Some(g)
    }

    fn name(&mut self) -> Option<String> {
        let n = usize::from(self.be_u16()?);
        let raw = self.take(n)?;
        Some(match std::str::from_utf8(raw) {
            Ok(s) => s.to_string(),
            Err(_) => latin1(raw),
        })
    }

    fn data(&mut self) -> Option<TagValue> {
        if self.take(4)? != b"%%%%" {
            return None;
        }
        let ninfo = self.word()?;
        if ninfo == 0 || ninfo > 1 << 16 {
            return None;
        }
        let mut info = Vec::with_capacity(usize::try_from(ninfo).ok()?);
        for _ in 0..ninfo {
            let w = self.word()?;
            info.push(if self.dm4 {
                w as i64
            } else {
                i64::from(w as u32 as i32)
            });
        }
        let t = TagType::from_code(info[0]);
        match t {
            TagType::Struct => {
                let fields = struct_fields(&info[1..])?;
                let mut out = Vec::with_capacity(fields.len());
                for f in fields {
                    out.push(self.scalar(f)?);
                }
                Some(TagValue::Json(Value::Array(out)))
            }
            TagType::Array => {
                let (element, count) = array_spec(&info[1..])?;
                let length = element.width()?.checked_mul(count)?;
                let offset = self.pos;
                let inline = if count <= INLINE_ARRAY_LIMIT {
                    let e = self.e;
                    let raw = self.take(usize::try_from(length).ok()?)?;
                    Some(array_json(raw, &element, e))
                } else {
                    // Skip without reading; the end of the file is checked by the caller's next read.
                    self.pos = offset.checked_add(length)?;
                    None
                };
                Some(TagValue::Array {
                    element,
                    count,
                    offset,
                    length,
                    inline,
                })
            }
            TagType::Text => {
                // DM3 string tags: info[1] = length in bytes (UTF-16).
                let n = usize::try_from(*info.get(1)?).ok()?;
                let e = self.e;
                let raw = self.take(n)?;
                Some(TagValue::Json(Value::String(utf16(raw, e))))
            }
            _ => self.scalar(t).map(TagValue::Json),
        }
    }

    fn scalar(&mut self, t: TagType) -> Option<Value> {
        let w = usize::try_from(t.width()?).ok()?;
        let e = self.e;
        let raw = self.take(w)?;
        scalar_at(raw, 0, t, e)
    }
}

fn array_json(raw: &[u8], element: &ElementType, e: Endian) -> Value {
    match element {
        ElementType::Scalar(TagType::U16) => {
            // Short u16 arrays are UTF-16 text in DM files.
            let s = utf16(raw, e);
            let texty = !s.is_empty()
                && !s.contains('\u{fffd}')
                && s.chars()
                    .all(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'));
            if texty || raw.is_empty() {
                return Value::String(s);
            }
            Value::Array(
                raw.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| Value::from(e.u16(c, 0).unwrap_or(0)))
                    .collect(),
            )
        }
        ElementType::Scalar(t) => {
            let w = usize::try_from(t.width().unwrap_or(1)).unwrap_or(1);
            Value::Array(
                raw.chunks_exact(w)
                    .map(|c| scalar_at(c, 0, *t, e).unwrap_or(Value::Null))
                    .collect(),
            )
        }
        ElementType::Struct(fields) => {
            let w = usize::try_from(element.width().unwrap_or(1)).unwrap_or(1);
            Value::Array(
                raw.chunks_exact(w)
                    .map(|item| {
                        let mut pos = 0usize;
                        Value::Array(
                            fields
                                .iter()
                                .map(|f| {
                                    let v = scalar_at(item, pos, *f, e).unwrap_or(Value::Null);
                                    pos += usize::try_from(f.width().unwrap_or(1)).unwrap_or(1);
                                    v
                                })
                                .collect(),
                        )
                    })
                    .collect(),
            )
        }
    }
}
fn utf16(raw: &[u8], e: Endian) -> String {
    openreadout_core::bytes::utf16(raw, e)
        .trim_end_matches('\0')
        .to_string()
}

fn scalar_at(b: &[u8], pos: usize, t: TagType, e: Endian) -> Option<Value> {
    Some(match t {
        TagType::I16 => Value::from(e.i16(b, pos)?),
        TagType::I32 => Value::from(e.i32(b, pos)?),
        TagType::U16 => Value::from(e.u16(b, pos)?),
        TagType::U32 => Value::from(e.u32(b, pos)?),
        TagType::F32 => num(f64::from(e.f32(b, pos)?)),
        TagType::F64 => num(e.f64(b, pos)?),
        TagType::Bool => Value::Bool(*b.get(pos)? != 0),
        TagType::Char => Value::String(char::from(*b.get(pos)?).to_string()),
        TagType::Octet => Value::from(*b.get(pos)?),
        TagType::I64 => Value::from(e.i64(b, pos)?),
        TagType::U64 => Value::from(e.u64(b, pos)?),
        _ => return None,
    })
}

/// Struct info after the type word: name length, field count, then (name length, type) pairs.
fn struct_fields(info: &[i64]) -> Option<Vec<TagType>> {
    let n = usize::try_from(*info.get(1)?).ok()?;
    if n > 1024 || info.len() < 2 + 2 * n {
        return None;
    }
    let fields: Vec<TagType> = (0..n)
        .map(|k| TagType::from_code(info[3 + 2 * k]))
        .collect();
    fields.iter().all(|t| t.width().is_some()).then_some(fields)
}

/// Array info after the type word: element type (or a struct spec), then the element count.
fn array_spec(info: &[i64]) -> Option<(ElementType, u64)> {
    let count = u64::try_from(*info.last()?).ok()?;
    let elem = TagType::from_code(*info.first()?);
    let element = match elem {
        TagType::Struct => ElementType::Struct(struct_fields(&info[1..])?),
        t if t.width().is_some() && info.len() == 2 => ElementType::Scalar(t),
        _ => return None,
    };
    Some((element, count))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Build a tiny DM file: `version` 3 or 4, little-endian, root entries given as raw bytes.
    pub(crate) fn file(version: u32, root: &[Vec<u8>]) -> Vec<u8> {
        let mut body = vec![1u8, 0];
        word(&mut body, version, root.len() as u64);
        for r in root {
            body.extend_from_slice(r);
        }
        let mut out = version.to_be_bytes().to_vec();
        if version == 4 {
            out.extend_from_slice(&(body.len() as u64).to_be_bytes());
        } else {
            out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        }
        out.extend_from_slice(&1u32.to_be_bytes());
        out.extend_from_slice(&body);
        out.extend_from_slice(&[0u8; 8]);
        out
    }

    pub(crate) fn word(b: &mut Vec<u8>, version: u32, v: u64) {
        if version == 4 {
            b.extend_from_slice(&v.to_be_bytes());
        } else {
            b.extend_from_slice(&(v as u32).to_be_bytes());
        }
    }

    /// A data tag with info words `info` and little-endian value bytes `value`.
    pub(crate) fn data(version: u32, name: &str, info: &[u64], value: &[u8]) -> Vec<u8> {
        let mut t = vec![21u8];
        t.extend_from_slice(&(name.len() as u16).to_be_bytes());
        t.extend_from_slice(name.as_bytes());
        let mut body = b"%%%%".to_vec();
        word(&mut body, version, info.len() as u64);
        for i in info {
            word(&mut body, version, *i);
        }
        body.extend_from_slice(value);
        if version == 4 {
            t.extend_from_slice(&(body.len() as u64).to_be_bytes());
        }
        t.extend_from_slice(&body);
        t
    }

    /// A group tag containing `entries`.
    pub(crate) fn group(version: u32, name: &str, entries: &[Vec<u8>]) -> Vec<u8> {
        let mut t = vec![20u8];
        t.extend_from_slice(&(name.len() as u16).to_be_bytes());
        t.extend_from_slice(name.as_bytes());
        let mut body = vec![1u8, 0];
        word(&mut body, version, entries.len() as u64);
        for e in entries {
            body.extend_from_slice(e);
        }
        if version == 4 {
            t.extend_from_slice(&(body.len() as u64).to_be_bytes());
        }
        t.extend_from_slice(&body);
        t
    }

    #[test]
    fn parses_scalars_strings_structs_and_arrays() {
        for v in [3u32, 4] {
            let text: Vec<u8> = "nm".encode_utf16().flat_map(u16::to_le_bytes).collect();
            let b = file(
                v,
                &[
                    data(v, "Voltage", &[7], &200_000f64.to_le_bytes()),
                    data(v, "Units", &[20, 4, 2], &text),
                    data(
                        v,
                        "Pair",
                        &[15, 0, 2, 0, 3, 0, 3],
                        &[1, 0, 0, 0, 2, 0, 0, 0],
                    ),
                    group(v, "G", &[data(v, "", &[3], &5i32.to_le_bytes())]),
                ],
            );
            let h = DmHeader::parse(&b).unwrap();
            assert_eq!(h.version, v);
            let mut src: &[u8] = &b;
            let mut p = Parser::new(&mut src, &h);
            let root = p.root();
            assert!(p.complete, "{:?}", p.issues);
            assert!(p.issues.is_empty(), "{:?}", p.issues);
            assert_eq!(root.value("Voltage"), Some(&Value::from(200_000.0)));
            assert_eq!(root.value("Units"), Some(&Value::from("nm")));
            assert_eq!(root.value("Pair"), Some(&serde_json::json!([1, 2])));
            assert_eq!(root.group("G").unwrap().children().len(), 0);
            assert_eq!(root.to_json()["G"], serde_json::json!([5]));
        }
    }

    #[test]
    fn large_arrays_are_referenced_not_copied() {
        let values = vec![0u8; 8192];
        let b = file(4, &[data(4, "Data", &[20, 4, 4096], &values)]);
        let h = DmHeader::parse(&b).unwrap();
        let mut src: &[u8] = &b;
        let mut p = Parser::new(&mut src, &h);
        let root = p.root();
        assert!(p.complete);
        // 4096 u16 values that are all zero are not text: they stay numbers
        assert!(matches!(root.value("Data"), Some(Value::Array(_))));
        let values = vec![0u8; 8194];
        let b = file(4, &[data(4, "Data", &[20, 4, 4097], &values)]);
        let h = DmHeader::parse(&b).unwrap();
        let mut src: &[u8] = &b;
        let mut p = Parser::new(&mut src, &h);
        let root = p.root();
        match root.get("Data") {
            Some(Tag::Data(TagValue::Array {
                count,
                length,
                inline,
                ..
            })) => {
                assert_eq!((*count, *length), (4097, 8194));
                assert!(inline.is_none());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn truncation_is_reported() {
        let b = file(3, &[data(3, "Voltage", &[7], &200_000f64.to_le_bytes())]);
        let cut = &b[..b.len() - 14];
        let h = DmHeader::parse(cut).unwrap();
        let mut src: &[u8] = cut;
        let mut p = Parser::new(&mut src, &h);
        let _ = p.root();
        assert!(!p.complete);
        assert!(!p.issues.is_empty());
    }
}
