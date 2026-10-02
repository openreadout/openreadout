//! UNICORN 3-5 result files (`.res`): a directory of named blocks (text, curves, event lists).
//! Layout: `docs/formats/cytiva-unicorn.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use openreadout_core::bytes::{latin1, le_f64, le_u16, le_u32, until_nul};
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::source::SourceFile;
use openreadout_core::time::unix_to_iso8601;
use openreadout_core::{Error, Result};
use serde_json::{Map, Value, json};

use crate::model::{Curve, CurveKind, Event, EventList, Facts, Parsed, Points, wavelength_of};

pub(crate) const FORMAT_ID: &str = "cytiva-unicorn-res";
/// First four bytes of every `.res` file.
pub(crate) const MAGIC: [u8; 4] = [0x11, 0x47, 0x11, 0x47];
/// Bytes of one directory entry.
const ENTRY_LEN: u64 = 344;
/// Bytes of a column descriptor inside a curve or event block header.
const DESC_LEN: usize = 78;
/// Bytes of one event record (time, volume, two 76-byte texts, value, flags).
const EVENT_LEN: usize = 180;
/// Most directory entries read (the files seen hold 13-30).
const MAX_ENTRIES: u64 = 4096;
/// Largest text or event block read into memory.
const MAX_BLOCK: u64 = 64 << 20;

/// One directory entry.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub(crate) name: String,
    pub(crate) kind: [u8; 6],
    pub(crate) size: u64,
    pub(crate) offset: u64,
    pub(crate) header: u64,
}

/// A column descriptor of a curve or event block.
#[derive(Debug, Clone)]
struct Desc {
    flags: u16,
    name: String,
    unit: String,
    storage: u16,
    factor: f64,
    second: f64,
}

/// Text of a fixed field: cut at the first NUL, Latin-1 when not UTF-8, trimmed.
fn text_field(b: &[u8]) -> String {
    let b = until_nul(b);
    match std::str::from_utf8(b) {
        Ok(s) => s.trim().to_string(),
        Err(_) => latin1(b).trim().to_string(),
    }
}

fn read(f: &SourceFile, path: &Path, offset: u64, len: u64, file_len: u64) -> Result<Vec<u8>> {
    let end = offset
        .checked_add(len)
        .filter(|e| *e <= file_len)
        .ok_or_else(|| {
            Error::corrupt_at(
                FORMAT_ID,
                offset,
                format!("{len} bytes at offset {offset} run past the end of the {file_len}-byte file (truncated)"),
            )
        })?;
    let n = usize::try_from(end - offset)
        .map_err(|_| Error::Other("read larger than memory".into()))?;
    let mut buf = vec![0u8; n];
    f.read_exact_at(offset, &mut buf)
        .map_err(|e| Error::io(path, e))?;
    Ok(buf)
}

/// Column descriptors at the start of a curve/event block: `(descriptors, stored columns)`.
fn descriptors(b: &[u8]) -> Option<(Vec<Desc>, u16)> {
    let stored = le_u16(b, 2)?;
    let mut out = Vec::new();
    let mut p = 6usize;
    while p + DESC_LEN <= b.len() && out.len() < 16 {
        if le_u16(b, p)? != DESC_LEN as u16 {
            break;
        }
        out.push(Desc {
            flags: le_u16(b, p + 2)?,
            name: text_field(b.get(p + 4..p + 44)?),
            unit: text_field(b.get(p + 44..p + 60)?),
            storage: le_u16(b, p + 60)?,
            factor: le_f64(b, p + 62)?,
            second: le_f64(b, p + 70)?,
        });
        p += DESC_LEN;
    }
    Some((out, stored))
}

/// Split a curve block name `<run name>:<run number>_<curve>` (the curve name may itself
/// hold underscores: `UV1_215nm`). Names without a colon (`=SAPA_215nm`, an evaluated curve)
/// are the curve name.
fn split_block_name(name: &str) -> (Option<String>, String) {
    if let Some((run, rest)) = name.split_once(':')
        && let Some((_, curve)) = rest.split_once('_')
    {
        return (Some(run.trim().to_string()), curve.trim().to_string());
    }
    (None, name.trim_start_matches('=').trim().to_string())
}

