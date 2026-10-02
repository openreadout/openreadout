//! The tagged record tree stored in a `.vsi` file right after its TIFF header.
//! Layout (our derivation, `docs/formats/vsi.md`): record sets ("volumes") of typed records
//! chained by offsets relative to the volume start; records may nest volumes.

use openreadout_core::bytes::{le_f64, le_i64, le_u16, le_u32, utf16le};
use serde_json::{Map, Value, json};

/// Where the tree starts in every `.vsi` file (right after the 8-byte TIFF header).
pub const TREE_OFFSET: usize = 8;
/// First four bytes of a volume header: u16 24, `IS`.
pub const VOLUME_MAGIC: [u8; 4] = [0x18, 0x00, b'I', b'S'];
/// Type bit: a u32 index follows the 16-byte record header.
pub const FLAG_INDEXED: u32 = 0x0800_0000;
/// Type bit: a nested volume follows the record header.
pub const FLAG_VOLUME: u32 = 0x8000_0000;
/// Type bit: no payload; the size word is the value.
pub const FLAG_INLINE: u32 = 0x4000_0000;
/// Type bit: the value is an array.
pub const FLAG_ARRAY: u32 = 0x2000;

const MAX_DEPTH: usize = 64;
const MAX_RECORDS: usize = 4_000_000;

/// A record's value.
#[derive(Debug, Clone, PartialEq)]
pub enum TagValue {
    /// A nested volume.
    Volume(Vec<TagRecord>),
    /// No payload: the size word itself.
    Inline(u32),
    /// Payload bytes `[start, end)` of the file.
    Data { start: usize, end: usize },
}

/// One record.
#[derive(Debug, Clone, PartialEq)]
pub struct TagRecord {
    pub tag: u32,
    /// Present when the type has `FLAG_INDEXED` (e.g. the stack id of a stack record).
    pub index: Option<u32>,
    pub type_word: u32,
    /// File offset of the record header.
    pub offset: usize,
    pub value: TagValue,
}

impl TagRecord {
    /// Value kind (low 12 bits of the type word).
    pub fn kind(&self) -> u32 {
        self.type_word & 0xFFF
    }
    pub fn is_array(&self) -> bool {
        self.type_word & FLAG_ARRAY != 0
    }
    pub fn children(&self) -> &[TagRecord] {
        match &self.value {
            TagValue::Volume(v) => v,
            _ => &[],
        }
    }
}

/// A parsed tree plus the bytes it points into.
#[derive(Debug, Clone, Default)]
pub struct TagTree {
    pub data: Vec<u8>,
    pub root: Vec<TagRecord>,
    /// Problems met while walking (bad magic, count mismatch, backward or out-of-file links).
    pub problems: Vec<(usize, String)>,
    pub record_count: usize,
}

impl TagTree {
    /// Parse the tree of a whole `.vsi` file held in memory.
    pub fn parse(data: Vec<u8>) -> TagTree {
        let mut t = TagTree {
            data,
            ..TagTree::default()
        };
        let root = t.volume(TREE_OFFSET, 0);
        t.root = root;
        t
    }

    fn problem(&mut self, at: usize, what: impl Into<String>) {
        if self.problems.len() < 64 {
            self.problems.push((at, what.into()));
        }
    }

    fn volume(&mut self, v: usize, depth: usize) -> Vec<TagRecord> {
        let mut out = Vec::new();
        if depth > MAX_DEPTH {
            self.problem(v, "records nested too deeply");
            return out;
        }
        if self.data.get(v..v + 4) != Some(&VOLUME_MAGIC[..]) {
            self.problem(v, "record set does not start with 18 00 'IS'");
            return out;
        }
        let (Some(first), Some(count)) = (le_u32(&self.data, v + 8), le_u32(&self.data, v + 16))
        else {
            self.problem(v, "record set header runs past the end of the file");
            return out;
        };
        if first == 0 {
            return out;
        }
        let mut e = v + first as usize;
        loop {
            if self.record_count >= MAX_RECORDS {
                self.problem(e, "too many records");
                break;
            }
            let (Some(ty), Some(tag), Some(next), Some(size)) = (
                le_u32(&self.data, e),
                le_u32(&self.data, e + 4),
                le_u32(&self.data, e + 8),
                le_u32(&self.data, e + 12),
            ) else {
                self.problem(e, "record header runs past the end of the file");
                break;
            };
            self.record_count += 1;
            let mut hdr = 16;
            let index = if ty & FLAG_INDEXED != 0 {
                hdr = 20;
                le_u32(&self.data, e + 16)
            } else {
                None
            };
            let next_abs = (next != 0).then(|| v + next as usize);
            let value = if ty & FLAG_VOLUME != 0 {
                TagValue::Volume(self.volume(e + hdr, depth + 1))
            } else if ty & FLAG_INLINE != 0 {
                TagValue::Inline(size)
            } else {
                let start = e + hdr;
                let mut end = start.saturating_add(size as usize);
                if let Some(n) = next_abs {
                    end = end.min(n);
                }
                end = end.min(self.data.len());
                TagValue::Data {
                    start: start.min(end),
                    end,
                }
            };
            out.push(TagRecord {
                tag,
                index,
                type_word: ty,
                offset: e,
                value,
            });
            match next_abs {
                None => break,
                Some(n) if n <= e || n >= self.data.len() => {
                    self.problem(
                        e,
                        format!("record {tag} links to offset {n}, not forward inside the file"),
                    );
                    break;
                }
                Some(n) => e = n,
            }
        }
        if out.len() != count as usize {
            self.problem(
                v,
                format!(
                    "record set declares {count} records, {} were found",
                    out.len()
                ),
            );
        }
        out
    }

