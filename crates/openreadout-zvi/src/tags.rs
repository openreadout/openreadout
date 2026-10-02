//! Typed values and tag lists inside ZVI streams, as read off hex dumps of corpus files.
//!
//! A value is a 2-byte type code followed by its data; the type codes are the OLE Automation
//! `VARENUM` numbers of Microsoft's public [MS-OAUT] specification (2 = 16-bit integer,
//! 3 = 32-bit integer, 5 = double, 7 = date, 8 = length-prefixed UTF-16 text, 65 = blob,
//! 69 = stored-object name, ...). A tag list (`Tags/Contents` streams) is a 32-bit version,
//! a 32-bit count, then `count` triples of (value, 32-bit tag id, 32-bit attribute).

use std::collections::BTreeMap;

use openreadout_core::bytes::{
    array, latin1, le_f32, le_f64, le_i16, le_i32, le_i64, le_u16, le_u32, le_u64, until_nul,
    utf16le_z,
};
use serde_json::{Value, json};

/// Longest text or blob accepted in a value (a guard against corrupt lengths).
const MAX_VALUE_BYTES: usize = 16 << 20;

/// One typed value.
#[derive(Debug, Clone, PartialEq)]
pub enum TagValue {
    /// Type codes 0 and 1: no data.
    Empty,
    Int(i64),
    Float(f64),
    /// Type code 7: days since 1899-12-30 (fractional).
    Date(f64),
    Bool(bool),
    Text(String),
    /// Blob-like payloads (length-prefixed): only their length is kept.
    Bytes(usize),
    /// Fixed 16-byte payloads (type codes 9, 13, 14).
    Opaque,
}

impl TagValue {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            TagValue::Int(v) => Some(*v as f64),
            TagValue::Float(v) | TagValue::Date(v) => Some(*v),
            _ => None,
        }
        .filter(|v| v.is_finite())
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            TagValue::Int(v) => Some(*v),
            TagValue::Float(v) if v.fract() == 0.0 && v.is_finite() => Some(*v as i64),
            _ => None,
        }
    }
    pub fn as_text(&self) -> Option<&str> {
        match self {
            TagValue::Text(s) => Some(s.as_str()).filter(|s| !s.trim().is_empty()),
            _ => None,
        }
    }
    /// JSON form for `info --view full`.
    pub fn to_json(&self) -> Value {
        match self {
            TagValue::Empty | TagValue::Opaque => Value::Null,
            TagValue::Int(v) => json!(v),
            TagValue::Float(v) | TagValue::Date(v) => {
                if v.is_finite() {
                    json!(v)
                } else {
                    Value::Null
                }
            }
            TagValue::Bool(b) => json!(b),
            TagValue::Text(s) => json!(s),
            TagValue::Bytes(n) => json!({"bytes": n}),
        }
    }
}

/// Parse one value at `at`; returns it and the offset after it.
pub fn read_value(b: &[u8], at: usize) -> Option<(TagValue, usize)> {
    let vt = le_u16(b, at)?;
    let d = at + 2;
    Some(match vt {
        0 | 1 => (TagValue::Empty, d),
        16 => (TagValue::Int(i64::from(*b.get(d)? as i8)), d + 1),
        17 => (TagValue::Int(i64::from(*b.get(d)?)), d + 1),
        2 => (TagValue::Int(i64::from(le_i16(b, d)?)), d + 2),
        18 => (TagValue::Int(i64::from(le_u16(b, d)?)), d + 2),
        11 => (TagValue::Bool(le_i16(b, d)? != 0), d + 2),
        3 | 10 | 22 => (TagValue::Int(i64::from(le_i32(b, d)?)), d + 4),
        19 | 23 => (TagValue::Int(i64::from(le_u32(b, d)?)), d + 4),
        4 => (TagValue::Float(f64::from(le_f32(b, d)?)), d + 4),
        5 => (TagValue::Float(le_f64(b, d)?), d + 8),
        6 => (TagValue::Float(le_i64(b, d)? as f64 / 10_000.0), d + 8),
        7 => (TagValue::Date(le_f64(b, d)?), d + 8),
        20 => (TagValue::Int(le_i64(b, d)?), d + 8),
        21 => (
            TagValue::Int(i64::try_from(le_u64(b, d)?).unwrap_or(i64::MAX)),
            d + 8,
        ),
        8 => {
            let n = le_u32(b, d)? as usize;
            if n > MAX_VALUE_BYTES {
                return None;
            }
            let raw = b.get(d + 4..d + 4 + n)?;
            (TagValue::Text(utf16le_z(raw)), d + 4 + n)
        }
        30 => {
            let n = le_u32(b, d)? as usize;
            if n > MAX_VALUE_BYTES {
                return None;
            }
            let raw = b.get(d + 4..d + 4 + n)?;
            (TagValue::Text(latin1(until_nul(raw))), d + 4 + n)
        }
        65..=70 => {
            let n = le_u32(b, d)? as usize;
            if n > MAX_VALUE_BYTES || b.len() < d + 4 + n {
                return None;
            }
            (TagValue::Bytes(n), d + 4 + n)
        }
        9 | 13 | 14 => {
            array::<16>(b, d)?;
            (TagValue::Opaque, d + 16)
        }
        _ => return None,
    })
}

