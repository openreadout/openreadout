//! NETZSCH Proteus measurement files (`.ngb-ss3`, `.ngb-sd7`, `.ngb-ds3`, `.ngb-bs3`, `.ngb-dla`,
//! `.ngb-cla`): a zip of `Streams/stream_N.table` members, each a small database of serialized
//! tables. Notes: `docs/formats/netzsch-ngb.md`; provenance: `docs/provenance/netzsch-ngb.md`.

use std::collections::BTreeMap;

use openreadout_core::assurance::{FeatureKind, Observations, Scope};
use openreadout_core::bytes::{self, le_f32, le_f64, le_i32, le_u16, le_u32, utf16le};
use openreadout_core::experiment::MeasurementKind;
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::Source;
use openreadout_core::series::{Facts, SeriesChannel, SeriesFile, SeriesTrace};
use openreadout_core::zip::ZipIndex;
use openreadout_core::{Error, Result};
use serde_json::{Map, Value, json};

pub(crate) const FORMAT_ID: &str = "netzsch-ngb";

/// Largest array accepted (elements), a guard against absurd counts.
const MAX_ELEMENTS: usize = 200_000_000;
/// Largest stream member read.
const MAX_STREAM: u64 = 1 << 30;

const MAGIC: &[u8] = b"Netzsch TA file";
const FORMAT_TAG: &[u8] = b"_db_format_1";
const ANCHOR: &[u8] = &[0x18, 0xfc, 0xff, 0xff];
const HEADER: &[u8] = &[0x18, 0xfc, 0xff, 0xff, 0x03, 0x80, 0x01];
/// The twelve fixed bytes between the field id and the type byte.
const MIDDLE: &[u8] = &[0, 0, 1, 0, 0, 0, 0x0c, 0, 0x17, 0xfc, 0xff, 0xff];
const END_FIELD: &[u8] = &[1, 0, 0, 0, 2, 0, 1, 0, 0];
const UNIT_3: &[u8] = &[1, 0, 0, 0, 3, 0];
const UNIT_4_3: &[u8] = &[1, 0, 0, 0, 4, 0, 1, 0, 0, 0, 3, 0];
const TRAILER: &[u8] = &[0, 3, 0];
const TABLE_OPEN: &[u8] = &[2, 0, 0, 0x80];
const PROLOGUE: &[u8] = &[2, 0, 0, 0x80];

const T_NULL: u8 = 0x00;
const T_U16: u8 = 0x02;
const T_I32: u8 = 0x03;
const T_F32: u8 = 0x04;
const T_F64: u8 = 0x05;
const T_U8: u8 = 0x10;
const T_PACKED8: u8 = 0x14;
const T_REF: u8 = 0x1a;
const T_STRING: u8 = 0x1f;
const T_HASH16: u8 = 0x48;

/// Type ref of a channel-header table (the low byte of its category names the channel).
const CHANNEL_HEADER: u16 = 0x2B22;
/// Type ref of a per-segment value table.
const SEGMENT_VALUES: u16 = 0x2B23;
/// Array fields holding channel data.
const DATA_F64: u16 = 0x0F40;
const DATA_F32: u16 = 0x0F3D;

fn item_size(t: u8) -> Option<usize> {
    Some(match t {
        T_NULL => 0,
        T_U16 => 2,
        T_I32 | T_F32 => 4,
        T_F64 | T_PACKED8 => 8,
        T_U8 => 1,
        T_HASH16 => 16,
        _ => return None,
    })
}

fn starts(d: &[u8], at: usize, p: &[u8]) -> bool {
    d.get(at..).is_some_and(|s| s.starts_with(p))
}

fn find(d: &[u8], p: &[u8], from: usize, end: usize) -> Option<usize> {
    let end = end.min(d.len());
    if from >= end {
        return None;
    }
    bytes::find(&d[from..end], p).map(|k| k + from)
}

/// One decoded record: its field id, type, and payload range (arrays: after the count).
#[derive(Clone, Debug)]
struct Field {
    id: u16,
    dtype: u8,
    array: Option<usize>,
    payload: std::ops::Range<usize>,
}

enum Parsed {
    Record(usize, Field),
    Truncated,
    None,
}

