//! The record chain of `analysis.baf`: every record is a u32 length (the whole record), a u32
//! tag `0xBFA0xxxx` and a payload; several records hold a header and a chain of sub-records.
//! `docs/formats/bruker-baf.md` § Records.

use std::collections::BTreeMap;

use openreadout_core::bytes::{latin1, le_f32, le_i16, le_u16, le_u32, le_u64, until_nul};
use openreadout_core::{Error, Result};
use serde_json::Value;

use super::FORMAT_ID;

/// High half every tag shares.
pub const TAG_BASE: u32 = 0xBFA0_0000;
/// The first record: offsets of the acquisition header, the summary table and the trailer index.
pub const TAG_POINTERS: u32 = 0xBFA0_0009;
/// Per-spectrum summary table.
pub const TAG_SUMMARY: u32 = 0xBFA0_0008;
/// Trailer index of record offsets.
pub const TAG_TRAILER: u32 = 0xBFA0_000A;
/// Acquisition header.
pub const TAG_ACQUISITION: u32 = 0xBFA0_3001;
/// Variable definitions (in the acquisition header).
pub const TAG_VARIABLE_DEFS: u32 = 0xBFA0_3002;
/// String table (in the acquisition header).
pub const TAG_STRINGS: u32 = 0xBFA0_3003;
/// Acquisition segment.
pub const TAG_SEGMENT: u32 = 0xBFA0_2001;
/// One spectrum's header.
pub const TAG_SPECTRUM: u32 = 0xBFA0_2002;
/// A spectrum's profile block.
pub const TAG_PROFILE: u32 = 0xBFA0_1001;
/// A spectrum's centroid (line) block.
pub const TAG_LINES: u32 = 0xBFA0_1002;
/// Writer id.
pub const TAG_WRITER: u32 = 0xBFA0_4001;
/// Compressor description.
pub const TAG_COMPRESSOR: u32 = 0xBFA0_4003;
/// Per-spectrum variable list (sub-record of a spectrum).
pub const TAG_VARIABLES: u32 = 0xBFA0_0001;
/// Precursor list (sub-record of a spectrum).
pub const TAG_PRECURSORS: u32 = 0xBFA0_0005;
/// Calibration (sub-record of a spectrum or of the acquisition header).
pub const TAG_CALIBRATION: u32 = 0xBFA0_0006;
/// Per-scan-type variable list (sub-record of a segment).
pub const TAG_SEGMENT_VARIABLES: u32 = 0xBFA0_000D;

/// Most records accepted in one sub-chain.
const MAX_CHAIN: usize = 1 << 22;

pub(crate) fn bad(detail: impl Into<String>) -> Error {
    Error::corrupt(FORMAT_ID, detail.into())
}

pub(crate) fn bad_at(offset: u64, detail: impl Into<String>) -> Error {
    Error::corrupt_at(FORMAT_ID, offset, detail.into())
}

/// One record of a chain: where it starts (in the buffer it was found in), its length and tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordRef {
    /// Start of the record (its length field).
    pub offset: usize,
    /// Length of the whole record.
    pub len: usize,
    /// Its tag.
    pub tag: u32,
}

/// The chain of records in `buf[start..end]`; it must end exactly at `end`.
pub fn walk(buf: &[u8], start: usize, end: usize) -> Result<Vec<RecordRef>> {
    let mut out = Vec::new();
    let mut o = start;
    while o + 8 <= end {
        let len = le_u32(buf, o).unwrap_or(0) as usize;
        let tag = le_u32(buf, o + 4).unwrap_or(0);
        if len < 8 || o + len > end || tag & 0xFFFF_0000 != TAG_BASE {
            return Err(bad(format!(
                "bad sub-record at byte {o} (length {len}, tag {tag:#010x})"
            )));
        }
        out.push(RecordRef {
            offset: o,
            len,
            tag,
        });
        if out.len() > MAX_CHAIN {
            return Err(bad("more sub-records than accepted"));
        }
        o += len;
    }
    if o != end {
        return Err(bad(format!("{} stray bytes at the end of a record", end - o)));
    }
    Ok(out)
}

/// A NUL-terminated Latin-1 string at `at` (to `end` at most).
pub fn cstr(buf: &[u8], at: usize, end: usize) -> String {
    buf.get(at..end.min(buf.len()))
        .map(|b| latin1(until_nul(b)))
        .unwrap_or_default()
}

/// The pointer record's offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pointers {
    /// Offset of the acquisition header.
    pub acquisition: u64,
    /// Offset of the summary table.
    pub summary: u64,
    /// Offset of the trailer index.
    pub trailer: u64,
}