/// Parse the header and directory; curve points stay in the file.
pub(crate) fn parse(f: &SourceFile, path: &Path, file_len: u64) -> Result<Parsed> {
    let head = read(f, path, 0, 0x100.min(file_len), file_len)?;
    if head.len() < 0x80 || head[..4] != MAGIC {
        return Err(Error::corrupt(
            FORMAT_ID,
            "not a UNICORN .res file (the header is shorter than 128 bytes or lacks the 11 47 11 47 signature)",
        ));
    }
    let dir = u64::from(le_u32(&head, 8).unwrap_or(0));
    let declared_len = u64::from(le_u32(&head, 16).unwrap_or(0));
    let version_text = head.get(0x18..0x60).map(text_field).unwrap_or_default();
    let mut p = Parsed {
        format_version: version_text
            .strip_prefix("UNICORN")
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty()),
        ..Parsed::default()
    };
    if declared_len != file_len {
        p.findings.push(Finding::error(
            "truncated",
            format!(
                "the header says the file is {declared_len} bytes; it is {file_len} (an interrupted copy?)"
            ),
        ));
    }
    let t_first = le_u32(&head, 0x68).unwrap_or(0);
    let t_end = le_u32(&head, 0x6c).unwrap_or(0);
    let user = head.get(0x76..0x96).map(text_field).unwrap_or_default();

    // directory
    let mut entries = Vec::new();
    let mut at = dir;
    while entries.len() as u64 <= MAX_ENTRIES && at.saturating_add(ENTRY_LEN) <= file_len {
        let e = read(f, path, at, ENTRY_LEN, file_len)?;
        let name = text_field(&e[6..302]);
        if name.is_empty() {
            break;
        }
        let mut kind = [0u8; 6];
        kind.copy_from_slice(&e[..6]);
        entries.push(Entry {
            name,
            kind,
            size: u64::from(le_u32(&e, 302).unwrap_or(0)),
            offset: u64::from(le_u32(&e, 310).unwrap_or(0)),
            header: u64::from(le_u32(&e, 314).unwrap_or(0)),
        });
        at += ENTRY_LEN;
    }
    if entries.is_empty() {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            dir,
            "the block directory is empty or lies past the end of the file",
        ));
    }

    let mut vendor = Map::new();
    let mut texts = Map::new();
    let mut runs: BTreeSet<String> = BTreeSet::new();
    let mut provenance = ProvenanceMap::new();
    for (i, e) in entries.iter().enumerate() {
        p.entries.push(LsEntry {
            kind: block_kind(e).into(),
            name: e.name.clone(),
            offset: Some(e.offset),
            size: Some(e.size),
            image: None,
            details: json!({"block": i, "type": hex(&e.kind), "header_bytes": e.header}),
        });
        if e.size == 0 {
            continue;
        }
        if e.offset
            .checked_add(e.size)
            .is_none_or(|end| end > file_len)
        {
            p.findings.push(
                Finding::error(
                    "truncated",
                    format!(
                        "block `{}` ({} bytes at {}) runs past the end of the file",
                        e.name, e.size, e.offset
                    ),
                )
                .at(e.offset),
            );
            continue;
        }
        match e.header {
            240 => {
                if let Some(c) = curve(f, path, file_len, e, &mut p.findings)? {
                    if let (Some(run), _) = split_block_name(&e.name) {
                        runs.insert(run);
                    }
                    p.curves.push(c);
                }
            }
            552 => {
                if e.size > MAX_BLOCK {
                    continue;
                }
                let b = read(f, path, e.offset, e.size, file_len)?;
                let (run, what) = split_block_name(&e.name);
                if let Some(r) = run {
                    runs.insert(r);
                }
                let events = event_records(&b);
                let (name, label) = match what.to_ascii_lowercase().as_str() {
                    "logbook" => ("logbook".to_string(), "text"),
                    "fractions" => ("fractions".to_string(), "fraction"),
                    "inject" | "injections" => ("injections".to_string(), "injection"),
                    _ => (what.clone(), "text"),
                };
                p.events.push(EventList {
                    name,
                    label,
                    events,
                });
            }
            _ => {
                if is_text_block(e) && e.size <= MAX_BLOCK {
                    let b = read(f, path, e.offset, e.size, file_len)?;
                    let t = text_field(&b).replace("\r\n", "\n");
                    if !t.is_empty() {
                        texts.insert(e.name.clone(), Value::String(t));
                    }
                }
            }
        }
    }

    // experiment facts
    let mut facts = Facts::default();
    if !user.is_empty() {
        Facts::text(&mut facts.operator, &user, "header user name (byte 0x76)");
        provenance.insert("acquisition.operator".into(), Source::Inferred);
    }
    if let Some(Value::String(t)) = texts.get("METHODINFO") {
        let base = t.rsplit(['\\', '/']).next().unwrap_or(t);
        let stem = base.rsplit_once('.').map_or(base, |(s, _)| s);
        Facts::text(
            &mut facts.method_name,
            stem,
            "METHODINFO (method file path)",
        );
    }
    let logbook_first = p
        .events
        .iter()
        .find(|l| l.name == "logbook")
        .and_then(|l| l.events.iter().find(|e| e.text.starts_with("Method Run")))
        .map(|e| e.text.clone());
    if let Some(line) = &logbook_first
        && let Some((_, m)) = line.split_once("Method :")
    {
        let m = m.split(',').next().unwrap_or("").trim();
        Facts::text(
            &mut facts.method_name,
            m,
            "logbook `Method Run … Method : <name>`",
        );
    }
    if let Some(model) = system_model(&texts) {
        Facts::text(&mut facts.model, &model.0, model.1);
    }
    let duration_min = p
        .curves
        .iter()
        .filter(|c| c.original)
        .map(|c| c.start_min + c.interval_min * c.samples.saturating_sub(1) as f64)
        .fold(0.0_f64, f64::max);
    if t_end > 0 {
        let end = unix_to_iso8601(i64::from(t_end), 0);
        facts.ended_at = Some((
            end,
            "header time at byte 0x6c (UTC)".into(),
            Source::Inferred,
        ));
        if duration_min > 0.0 {
            let start = i64::from(t_end) - (duration_min * 60.0).round() as i64;
            facts.started_at = Some((
                unix_to_iso8601(start, 0),
                "header end time (byte 0x6c) − the curves' duration".into(),
                Source::Inferred,
            ));
        }
    }
    if let Some(Value::String(t)) = texts.get("Methods")
        && let Some(col) = column_of_methods(t)
    {
        facts.word(
            "column",
            &col.0,
            "Methods: `Base Volume, <V> {ml}, <column>`",
        );
        if let Some(v) = col.1 {
            facts.number(
                "column_volume",
                v,
                "mL",
                "Methods: `Base Volume, <V> {ml}, <column>`",
            );
        }
    }
    if let Some(Value::String(t)) = texts.get("Techniques") {
        facts.technique = technique_of(t);
        if let Some(l) = t.lines().nth(1).map(str::trim).filter(|l| !l.is_empty()) {
            facts.word("technique", l, "Techniques");
        }
    }

    // UNICORN shows volumes from the last injection
    p.zero_volume_ml = p
        .events
        .iter()
        .find(|l| l.name == "injections")
        .and_then(|l| l.events.last())
        .map(|e| e.volume_ml);
    for c in &mut p.curves {
        if let Some(z) = p.zero_volume_ml {
            c.extra.insert("injection_volume_ml".into(), json!(z));
        }
    }

    vendor.insert("format_text".into(), json!(version_text));
    vendor.insert("header_user".into(), json!(user));
    if t_first > 0 {
        vendor.insert(
            "header_time_0x68".into(),
            json!(unix_to_iso8601(i64::from(t_first), 0)),
        );
    }
    if t_end > 0 {
        vendor.insert(
            "header_time_0x6c".into(),
            json!(unix_to_iso8601(i64::from(t_end), 0)),
        );
    }
    vendor.insert(
        "runs".into(),
        json!(runs.iter().cloned().collect::<Vec<_>>()),
    );
    if let Some(l) = logbook_first {
        vendor.insert("method_run".into(), json!(l));
    }
    vendor.insert("text_blocks".into(), Value::Object(texts));
    vendor.insert(
        "blocks".into(),
        json!(
            entries
                .iter()
                .map(|e| json!({"name": e.name, "type": hex(&e.kind), "offset": e.offset, "size": e.size, "header_bytes": e.header}))
                .collect::<Vec<_>>()
        ),
    );
    p.vendor = json!({ "unicorn_res": Value::Object(vendor) });
    for k in ["traces", "tables"] {
        provenance.insert(k.into(), Source::Inferred);
    }
    p.provenance = provenance;
    p.facts = facts;
    if p.curves
        .iter()
        .any(|c| c.extra.contains_key("time_offset_ms"))
    {
        p.notes.push(
            "some curves carry a sub-sample start offset; it is read as milliseconds (an assumption: it moves times by less than one sampling interval)"
                .into(),
        );
    }
    Ok(p)
}