/// A record terminator at `pos`: its length.
fn terminator(d: &[u8], pos: usize, end: usize) -> Option<usize> {
    if pos + 9 <= end && starts(d, pos, END_FIELD) {
        Some(9)
    } else if pos + 12 <= end && starts(d, pos, UNIT_4_3) {
        Some(12)
    } else if pos + 6 <= end && starts(d, pos, UNIT_3) {
        Some(6)
    } else {
        None
    }
}

/// Parse one record strictly at `pos` (NGB record grammar).
fn parse_record(d: &[u8], pos: usize, end: usize) -> Parsed {
    if pos + 24 > end || !starts(d, pos, HEADER) || !starts(d, pos + 9, MIDDLE) {
        return Parsed::None;
    }
    let Some(id) = le_u16(d, pos + 7) else {
        return Parsed::None;
    };
    let dtype = d[pos + 21];
    let vs = pos + 24;
    if starts(d, pos + 22, &[0x80, 0x01]) {
        let len = match dtype {
            T_STRING => {
                if vs + 4 > end {
                    return Parsed::None;
                }
                if starts(d, vs, &[0xff, 0xfe, 0xff]) {
                    4 + 2 * usize::from(d[vs + 3])
                } else {
                    match le_u32(d, vs).and_then(|n| (n as usize).checked_add(4)) {
                        Some(n) => n,
                        None => return Parsed::None,
                    }
                }
            }
            T_REF => {
                let cap = (vs + 256 + END_FIELD.len()).min(end);
                match find(d, END_FIELD, vs, cap) {
                    Some(e) => e - vs,
                    None => return Parsed::None,
                }
            }
            t => match item_size(t) {
                Some(n) => n,
                None => return Parsed::None,
            },
        };
        let Some(pe) = vs.checked_add(len).filter(|&e| e <= end) else {
            return Parsed::None;
        };
        return match terminator(d, pe, end) {
            Some(t) => Parsed::Record(
                pe + t,
                Field {
                    id,
                    dtype,
                    array: None,
                    payload: vs..pe,
                },
            ),
            None => Parsed::None,
        };
    }
    if starts(d, pos + 22, &[0xa0, 0x01]) {
        let Some(size) = item_size(dtype) else {
            return Parsed::None;
        };
        if vs + 4 > end {
            return Parsed::None;
        }
        let Some(count) = le_u32(d, vs).map(|c| c as usize) else {
            return Parsed::None;
        };
        let Some(pe) = count.checked_mul(size).and_then(|b| b.checked_add(vs + 4)) else {
            return Parsed::Truncated;
        };
        if pe.checked_add(6).is_none_or(|x| x > end) {
            return Parsed::Truncated;
        }
        if count > MAX_ELEMENTS {
            return Parsed::Truncated;
        }
        return match terminator(d, pe, end) {
            Some(t) => Parsed::Record(
                pe + t,
                Field {
                    id,
                    dtype,
                    array: Some(count),
                    payload: vs + 4..pe,
                },
            ),
            None => Parsed::None,
        };
    }
    Parsed::None
}

/// A fixed-size scalar record without a terminator (a known variant at section ends).
fn is_bare_record(d: &[u8], pos: usize, end: usize) -> bool {
    pos + 24 <= end
        && starts(d, pos, HEADER)
        && starts(d, pos + 9, MIDDLE)
        && starts(d, pos + 22, &[0x80, 0x01])
        && item_size(d[pos + 21]).is_some_and(|n| pos + 24 + n == end)
}

/// Whether a run of non-record bytes is one of the known benign forms.
fn benign_gap(d: &[u8], start: usize, end: usize, saw_record: bool) -> bool {
    if !saw_record && starts(d, start, PROLOGUE) {
        return true;
    }
    if end - start == 3 && starts(d, start, TRAILER) {
        return true;
    }
    let anchor_at = if starts(d, start, &[0]) && starts(d, start + 1, ANCHOR) {
        start + 1
    } else {
        start
    };
    if starts(d, anchor_at, ANCHOR) && anchor_at + 7 <= end && starts(d, anchor_at + 5, &[0, 1]) {
        return true; // a preamble record
    }
    is_bare_record(d, start, end)
        || (starts(d, start, TRAILER) && is_bare_record(d, start + 3, end))
}