/// Parse the pointer record (its whole bytes).
pub fn parse_pointers(rec: &[u8]) -> Option<Pointers> {
    if le_u32(rec, 4)? != TAG_POINTERS || le_u32(rec, 8)? != 0xBFA0_0005 {
        return None;
    }
    Some(Pointers {
        acquisition: le_u64(rec, 12)?,
        summary: le_u64(rec, 20)?,
        trailer: le_u64(rec, 28)?,
    })
}

/// A variable definition (sub-record `0xBFA03002` of the acquisition header).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VariableDef {
    /// Permanent name (e.g. `Collision_Energy_Act`).
    pub name: String,
    /// Display group.
    pub group: String,
    /// Display label.
    pub label: String,
    /// Value texts, `0:Positive;1:Negative`.
    pub value_text: String,
    /// Display format.
    pub format: String,
    /// Unit.
    pub unit: String,
    /// Value type: 10 an IEEE double, 5 (and 0, 1, 3, 4) a 64-bit integer.
    pub kind: u8,
}

impl VariableDef {
    /// Decode an 8-byte value of this variable: a double, an integer, or `None` for types not
    /// known.
    pub fn decode(&self, raw: [u8; 8]) -> Option<f64> {
        match self.kind {
            0x0A | 0x02 => Some(f64::from_le_bytes(raw)),
            0x00 | 0x01 | 0x03 | 0x04 | 0x05 => Some(i64::from_le_bytes(raw) as f64),
            _ => None,
        }
    }

    /// The text `value_text` gives an integer value (`0:CID;1:ETD` → 0 → `CID`).
    pub fn text_of(&self, v: f64) -> Option<String> {
        self.value_text.split(';').find_map(|item| {
            let (k, t) = item.split_once(':')?;
            (k.trim().parse::<f64>().ok()? == v).then(|| t.trim().to_string())
        })
    }
}

/// Parse variable definitions (the whole sub-record).
pub fn parse_variable_defs(rec: &[u8]) -> BTreeMap<u16, VariableDef> {
    let mut defs = BTreeMap::new();
    let Some(hdr) = le_u16(rec, 8) else {
        return defs;
    };
    let mut p = usize::from(hdr);
    while p + 20 <= rec.len() {
        let (Some(elen), Some(id)) = (le_u16(rec, p), le_u16(rec, p + 2)) else {
            break;
        };
        let elen = usize::from(elen);
        if elen < 20 || p + elen > rec.len() {
            break;
        }
        let kind = rec[p + 4];
        let s = |k: usize| -> String {
            match le_u16(rec, p + 6 + 2 * k) {
                Some(0) | None => String::new(),
                Some(off) => cstr(rec, p + usize::from(off), p + elen),
            }
        };
        defs.insert(
            id,
            VariableDef {
                name: s(1),
                group: s(2),
                label: s(3),
                value_text: s(4),
                format: s(5),
                unit: s(6),
                kind,
            },
        );
        p += elen;
    }
    defs
}

/// Parse the string table (the whole sub-record): string 1 at the header size, then the
/// strings the u32 offsets after the header field point at, up to the first 0.
pub fn parse_strings(rec: &[u8]) -> Vec<String> {
    let Some(hdr) = le_u32(rec, 8).map(|v| v as usize) else {
        return Vec::new();
    };
    let mut out = vec![cstr(rec, hdr, rec.len())];
    let mut p = 12;
    while p + 4 <= hdr.min(rec.len()) {
        let off = le_u32(rec, p).unwrap_or(0) as usize;
        if off == 0 || off >= rec.len() {
            break;
        }
        out.push(cstr(rec, off, rec.len()));
        p += 4;
    }
    out
}

/// A list of (variable id, 8-byte value) after a header of `hdr` bytes holding the count at
/// byte 12 of the record.
pub fn parse_variable_list(rec: &[u8], hdr: usize, count: usize) -> Result<BTreeMap<u16, [u8; 8]>> {
    let mut out = BTreeMap::new();
    let mut p = hdr;
    for _ in 0..count {
        let (Some(id), Some(raw)) = (le_u16(rec, p), openreadout_core::bytes::array::<8>(rec, p + 2))
        else {
            return Err(bad("a variable list runs past its record"));
        };
        out.insert(id, raw);
        p += 10;
    }
    Ok(out)
}

/// The acquisition header.
#[derive(Debug, Clone, Default)]
pub struct AcquisitionHeader {
    /// Acquisition start, `YYYY-MM-DDThh:mm:ss.mmm` in UTC, and the local offset in minutes.
    pub started_utc: Option<(String, i16)>,
    /// The analysis name after the date block (the `.d` folder name).
    pub analysis_name: Option<String>,
    /// The string table.
    pub strings: Vec<String>,
    /// Variable definitions by id.
    pub variables: BTreeMap<u16, VariableDef>,
    /// Reference calibration payloads.
    pub reference_calibrations: Vec<Vec<u8>>,
}