fn hex(b: &[u8]) -> String {
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

fn block_kind(e: &Entry) -> &'static str {
    match e.header {
        240 => "curve",
        552 => "events",
        _ if is_text_block(e) => "text",
        _ => "block",
    }
}

/// Blocks that hold plain text.
fn is_text_block(e: &Entry) -> bool {
    matches!(
        e.name.as_str(),
        "CreationNotes"
            | "Methods"
            | "MethodStrategyNotes"
            | "ResultStrategyNotes"
            | "Techniques"
            | "METHODINFO"
            | "Method Signatures"
    )
}

/// A curve block: 240-byte header of three descriptors, then (int32 volume, int32 value) pairs.
fn curve(
    f: &SourceFile,
    path: &Path,
    file_len: u64,
    e: &Entry,
    findings: &mut Vec<Finding>,
) -> Result<Option<Curve>> {
    let hdr = read(f, path, e.offset, e.header.min(e.size), file_len)?;
    let Some((d, stored)) = descriptors(&hdr) else {
        findings.push(Finding::warning(
            "unreadable_curve_header",
            format!("curve `{}`: its column descriptors do not parse", e.name),
        ));
        return Ok(None);
    };
    let time = d.iter().find(|x| x.flags == 0x8001);
    let volume = d.iter().find(|x| x.flags == 0x8002);
    let value = d.iter().find(|x| x.flags & 0x4000 != 0);
    let (Some(time), Some(volume), Some(value)) = (time, volume, value) else {
        findings.push(Finding::warning(
            "unreadable_curve_header",
            format!("curve `{}`: no time, volume and value descriptors", e.name),
        ));
        return Ok(None);
    };
    if stored != 2 || volume.storage != 0x0104 || value.storage != 0x0104 {
        findings.push(Finding::warning(
            "unsupported_curve_storage",
            format!(
                "curve `{}` stores {stored} columns with codes {:#06x}/{:#06x}; only two int32 columns are read",
                e.name, volume.storage, value.storage
            ),
        ));
        return Ok(None);
    }
    let usable = time.factor.is_finite()
        && time.factor > 0.0
        && volume.factor.is_finite()
        && value.factor.is_finite();
    if !usable {
        findings.push(Finding::warning(
            "unreadable_curve_header",
            format!(
                "curve `{}`: its sampling interval or scale factors are not finite",
                e.name
            ),
        ));
        return Ok(None);
    }
    let body = e.size.saturating_sub(e.header);
    if !body.is_multiple_of(8) {
        findings.push(Finding::warning(
            "partial_record",
            format!(
                "curve `{}`: {} bytes after the header are not whole 8-byte samples",
                e.name, body
            ),
        ));
    }
    let samples = body / 8;
    let (_, name) = split_block_name(&e.name);
    let unit = value.unit.trim().to_string();
    let kind = CurveKind::classify(&name, &unit, None);
    let original = e.kind[4] == 0x01; // 01 14 recorded, 03 14 evaluated
    let mut extra = BTreeMap::new();
    extra.insert("block".into(), json!(e.name));
    extra.insert("channel_label".into(), json!(value.name));
    extra.insert("time_axis_label".into(), json!(time.name));
    extra.insert("volume_resolution_ml".into(), json!(volume.factor));
    let mut start_min = 0.0;
    if time.second != 0.0 && time.second.is_finite() {
        extra.insert("time_offset_ms".into(), json!(time.second));
        start_min = time.second / 60_000.0;
    }
    Ok(Some(Curve {
        wavelength_nm: if kind == CurveKind::Uv {
            wavelength_of(&name)
        } else {
            None
        },
        name,
        kind,
        unit: (!unit.is_empty()).then_some(unit),
        samples,
        start_min,
        interval_min: time.factor,
        points: Points::Res {
            offset: e.offset + e.header,
            volume_factor: volume.factor,
            value_factor: value.factor,
        },
        original,
        extra,
    }))
}