/// A table: its category (the open record's field id), type ref and fields (first occurrence).
#[derive(Debug)]
struct Table {
    category: u16,
    type_ref: u16,
    fields: Vec<Field>,
}

impl Table {
    fn get(&self, id: u16) -> Option<&Field> {
        self.fields.iter().find(|f| f.id == id)
    }
}

/// The type ref when a reference payload opens a table.
fn table_open(p: &[u8]) -> Option<u16> {
    if p.len() < 10 || !(p.starts_with(&[0x01, 0x80]) || p.starts_with(&[0xff, 0xff])) {
        return None;
    }
    let tag = p.windows(4).rposition(|w| w == TABLE_OPEN)?;
    if p.len() - tag != 8 || !p.ends_with(&[0, 0]) {
        return None;
    }
    le_u16(p, tag + 4)
}

/// One stream: its bytes, tables and how many defective (malformed or truncated) spans it has.
struct Stream {
    raw: Vec<u8>,
    tables: Vec<Table>,
    defects: usize,
    first_defect: Option<usize>,
}

/// Read one stream member: the container header, the section directory, and every section's
/// records assembled into tables.
fn read_stream(raw: Vec<u8>, name: &str) -> Result<Stream> {
    let corrupt = |msg: String| Error::corrupt(FORMAT_ID, format!("{name}: {msg}"));
    if raw.len() < 0x50 + 14 {
        return Err(corrupt(format!(
            "{} bytes is too small for its header",
            raw.len()
        )));
    }
    if !starts(&raw, 2, MAGIC) || !starts(&raw, 28, FORMAT_TAG) {
        return Err(corrupt(
            "no `Netzsch TA file` / `_db_format_1` header".into(),
        ));
    }
    let mut sections = Vec::new();
    let mut pos = 0x50;
    while pos + 14 <= raw.len() && starts(&raw, pos, &[0xff, 0xff]) {
        let id = le_u16(&raw, pos + 2).unwrap_or(0);
        let off = le_u32(&raw, pos + 4).unwrap_or(0) as usize;
        let size = le_u32(&raw, pos + 8).unwrap_or(0) as usize;
        if id == 0 && off == 0 && size == 0 {
            break;
        }
        sections.push((id, off, size));
        pos += 14;
    }
    if sections.is_empty() {
        return Err(corrupt("an empty section directory".into()));
    }
    let mut expect = sections[0].1;
    if expect < pos {
        return Err(corrupt("the first section overlaps the directory".into()));
    }
    for &(id, off, size) in &sections {
        if off != expect {
            return Err(corrupt(format!(
                "section {id} starts at {off}, not {expect}"
            )));
        }
        expect = off
            .checked_add(size)
            .ok_or_else(|| corrupt("a section size overflows".into()))?;
    }
    if expect != raw.len() {
        return Err(corrupt(format!(
            "the sections end at {expect}, the stream at {}",
            raw.len()
        )));
    }
    let mut tables: Vec<Table> = Vec::new();
    let mut current: Option<Table> = None;
    let mut defects = 0usize;
    let mut first_defect = None;
    for &(_, off, size) in &sections {
        let end = off + size;
        let mut pos = off;
        let mut saw = false;
        let mut pending: Option<(usize, Field)> = None;
        while pos < end {
            let parsed = match pending.take() {
                Some((n, f)) => Parsed::Record(n, f),
                None => parse_record(&raw, pos, end),
            };
            match parsed {
                Parsed::Truncated => {
                    defects += 1;
                    first_defect.get_or_insert(pos);
                    break;
                }
                Parsed::Record(next, f) => {
                    saw = true;
                    if f.dtype == T_REF
                        && f.array.is_none()
                        && let Some(tr) = table_open(&raw[f.payload.clone()])
                    {
                        if let Some(t) = current.take() {
                            tables.push(t);
                        }
                        current = Some(Table {
                            category: f.id,
                            type_ref: tr,
                            fields: Vec::new(),
                        });
                        pos = next;
                        continue;
                    }
                    if let Some(t) = current.as_mut()
                        && t.get(f.id).is_none()
                    {
                        t.fields.push(f);
                    }
                    pos = next;
                }
                Parsed::None => {
                    if saw
                        && pos + 3 <= end
                        && starts(&raw, pos, TRAILER)
                        && (pos + 3 == end || starts(&raw, pos + 3, ANCHOR))
                    {
                        pos += 3;
                        continue;
                    }
                    // resync at the next anchor that parses as a whole record
                    let mut gap_end = end;
                    let mut probe = find(&raw, ANCHOR, pos + 1, end);
                    while let Some(p) = probe {
                        if let Parsed::Record(n, f) = parse_record(&raw, p, end) {
                            gap_end = p;
                            pending = Some((n, f));
                            break;
                        }
                        probe = find(&raw, ANCHOR, p + 1, end);
                    }
                    if !benign_gap(&raw, pos, gap_end, saw) {
                        defects += 1;
                        first_defect.get_or_insert(pos);
                    }
                    pos = gap_end;
                }
            }
        }
    }
    if let Some(t) = current.take() {
        tables.push(t);
    }
    Ok(Stream {
        raw,
        tables,
        defects,
        first_defect,
    })
}