impl AcquisitionHeader {
    /// String `k` of the table, when present and not empty.
    pub fn string(&self, k: usize) -> Option<&str> {
        self.strings
            .get(k)
            .map(String::as_str)
            .filter(|s| !s.trim().is_empty())
    }

    /// The id of the variable named `name`.
    pub fn variable_id(&self, name: &str) -> Option<u16> {
        self.variables
            .iter()
            .find(|(_, d)| d.name == name)
            .map(|(k, _)| *k)
    }

    /// Variable definitions as JSON (id → name, unit, values).
    pub fn variables_json(&self) -> Value {
        Value::Object(
            self.variables
                .iter()
                .map(|(k, d)| {
                    (
                        k.to_string(),
                        serde_json::json!({"name": d.name, "group": d.group, "label": d.label,
                                           "values": d.value_text, "unit": d.unit, "type": d.kind}),
                    )
                })
                .collect(),
        )
    }
}

fn date_block(rec: &[u8], at: usize) -> Option<(String, i16)> {
    let (day, month, year) = (*rec.get(at)?, *rec.get(at + 1)?, le_u16(rec, at + 2)?);
    if !(1..=31).contains(&day) || !(1..=12).contains(&month) || !(1990..=2100).contains(&year) {
        return None;
    }
    let tz = le_i16(rec, at + 4)?;
    let ms = le_u16(rec, at + 6)?;
    let (sec, min, hour) = (*rec.get(at + 8)?, *rec.get(at + 9)?, *rec.get(at + 10)?);
    if sec > 60 || min > 59 || hour > 23 || ms > 999 {
        return None;
    }
    Some((
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}.{ms:03}"),
        tz,
    ))
}

/// Parse the acquisition header record (its whole bytes).
pub fn parse_acquisition(rec: &[u8]) -> Result<AcquisitionHeader> {
    let mut h = AcquisitionHeader::default();
    // The date block sits 40 bytes into the body (otofControl) or 44 (the 2008 file).
    let mut tail = 48;
    for at in [48usize, 52] {
        if let Some(d) = date_block(rec, at) {
            h.started_utc = Some(d);
            tail = at + 12;
            break;
        }
    }
    // The sub-records start at the string table; an analysis name may come before it.
    let needle = TAG_STRINGS.to_le_bytes();
    let first = rec
        .get(tail..)
        .and_then(|b| openreadout_core::bytes::find(b, &needle))
        .map(|i| tail + i)
        .filter(|&i| i >= tail + 4)
        .ok_or_else(|| bad("the acquisition header has no string table"))?
        - 4;
    if first > tail {
        let name = cstr(rec, tail, first);
        if !name.is_empty() {
            h.analysis_name = Some(name);
        }
    }
    for r in walk(rec, first, rec.len())? {
        let sub = &rec[r.offset..r.offset + r.len];
        match r.tag {
            TAG_STRINGS => h.strings = parse_strings(sub),
            TAG_VARIABLE_DEFS => h.variables = parse_variable_defs(sub),
            TAG_CALIBRATION if sub.len() > 12 => h.reference_calibrations.push(sub[12..].to_vec()),
            _ => {}
        }
    }
    Ok(h)
}

/// One per-scan-type list of a segment.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SegmentList {
    /// The four scan-type bytes (polarity, ?, scan mode, ?).
    pub scan_type: [u8; 4],
    /// Values by variable id.
    pub values: BTreeMap<u16, [u8; 8]>,
}

/// An acquisition segment.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Segment {
    /// Segment number.
    pub number: u32,
    /// Its per-scan-type lists.
    pub lists: Vec<SegmentList>,
}

/// Parse a segment record (its whole bytes).
pub fn parse_segment(rec: &[u8]) -> Result<Segment> {
    let hdr = le_u32(rec, 8).ok_or_else(|| bad("short segment record"))? as usize;
    let number = le_u32(rec, 16).unwrap_or(0);
    let mut seg = Segment {
        number,
        lists: Vec::new(),
    };
    if hdr > rec.len() {
        return Err(bad("segment header longer than its record"));
    }
    for r in walk(rec, hdr, rec.len())? {
        if r.tag != TAG_SEGMENT_VARIABLES {
            continue;
        }
        let sub = &rec[r.offset..r.offset + r.len];
        let (Some(lh), Some(n)) = (le_u32(sub, 8), le_u32(sub, 12)) else {
            continue;
        };
        let scan_type = openreadout_core::bytes::array::<4>(sub, 16).unwrap_or_default();
        seg.lists.push(SegmentList {
            scan_type,
            values: parse_variable_list(sub, lh as usize, n as usize)?,
        });
    }
    Ok(seg)
}