/// Event records of an event block (after its 552-byte header).
fn event_records(b: &[u8]) -> Vec<Event> {
    let mut out = Vec::new();
    let mut pos = 552usize;
    while pos + EVENT_LEN <= b.len() {
        let time = le_f64(b, pos).unwrap_or(f64::NAN);
        let volume = le_f64(b, pos + 8).unwrap_or(f64::NAN);
        let first = text_field(&b[pos + 16..pos + 92]);
        let extra = text_field(&b[pos + 92..pos + 168]);
        let text = if extra.is_empty() {
            first
        } else if first.is_empty() {
            extra
        } else {
            format!("{first} {extra}")
        };
        out.push(Event {
            time_min: time,
            volume_ml: volume,
            text,
        });
        pos += EVENT_LEN;
    }
    out
}

/// Column name and volume from the method text: `0.00 Base Volume, 3.122 {ml}, <column>`.
fn column_of_methods(t: &str) -> Option<(String, Option<f64>)> {
    let line = t.lines().find(|l| l.contains("Base Volume,"))?;
    let rest = line.split_once("Base Volume,")?.1;
    let mut parts = rest.split(',');
    let vol = parts
        .next()
        .and_then(|s| s.split('{').next())
        .and_then(crate::model::num);
    let col = parts.next()?.trim();
    (!col.is_empty()).then(|| (col.to_string(), vol))
}