    pub fn bytes(&self, r: &TagRecord) -> &[u8] {
        match r.value {
            TagValue::Data { start, end } => &self.data[start..end],
            _ => &[],
        }
    }

    /// Text value: UTF-16 for kinds 0xD, 0x0 and 0x2 arrays; UTF-8 for kind 0x1.
    pub fn text(&self, r: &TagRecord) -> Option<String> {
        let b = self.bytes(r);
        match (r.kind(), r.is_array()) {
            (0xD, _) | (0x0 | 0x2, true) => Some(utf16le(b).trim_end_matches('\0').to_string()),
            (0x1, _) => Some(
                String::from_utf8_lossy(b)
                    .trim_end_matches('\0')
                    .to_string(),
            ),
            _ => None,
        }
    }

    /// Scalar number (i32, u32, i64, u16, f64, boolean, or an inline value).
    pub fn number(&self, r: &TagRecord) -> Option<f64> {
        if let TagValue::Inline(v) = r.value {
            return Some(match r.kind() {
                0x5 => f64::from(v as i32),
                _ => f64::from(v),
            });
        }
        if r.is_array() {
            return None;
        }
        let b = self.bytes(r);
        match (r.kind(), b.len()) {
            (0x4, 2) => Some(f64::from(le_u16(b, 0)?)),
            (0x5, 4) => Some(f64::from(le_u32(b, 0)? as i32)),
            (0x6, 4) => Some(f64::from(le_u32(b, 0)?)),
            (0x7, 8) => Some(le_i64(b, 0)? as f64),
            (0xA, 8) => le_f64(b, 0).filter(|v| v.is_finite()),
            (0xC, n) if n > 0 => Some(f64::from(b[0])),
            _ => None,
        }
    }

    /// i64 value (kind 0x7), exactly.
    pub fn int64(&self, r: &TagRecord) -> Option<i64> {
        let b = self.bytes(r);
        if r.kind() != 0x7 {
            return None;
        }
        Some(i64::from_le_bytes(b.try_into().ok()?))
    }

    /// Array of i32 (kinds 0x103 and arrays of 0x3/0x7/0x8). Arrays of kind 0x7 hold 4-byte
    /// elements (the tile-grid offset 2410: 12 bytes for 3 values; its values place the tiles
    /// where Bio-Formats and the depositor's own export do, `docs/provenance/vsi.md`).
    pub fn ints(&self, r: &TagRecord) -> Option<Vec<i32>> {
        let ok = matches!((r.kind(), r.is_array()), (0x103, _) | (0x3 | 0x8, true))
            || (r.kind() == 0x7 && r.is_array() && self.bytes(r).len().is_multiple_of(4));
        ok.then(|| {
            self.bytes(r)
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| i32::from_le_bytes(*c))
                .collect()
        })
    }

    /// Pair/list of f64 (kinds 0x104, 0x10C, 0x117 and f64 arrays).
    pub fn floats(&self, r: &TagRecord) -> Option<Vec<f64>> {
        let ok = matches!(
            (r.kind(), r.is_array()),
            (0x104 | 0x10C | 0x117, _) | (0xA, true)
        );
        ok.then(|| {
            self.bytes(r)
                .as_chunks::<8>()
                .0
                .iter()
                .map(|c| f64::from_le_bytes(*c))
                .collect()
        })
    }

    /// A value container (tags 0x10000000..): `(value, unit)`.
    pub fn quantity(&self, vol: &TagRecord) -> Option<(f64, Option<String>)> {
        let kids = vol.children();
        let v = kids.iter().find(|c| c.tag == 0x1000_0002)?;
        let unit = kids
            .iter()
            .find(|c| c.tag == 0x1000_0000)
            .and_then(|u| self.text(u));
        Some((self.number(v)?, unit))
    }

    /// The whole tree as JSON: `"<tag>"` or `"<tag>[<index>]"` keys, repeated keys become
    /// arrays, values decoded by kind (unknown kinds as hex, capped at 256 bytes).
    pub fn to_json(&self) -> Value {
        self.records_json(&self.root)
    }

    fn records_json(&self, recs: &[TagRecord]) -> Value {
        let mut groups: Vec<(String, Vec<Value>)> = Vec::new();
        let mut at: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for r in recs {
            let key = match r.index {
                Some(i) => format!("{}[{i}]", r.tag),
                None => r.tag.to_string(),
            };
            let v = self.record_json(r);
            if let Some(&i) = at.get(&key) {
                groups[i].1.push(v);
            } else {
                at.insert(key.clone(), groups.len());
                groups.push((key, vec![v]));
            }
        }
        let mut obj = Map::new();
        for (k, mut vs) in groups {
            obj.insert(
                k,
                if vs.len() == 1 {
                    vs.remove(0)
                } else {
                    Value::Array(vs)
                },
            );
        }
        Value::Object(obj)
    }

    fn record_json(&self, r: &TagRecord) -> Value {
        match &r.value {
            TagValue::Volume(kids) => self.records_json(kids),
            TagValue::Inline(v) => match r.kind() {
                0xC => Value::Bool(*v != 0),
                0x5 => json!(*v as i32),
                _ => json!(v),
            },
            TagValue::Data { .. } => {
                if let Some(t) = self.text(r) {
                    return Value::String(if t.len() > 4096 {
                        format!("<{} characters omitted>", t.len())
                    } else {
                        t
                    });
                }
                if let Some(n) = self.number(r) {
                    return json!(n);
                }
                if let Some(v) = self.ints(r) {
                    return json!(v);
                }
                if let Some(v) = self.floats(r) {
                    return json!(v);
                }
                let b = self.bytes(r);
                let hex = b.iter().take(256).fold(String::new(), |mut acc, x| {
                    use std::fmt::Write as _;
                    let _ = write!(acc, "{x:02x}");
                    acc
                });
                json!({"#type": format!("{:#010x}", r.type_word), "#bytes": b.len(), "#hex": hex})
            }
        }
    }
}