/// One entry of the summary table.
#[derive(Debug, Clone, PartialEq)]
pub struct SummaryEntry {
    /// Spectrum id (1-based).
    pub id: u32,
    /// Segment number.
    pub segment: u32,
    /// Retention time in milliseconds.
    pub rt_ms: u32,
    /// Block flags (3 profile and lines, 2 lines only).
    pub flags: u32,
    /// Line statistics: first and last sample of the range, minimum, maximum, sum.
    pub stats: [f32; 5],
    /// Scan-type bytes.
    pub scan_type: [u8; 4],
}

/// Parse the summary table (its whole bytes).
pub fn parse_summary(rec: &[u8]) -> Result<Vec<SummaryEntry>> {
    let hdr = usize::from(le_u16(rec, 8).ok_or_else(|| bad("short summary table"))?);
    let n = le_u32(rec, 16).unwrap_or(0) as usize;
    if n > MAX_CHAIN {
        return Err(bad(format!("summary table of {n} spectra")));
    }
    let mut out = Vec::with_capacity(n);
    let mut p = hdr;
    for _ in 0..n {
        let size = usize::from(le_u16(rec, p).ok_or_else(|| bad("summary table cut short"))?);
        if size < 56 || p + size > rec.len() {
            return Err(bad("a summary entry runs past the table"));
        }
        let u = |k: usize| le_u32(rec, p + k).unwrap_or(0);
        let f = |k: usize| le_f32(rec, p + k).unwrap_or(0.0);
        out.push(SummaryEntry {
            rt_ms: u(8),
            segment: u(16),
            id: u(20),
            flags: u(24),
            stats: [f(28), f(32), f(36), f(40), f(44)],
            scan_type: openreadout_core::bytes::array::<4>(rec, p + 48).unwrap_or_default(),
        });
        p += size;
    }
    if p != rec.len() {
        return Err(bad("the summary table's entries do not fill it"));
    }
    Ok(out)
}

/// Parse the trailer index (its whole bytes): (id, offset) pairs after the header.
pub fn parse_trailer(rec: &[u8]) -> Vec<(u32, u64)> {
    let hdr = le_u32(rec, 12).unwrap_or(64) as usize;
    let mut out = Vec::new();
    let mut p = hdr;
    while p + 12 <= rec.len() {
        if let (Some(id), Some(off)) = (le_u32(rec, p), le_u64(rec, p + 4)) {
            out.push((id, off));
        }
        p += 12;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(tag: u32, body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_le_bytes().to_vec();
        v.extend_from_slice(&tag.to_le_bytes());
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn chains_strings_and_variables() {
        let mut buf = rec(TAG_VARIABLES, &[1, 2, 3, 4]);
        buf.extend(rec(TAG_PRECURSORS, &[]));
        let r = walk(&buf, 0, buf.len()).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!((r[1].offset, r[1].tag), (12, TAG_PRECURSORS));
        assert!(walk(&buf, 0, buf.len() - 1).is_err());
        let mut bad_tag = buf.clone();
        bad_tag[7] = 0;
        assert!(walk(&bad_tag, 0, buf.len()).is_err());

        // String table: header size 20 (string 1 at record offset 20), the offset of string
        // 2 (22), then 0.
        let mut st = Vec::new();
        st.extend_from_slice(&20u32.to_le_bytes());
        st.extend_from_slice(&22u32.to_le_bytes());
        st.extend_from_slice(&0u32.to_le_bytes());
        st.extend_from_slice(b"a\0b\0");
        let r = rec(TAG_STRINGS, &st);
        assert_eq!(parse_strings(&r), ["a", "b"]);

        let def = VariableDef {
            value_text: "0:CID;1:ETD".into(),
            kind: 5,
            ..VariableDef::default()
        };
        assert_eq!(def.decode(1i64.to_le_bytes()), Some(1.0));
        assert_eq!(def.text_of(1.0).as_deref(), Some("ETD"));
        let dbl = VariableDef {
            kind: 10,
            ..VariableDef::default()
        };
        assert_eq!(dbl.decode(2.5f64.to_le_bytes()), Some(2.5));
    }

    #[test]
    fn dates() {
        let mut b = vec![0u8; 60];
        b[48..60].copy_from_slice(&[20, 11, 0xE5, 0x07, 60, 0, 0x33, 0x01, 36, 52, 3, 0]);
        assert_eq!(
            date_block(&b, 48),
            Some(("2021-11-20T03:52:36.307".into(), 60))
        );
        assert_eq!(date_block(&b, 44), None);
    }
}