/// The system (instrument) name from a method dump or strategy notes.
fn system_model(texts: &Map<String, Value>) -> Option<(String, &'static str)> {
    if let Some(Value::String(t)) = texts.get("CreationNotes") {
        // `** AKTAprime   Ver  , V2.01`
        for l in t.lines() {
            let l = l.trim_start_matches('*').trim();
            if let Some((name, _)) = l.split_once(" Ver")
                && !name.trim().is_empty()
                && !name.contains("Dump Format")
            {
                return Some((
                    name.trim().to_string(),
                    "CreationNotes (method dump header)",
                ));
            }
        }
    }
    for key in ["ResultStrategyNotes", "MethodStrategyNotes"] {
        if let Some(Value::String(t)) = texts.get(key) {
            // `The strategy is designed for the systems:\n\nEttanLC\n`
            let mut lines = t.lines().map(str::trim);
            if lines.any(|l| l.starts_with("The strategy is designed for the system"))
                && let Some(name) = lines.find(|l| !l.is_empty())
            {
                return Some((
                    name.to_string(),
                    "strategy notes (designed for the systems)",
                ));
            }
        }
    }
    None
}

/// CHMO id of the technique a method or column names.
pub(crate) fn technique_of(t: &str) -> Option<&'static str> {
    let t = t.to_ascii_lowercase();
    if t.contains("size_exclusion")
        || t.contains("size exclusion")
        || t.contains("gelfiltration")
        || t.contains("gel filtration")
    {
        Some("CHMO:0001013")
    } else if t.contains("affinity") {
        Some("CHMO:0001006")
    } else if t.contains("ion_exchange")
        || t.contains("ion exchange")
        || t.contains("ionexchange")
        || t.contains("anion")
        || t.contains("cation")
    {
        Some("CHMO:0001014")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_names() {
        assert_eq!(
            split_block_name("2009Jun16no001:1_UV"),
            (Some("2009Jun16no001".into()), "UV".into())
        );
        assert_eq!(
            split_block_name("2 8 0p1003:10_UV1_215nm"),
            (Some("2 8 0p1003".into()), "UV1_215nm".into())
        );
        assert_eq!(split_block_name("=SAPA_215nm"), (None, "SAPA_215nm".into()));
    }

    #[test]
    fn method_text() {
        let t = "METHOD\nMAIN_SEPARATION\n0.00 Base Volume, 3.122 {ml}, Superdex_200_5/150_Gavin\n";
        assert_eq!(
            column_of_methods(t),
            Some(("Superdex_200_5/150_Gavin".into(), Some(3.122)))
        );
        assert_eq!(
            technique_of("START_TECHNIQUES\nSize_Exclusion\n"),
            Some("CHMO:0001013")
        );
        assert_eq!(technique_of("GelFiltration"), Some("CHMO:0001013"));
        assert_eq!(technique_of("Affinity"), Some("CHMO:0001006"));
        assert_eq!(technique_of("Desalting"), None);
    }
}