/// Find the first child with `tag` (and `index`, when given).
pub fn find(recs: &[TagRecord], tag: u32, index: Option<u32>) -> Option<&TagRecord> {
    recs.iter()
        .find(|r| r.tag == tag && (index.is_none() || r.index == index))
}

/// Follow a path of tags from `recs`.
pub fn path<'a>(recs: &'a [TagRecord], tags: &[u32]) -> Option<&'a TagRecord> {
    let (first, rest) = tags.split_first()?;
    let mut cur = find(recs, *first, None)?;
    for t in rest {
        cur = find(cur.children(), *t, None)?;
    }
    Some(cur)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(ty: u32, tag: u32, next: u32, size: u32, payload: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        for w in [ty, tag, next, size] {
            v.extend_from_slice(&w.to_le_bytes());
        }
        v.extend_from_slice(payload);
        v
    }

    fn volume_header(first: u32, count: u32) -> Vec<u8> {
        let mut v = VOLUME_MAGIC.to_vec();
        for w in [1u32, first, 0, count, 0] {
            v.extend_from_slice(&w.to_le_bytes());
        }
        v
    }

    #[test]
    fn parses_nested_records() {
        // TIFF header, then a volume with: f64 record, inline i32, indexed nested volume
        let mut d = b"II*\0\0\0\0\0".to_vec();
        let v = d.len();
        d.extend(volume_header(24, 3));
        let r1 = d.len() - v;
        let r2 = r1 + 16 + 8;
        let r3 = r2 + 16;
        d.extend(record(0xA, 2019, r2 as u32, 8, &2.5f64.to_le_bytes()));
        d.extend(record(0x4000_0005, 2031, r3 as u32, 7, &[]));
        let mut rec3 = record(
            FLAG_VOLUME | FLAG_INDEXED,
            2001,
            0,
            0,
            &10002u32.to_le_bytes(),
        );
        rec3.extend(volume_header(24, 1));
        let name: Vec<u8> = "20x".encode_utf16().flat_map(u16::to_le_bytes).collect();
        rec3.extend(record(0x2000, 2030, 0, name.len() as u32, &name));
        d.extend(rec3);
        let t = TagTree::parse(d);
        assert!(t.problems.is_empty(), "{:?}", t.problems);
        assert_eq!(t.root.len(), 3);
        assert_eq!(t.number(&t.root[0]), Some(2.5));
        assert_eq!(t.number(&t.root[1]), Some(7.0));
        assert_eq!(t.root[2].index, Some(10002));
        let name = find(t.root[2].children(), 2030, None).unwrap();
        assert_eq!(t.text(name).as_deref(), Some("20x"));
        let j = t.to_json();
        assert_eq!(j["2019"], 2.5);
        assert_eq!(j["2001[10002]"]["2030"], "20x");
    }

    #[test]
    fn reports_bad_links() {
        let mut d = b"II*\0\0\0\0\0".to_vec();
        d.extend(volume_header(24, 2));
        d.extend(record(0x4000_0005, 1, 4, 0, &[]));
        let t = TagTree::parse(d);
        assert!(!t.problems.is_empty());
        let t = TagTree::parse(b"II*\0\0\0\0\0garbage".to_vec());
        assert!(!t.problems.is_empty());
    }
}