/// A decoded string payload (a UTF-16 `ff fe ff <chars>` form, or a `u32` byte length and UTF-8).
fn string(p: &[u8]) -> Option<String> {
    if p.len() < 4 {
        return None;
    }
    if p.starts_with(&[0xff, 0xfe, 0xff]) {
        let n = usize::from(p[3]);
        let b = p.get(4..4 + 2 * n)?;
        let s = utf16le(b).trim_matches('\0').to_string();
        return (!s.is_empty()).then_some(s);
    }
    let n = le_u32(p, 0)? as usize;
    let b = p.get(4..4usize.checked_add(n)?)?;
    let s = if let Ok(s) = std::str::from_utf8(b) {
        s.trim().replace('\0', "")
    } else {
        utf16le(b).trim_matches('\0').to_string()
    };
    (!s.is_empty()).then_some(s)
}

/// A scalar value as JSON.
fn scalar(raw: &[u8], f: &Field) -> Value {
    let p = &raw[f.payload.clone()];
    match f.dtype {
        T_U16 => le_u16(p, 0).map_or(Value::Null, |v| json!(v)),
        T_I32 => le_i32(p, 0).map_or(Value::Null, |v| json!(v)),
        T_F32 => le_f32(p, 0).map_or(Value::Null, |v| json!(f64::from(v))),
        T_F64 => le_f64(p, 0).map_or(Value::Null, |v| json!(v)),
        T_U8 => p.first().map_or(Value::Null, |v| json!(v)),
        T_STRING => string(p).map_or(Value::Null, Value::String),
        _ => Value::Null,
    }
}

/// A numeric array field as f64 values.
fn array(raw: &[u8], f: &Field) -> Option<Vec<f64>> {
    let p = &raw[f.payload.clone()];
    match f.dtype {
        T_F64 => Some(
            p.as_chunks::<8>()
                .0
                .iter()
                .map(|c| f64::from_le_bytes(*c))
                .collect(),
        ),
        T_F32 => Some(
            p.as_chunks::<4>()
                .0
                .iter()
                .map(|c| f64::from(f32::from_le_bytes(*c)))
                .collect(),
        ),
        _ => None,
    }
}

/// Our name and unit for a channel id (the low byte of a channel header's category).
fn channel(id: u8) -> (String, Option<&'static str>) {
    let (n, u) = match id {
        0x8C => ("time", Some("s")),
        0x8D => ("sample_temperature", Some("°C")),
        0x8E => ("dsc_signal", Some("µV")),
        0x90 => ("mass", Some("mg")),
        0x9C => ("purge_flow_1", Some("mL/min")),
        0x9D => ("purge_flow_2", Some("mL/min")),
        0x9E => ("protective_flow", Some("mL/min")),
        0x8F => ("length_change", Some("µm")),
        0x4E => ("force", Some("N")),
        0x4F => ("force_setpoint", Some("N")),
        0x30 => ("furnace_temperature", Some("°C")),
        0x31 => ("cooling_power", None),
        0x32 => ("furnace_power", None),
        0x33 => ("h_foil_temperature", Some("°C")),
        0x34 => ("uc_module", None),
        0x35 => ("environmental_pressure", None),
        0x36 => ("acceleration_x", None),
        0x37 => ("acceleration_y", None),
        0x38 => ("acceleration_z", None),
        _ => return (format!("channel_{id:02x}"), None),
    };
    (n.to_string(), u)
}