/// One entry of a tag list.
#[derive(Debug, Clone, PartialEq)]
pub struct Tag {
    pub id: u32,
    pub value: TagValue,
    pub attribute: i64,
}

/// A parsed tag list: version and entries in file order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TagList {
    pub version: i64,
    pub tags: Vec<Tag>,
    /// Entries announced by the count but not parseable (truncated or unknown type code).
    pub unparsed: usize,
}

impl TagList {
    /// First entry with this id.
    pub fn get(&self, id: u32) -> Option<&TagValue> {
        self.tags.iter().find(|t| t.id == id).map(|t| &t.value)
    }
    pub fn f64(&self, id: u32) -> Option<f64> {
        self.get(id).and_then(TagValue::as_f64)
    }
    pub fn i64(&self, id: u32) -> Option<i64> {
        self.get(id).and_then(TagValue::as_i64)
    }
    pub fn text(&self, id: u32) -> Option<String> {
        self.get(id)
            .and_then(TagValue::as_text)
            .map(|s| s.trim().to_string())
    }
    /// `{"<id>": value}` for `info --view full`, ids in numeric order (a repeated id keeps its
    /// first value).
    pub fn to_json(&self) -> Value {
        let mut m = BTreeMap::new();
        for t in &self.tags {
            m.entry(t.id).or_insert_with(|| t.value.to_json());
        }
        Value::Object(m.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }
}

/// Parse a tag list; `None` when the stream does not start with a version and a count.
pub fn parse_tags(b: &[u8]) -> Option<TagList> {
    let (version, at) = read_value(b, 0)?;
    let (count, mut at) = read_value(b, at)?;
    let version = version.as_i64()?;
    let count = usize::try_from(count.as_i64()?).ok()?;
    let mut tags = Vec::with_capacity(count.min(4096));
    let mut unparsed = 0;
    for k in 0..count {
        let parsed = (|| {
            let (value, a) = read_value(b, at)?;
            let (id, a) = read_value(b, a)?;
            let (attr, a) = read_value(b, a)?;
            Some((value, id.as_i64()?, attr.as_i64().unwrap_or(0), a))
        })();
        let Some((value, id, attribute, a)) = parsed else {
            unparsed = count - k;
            break;
        };
        tags.push(Tag {
            id: u32::try_from(id).unwrap_or(u32::MAX),
            value,
            attribute,
        });
        at = a;
    }
    Some(TagList {
        version,
        tags,
        unparsed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_and_lists() {
        // version 0x20001000, count 2: (I4 1513, id 515, attr 0x88), (BSTR "GFP", id 1284, attr 0)
        let mut b = vec![3u8, 0];
        b.extend_from_slice(&0x2000_1000u32.to_le_bytes());
        b.extend_from_slice(&[3, 0, 2, 0, 0, 0]);
        b.extend_from_slice(&[3, 0]);
        b.extend_from_slice(&1513i32.to_le_bytes());
        b.extend_from_slice(&[3, 0, 3, 2, 0, 0, 3, 0, 0x88, 0, 0, 0]);
        b.extend_from_slice(&[8, 0, 8, 0, 0, 0]);
        for u in "GFP\0".encode_utf16() {
            b.extend_from_slice(&u.to_le_bytes());
        }
        b.extend_from_slice(&[3, 0, 4, 5, 0, 0, 3, 0, 0, 0, 0, 0]);
        let t = parse_tags(&b).unwrap();
        assert_eq!(t.version, 0x2000_1000);
        assert_eq!(t.i64(515), Some(1513));
        assert_eq!(t.text(1284).as_deref(), Some("GFP"));
        assert_eq!(t.unparsed, 0);
        // truncated: the second entry is lost, never a panic
        let t = parse_tags(&b[..b.len() - 5]).unwrap();
        assert_eq!(t.tags.len(), 1);
        assert_eq!(t.unparsed, 1);
        assert_eq!(
            read_value(&[5, 0, 0, 0, 0, 0, 0, 0, 0xf0, 0x3f], 0),
            Some((TagValue::Float(1.0), 10))
        );
        assert_eq!(read_value(&[99, 0], 0), None);
        assert_eq!(read_value(&[8, 0, 0xff, 0xff, 0xff, 0xff], 0), None);
    }
}