/// One run's channels: (channel id, values) in stream order.
type Run = Vec<(u8, Vec<f64>)>;

/// Split a data stream into runs and assemble each run's channels.
fn stream_runs(s: &Stream, sid: u32) -> Result<Vec<Run>> {
    let mut runs: Vec<Run> = vec![Vec::new()];
    let mut seen: Vec<u16> = Vec::new();
    let mut title: Option<u8> = None;
    let mut chunks: Vec<f64> = Vec::new();
    let flush = |runs: &mut Vec<Run>, title: Option<u8>, chunks: &mut Vec<f64>| -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }
        let Some(t) = title else {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("stream_{sid}: data values precede any channel header"),
            ));
        };
        if let Some(run) = runs.last_mut() {
            run.push((t, std::mem::take(chunks)));
        }
        Ok(())
    };
    for t in &s.tables {
        if t.type_ref == CHANNEL_HEADER {
            flush(&mut runs, title, &mut chunks)?;
            if seen.contains(&t.category) {
                runs.push(Vec::new());
                seen.clear();
            }
            seen.push(t.category);
            title = Some((t.category & 0xFF) as u8);
        } else if t.type_ref == SEGMENT_VALUES {
            let data = t
                .fields
                .iter()
                .find(|f| {
                    f.array.is_some_and(|n| n > 0)
                        && ((f.id == DATA_F64 && f.dtype == T_F64)
                            || (f.id == DATA_F32 && f.dtype == T_F32))
                })
                .and_then(|f| array(&s.raw, f));
            if let Some(v) = data {
                if title.is_none() {
                    return Err(Error::corrupt(
                        FORMAT_ID,
                        format!("stream_{sid}: data values precede any channel header"),
                    ));
                }
                chunks.extend(v);
            }
        }
    }
    flush(&mut runs, title, &mut chunks)?;
    runs.retain(|r| !r.is_empty());
    Ok(runs)
}

/// The first stream-1 table of `category` that carries `field`: its scalar value.
fn meta(s: &Stream, category: u16, field: u16) -> Option<Value> {
    s.tables
        .iter()
        .filter(|t| t.category == category)
        .find_map(|t| t.get(field))
        .map(|f| scalar(&s.raw, f))
        .filter(|v| !v.is_null())
}

fn meta_str(s: &Stream, category: u16, field: u16) -> Option<String> {
    meta(s, category, field)
        .and_then(|v| v.as_str().map(str::trim).map(str::to_string))
        .filter(|v| !v.is_empty())
}

/// The kind of instrument from the file extension's letters (`ss3`: STA, `sd7`: DSC, `dla`: DIL).
pub(crate) fn kind_of(ext: &str) -> &'static str {
    match ext.get(..1) {
        Some("s" | "b") if ext.ends_with('3') => "sta",
        Some("d") if ext.ends_with('3') => "sta",
        Some("s" | "b") if ext.ends_with('7') => "dsc",
        Some("d" | "c") if ext.ends_with('a') => "dil",
        _ => "thermal",
    }
}

/// Parse an `.ngb-*` zip.
pub(crate) fn parse(zip: &ZipIndex) -> Result<SeriesFile> {
    let zip_names: Vec<String> = zip.members.iter().map(|m| m.name.clone()).collect();
    let mut stream_ids: Vec<u32> = zip_names
        .iter()
        .filter_map(|n| {
            n.strip_prefix("Streams/stream_")
                .and_then(|r| r.strip_suffix(".table"))
                .and_then(|r| r.parse::<u32>().ok())
        })
        .collect();
    stream_ids.sort_unstable();
    if !stream_ids.contains(&1) || !stream_ids.contains(&2) {
        return Err(Error::Unsupported {
            format: FORMAT_ID,
            feature: "an NGB container without streams 1 and 2 (not a measurement file)".into(),
            hint: Some(
                "Proteus measurement files (.ngb-ss3, -sd7, -ds3, -bs3, -dla, -cla) are read; analysis (.ngb-taa) and state files are not".into(),
            ),
        });
    }
    let mut streams: BTreeMap<u32, Stream> = BTreeMap::new();
    let mut entries = Vec::new();
    for sid in &stream_ids {
        let name = format!("Streams/stream_{sid}.table");
        let m = zip
            .get(&name)
            .ok_or_else(|| Error::corrupt(FORMAT_ID, format!("{name} vanished")))?;
        if m.size > MAX_STREAM {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("{name} declares {} bytes", m.size),
            ));
        }
        let raw = zip.read(m)?;
        entries.push(LsEntry {
            kind: "member".into(),
            name: name.clone(),
            offset: None,
            size: Some(raw.len() as u64),
            image: None,
            details: Value::Null,
        });
        let s = read_stream(raw, &name)?;
        streams.insert(*sid, s);
    }
    // data: streams 2 and 3; any defect there is fatal
    let mut runs_by_stream: Vec<(u32, Vec<Run>)> = Vec::new();
    for sid in [2u32, 3] {
        let Some(s) = streams.get(&sid) else { continue };
        if s.defects > 0 {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                s.first_defect.unwrap_or(0) as u64,
                format!(
                    "stream_{sid} has {} span(s) outside the record grammar; its data are not read",
                    s.defects
                ),
            ));
        }
        let runs = stream_runs(s, sid)?;
        if !runs.is_empty() {
            runs_by_stream.push((sid, runs));
        }
    }
    let n_runs = runs_by_stream.first().map_or(0, |(_, r)| r.len());
    if n_runs == 0 {
        return Err(Error::corrupt(
            FORMAT_ID,
            "no channel data in streams 2 and 3",
        ));
    }
    if runs_by_stream.iter().any(|(_, r)| r.len() != n_runs) {
        return Err(Error::corrupt(
            FORMAT_ID,
            "the data streams disagree on the number of measurement runs",
        ));
    }
    let meta_stream = streams
        .get(&1)
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "no stream_1"))?;
    let measurement_type = meta(meta_stream, 0x1770, 0x103A).and_then(|v| v.as_i64());
    let mut findings = Vec::new();
    let mut traces = Vec::new();
    for run in 0..n_runs {
        let mut cols: Vec<(u8, Vec<f64>)> = Vec::new();
        for (_, runs) in &runs_by_stream {
            for (id, v) in &runs[run] {
                if cols.iter().any(|(c, _)| c == id) {
                    findings.push(Finding::warning(
                        "duplicate_channel",
                        format!("channel {id:#04x} appears twice in run {run}; the first is kept"),
                    ));
                    continue;
                }
                cols.push((*id, v.clone()));
            }
        }
        let n = cols
            .iter()
            .find(|(c, _)| *c == 0x8C)
            .map_or(0, |(_, v)| v.len());
        if n == 0 {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("run {run} has no time channel"),
            ));
        }
        if let Some((c, v)) = cols.iter().find(|(_, v)| v.len() != n) {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "channel {c:#04x} has {} values, the time channel {n}",
                    v.len()
                ),
            ));
        }
        // time first (minutes -> s), then the rest in stream order
        cols.sort_by_key(|(c, _)| u8::from(*c != 0x8C));
        let mut channels = Vec::new();
        let mut values = Vec::new();
        for (id, mut v) in cols {
            let (name, unit) = channel(id);
            if id == 0x8C {
                for x in &mut v {
                    *x *= 60.0;
                }
            }
            if name.starts_with("channel_") {
                findings.push(Finding::info(
                    "channel_unknown",
                    format!("channel {id:#04x} has no known quantity; its values are returned without a unit"),
                ));
            }
            let mut ch = SeriesChannel::new(name, unit, "float64");
            ch.extra.insert("channel_id".into(), json!(id));
            channels.push(ch);
            values.push(v);
        }
        let mut extra = BTreeMap::new();
        extra.insert(
            "axis".into(),
            json!({"quantity": "time", "unit": "s", "irregular": true, "channel": 0, "size": n}),
        );
        extra.insert("kind".into(), json!("thermal_analysis"));
        let run_name = match (n_runs, run, measurement_type) {
            (2, 1, _) => "correction",
            (_, _, Some(1)) => "correction",
            _ => "sample",
        };
        extra.insert("run".into(), json!(run_name));
        traces.push(SeriesTrace {
            name: run_name.into(),
            channels,
            sweeps: vec![values],
            sample_rate_hz: 0.0,
            start_s: None,
            extra,
        });
    }
    // metadata (stream 1)
    let s1 = meta_stream;
    let mut vendor = Map::new();
    let mut facts = Facts::new();
    facts.set("instrument.vendor", "NETZSCH", "the file format");
    facts.set("instrument.software", "NETZSCH Proteus", "the file format");
    let model = s1
        .tables
        .iter()
        .filter(|t| t.type_ref == 0x2B17)
        .find_map(|t| t.get(0x0432))
        .map(|f| scalar(&s1.raw, f))
        .and_then(|v| v.as_str().map(str::to_string));
    if let Some(m) = &model {
        facts.set(
            "instrument.model",
            m,
            "the instrument table (type 0x2B17, field 0x0432)",
        );
        vendor.insert("instrument_model".into(), json!(m));
    }
    for (key, cat, field) in [
        ("instrument", 0x1775u16, 0x1059u16),
        ("project", 0x1772, 0x083C),
        ("lab", 0x1772, 0x0834),
        ("operator", 0x1772, 0x0835),
        ("comment", 0x1772, 0x083D),
        ("crucible", 0x177E, 0x0840),
        ("furnace", 0x177A, 0x0840),
        ("carrier", 0x1779, 0x0840),
        ("sample_id", 0x7530, 0x0898),
        ("sample_name", 0x7530, 0x0840),
        ("material", 0x7530, 0x0962),
    ] {
        if let Some(v) = meta_str(s1, cat, field) {
            vendor.insert(key.into(), json!(v));
        }
    }
    let g = |k: &str| vendor.get(k).and_then(Value::as_str).map(str::to_string);
    if let Some(v) = g("instrument")
        && model.is_none()
    {
        facts.set("instrument.model", &v, "stream 1 instrument name");
    }
    if let Some(v) = g("operator") {
        facts.set("acquisition.operator", &v, "stream 1 operator");
    }
    if let Some(v) = g("comment") {
        facts.set("acquisition.comment", &v, "stream 1 comment");
    }
    if let Some(v) = g("sample_name") {
        facts.set("sample.name", &v, "stream 1 sample name");
    }
    if let Some(v) = g("sample_id") {
        facts.set("sample.id", &v, "stream 1 sample id");
    }
    if let Some(t) = meta(s1, 0x1772, 0x083E).and_then(|v| v.as_i64())
        && t > 0
    {
        let iso = openreadout_core::time::unix_to_iso8601(t, 0).replace(".000Z", "Z");
        facts.set(
            "acquisition.started_at",
            &iso,
            "stream 1 date performed (Unix seconds, UTC)",
        );
        vendor.insert("date_performed".into(), json!(iso));
    }
    for (name, field, unit) in [
        ("sample_mass", 0x0C9Eu16, "mg"),
        ("sample_length", 0x0C9F, "mm"),
    ] {
        if let Some(v) = meta(s1, 0x7530, field).and_then(|v| v.as_f64())
            && v > 0.0
        {
            facts.number(name, v, unit, "stream 1 sample descriptor");
            vendor.insert(name.into(), json!(v));
        }
    }
    let mtype = match measurement_type {
        Some(1) => Some("correction"),
        Some(2) => Some("sample"),
        Some(3) => Some("sample_correction"),
        _ => None,
    };
    if let Some(m) = mtype {
        facts.plain(
            "measurement_type",
            m,
            "stream 1 measurement definition (0x103A)",
        );
        vendor.insert("measurement_type".into(), json!(m));
    }
    let duration = traces
        .first()
        .and_then(|t| t.sweeps.first())
        .and_then(|s| s.first())
        .and_then(|t| t.last())
        .copied()
        .unwrap_or(0.0);
    if duration > 0.0 {
        facts.duration(duration, "the last time value of the sample run");
    }
    let names: Vec<String> = traces
        .first()
        .map(|t| t.channels.iter().map(|c| c.name.clone()).collect())
        .unwrap_or_default();
    let term = if names.iter().any(|c| c == "mass") && names.iter().any(|c| c == "dsc_signal") {
        Some("CHMO:0000681")
    } else if names.iter().any(|c| c == "mass") {
        Some("CHMO:0000690")
    } else if names.iter().any(|c| c == "dsc_signal") {
        Some("CHMO:0000684")
    } else if names.iter().any(|c| c == "length_change") {
        Some("CHMO:0002642")
    } else {
        None
    };
    if let Some(t) = term {
        facts.technique(t, "the channels the file holds");
    }
    for (k, t) in traces.iter().enumerate() {
        facts.measurement(
            MeasurementKind::Trace,
            vec![u32::try_from(k).unwrap_or(u32::MAX)],
            format!(
                "{} run, {} points, {} channels",
                t.name,
                t.sweeps.first().and_then(|s| s.first()).map_or(0, Vec::len),
                t.channels.len()
            ),
            term,
        );
    }
    let mut observations = Observations::default();
    let version = "db_format_1";
    observations.feature(
        FeatureKind::FormatVersion,
        version,
        &[Scope::Metadata, Scope::Traces],
    );
    if let Some(m) = &model {
        observations.feature(FeatureKind::Instrument, m, &[]);
    }
    for c in &names {
        observations.feature(
            FeatureKind::Record,
            format!("channel {c}"),
            &[Scope::Traces],
        );
    }
    if n_runs > 1 {
        observations.feature(
            FeatureKind::Layout,
            "sample and correction runs",
            &[Scope::Traces],
        );
    } else {
        observations.feature(FeatureKind::Layout, "one run", &[Scope::Traces]);
    }
    let meta_defects = streams
        .iter()
        .filter(|(k, _)| **k != 2 && **k != 3)
        .map(|(_, s)| s.defects)
        .sum::<usize>();
    if meta_defects > 0 {
        observations.undecoded(
            "metadata spans outside the record grammar",
            &[],
            format!("{meta_defects} span(s) in the metadata streams are not decoded"),
        );
    }
    vendor.insert("streams".into(), json!(stream_ids));
    vendor.insert("runs".into(), json!(n_runs));
    let mut provenance = openreadout_core::ProvenanceMap::new();
    provenance.insert("traces".into(), Source::PriorArt);
    Ok(SeriesFile {
        format_version: Some(version.into()),
        traces,
        tables: Vec::new(),
        experiment: Some(facts.build()),
        vendor: Value::Object(vendor),
        entries,
        findings,
        notes: Vec::new(),
        provenance,
        observations,
        checks: vec![
            "section directory contiguous to the end of each stream".into(),
            "every record of the data streams in the record grammar".into(),
            "every channel as long as the time channel".into(),
        ],
        members: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: u16, dtype: u8, array: Option<u32>, payload: &[u8]) -> Vec<u8> {
        let mut r = HEADER.to_vec();
        r.extend(id.to_le_bytes());
        r.extend(MIDDLE);
        r.push(dtype);
        if let Some(n) = array {
            r.extend([0xa0, 0x01]);
            r.extend(n.to_le_bytes());
        } else {
            r.extend([0x80, 0x01]);
        }
        r.extend(payload);
        r.extend(END_FIELD);
        r
    }

    #[test]
    fn records_and_table_opens() {
        let r = record(0x0F3D, T_F32, Some(2), &[0, 0, 128, 63, 0, 0, 0, 64]);
        let Parsed::Record(n, f) = parse_record(&r, 0, r.len()) else {
            panic!("not parsed");
        };
        assert_eq!(n, r.len());
        assert_eq!(array(&r, &f), Some(vec![1.0, 2.0]));
        // a count running past the end: truncated
        let mut t = r.clone();
        t[24..28].copy_from_slice(&1000u32.to_le_bytes());
        assert!(matches!(parse_record(&t, 0, t.len()), Parsed::Truncated));
        let open = [0x01, 0x80, 2, 0, 0, 0x80, 0x22, 0x2B, 0, 0];
        assert_eq!(table_open(&open), Some(0x2B22));
        assert_eq!(
            string(&[0xff, 0xfe, 0xff, 2, b'O', 0, b'K', 0]).as_deref(),
            Some("OK")
        );
        assert_eq!(
            string(&[3, 0, 0, 0, b'a', b'b', b'c']).as_deref(),
            Some("abc")
        );
    }
}
