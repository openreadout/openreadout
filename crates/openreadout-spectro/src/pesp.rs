//! PerkinElmer `.sp` files: `PEPE`, a 40-byte description, then nested blocks of typed members.
//!
//! Layout and vocabulary: `docs/formats/perkinelmer-sp.md`; provenance:
//! `docs/provenance/perkinelmer-sp.md`.

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::Source;
use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};
use serde_json::{Map, Value, json};

use crate::PESP_FORMAT_ID as FMT;
use crate::common::{Facts, Le, Parsed, Rows, SpectrumSet, Stored, XValues, num, read_at};

/// First four bytes of every `.sp` file.
pub(crate) const PESP_MAGIC: &[u8; 4] = b"PEPE";
/// Largest file whose block tree is read (the data are read lazily; the tree is small).
const MAX_TREE: u64 = 64 << 20;
/// Deepest block nesting read: every processing step nests the previous history record one
/// level (two blocks) further, and the instrument record sits under the oldest one.
const MAX_DEPTH: u32 = 256;

/// A typed member value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Member {
    Text(String),
    F64(f64),
    Pair(f64, f64),
    U32(u32),
    U16(u16),
    /// f64 array: file offset of the first value and the value count.
    Array(u64, u64),
    /// A value with a validity flag (`$u` suffix): the flag.
    Flagged(Box<Member>, bool),
}

/// Decode the typed member at the start of a block body. Types (little-endian u16 tags, ASCII
/// `?u`): 0x7523 text (u16 length), 0x751b f64, 0x751c f64 then a flag, 0x751d two f64,
/// 0x752b / 0x751a u32, 0x752c u16 then a flag, 0x7515 u16, 0x7516 f64 array (u32 byte length).
fn member(b: &[u8], origin: u64) -> Option<Member> {
    let t = b.u16_at(0)?;
    let flag = |at: usize| -> bool { b.u16_at(at) == Some(0x7524) && b.u16_at(at + 2) == Some(1) };
    Some(match t {
        0x7523 => {
            let n = usize::from(b.u16_at(2)?);
            let s = b.bytes_at(4, n)?;
            Member::Text(crate::common::text_field(s))
        }
        0x751b => Member::F64(b.f64_at(2)?),
        0x751c => Member::Flagged(Box::new(Member::F64(b.f64_at(2)?)), flag(10)),
        0x751d => Member::Pair(b.f64_at(2)?, b.f64_at(10)?),
        0x752b | 0x751a => Member::U32(b.u32_at(2)?),
        0x752c => Member::Flagged(Box::new(Member::U16(b.u16_at(2)?)), flag(4)),
        0x7515 => Member::U16(b.u16_at(2)?),
        0x7516 => {
            let n = u64::from(b.u32_at(2)?);
            Member::Array(origin + 6, n / 8)
        }
        _ => return None,
    })
}

/// A block of the tree: id, file offset of the body, body length and children or member.
#[derive(Debug)]
pub(crate) struct Node {
    pub(crate) id: u16,
    pub(crate) offset: u64,
    pub(crate) size: u64,
    pub(crate) member: Option<Member>,
    pub(crate) children: Vec<Node>,
}

pub(crate) fn tree(
    b: &[u8],
    start: usize,
    end: usize,
    origin: u64,
    depth: u32,
    bad: &mut bool,
) -> Vec<Node> {
    let mut out = Vec::new();
    let mut at = start;
    while at + 6 <= end {
        let (Some(id), Some(size)) = (b.u16_at(at), b.i32_at(at + 2)) else {
            break;
        };
        let Ok(size) = usize::try_from(size) else {
            *bad = true;
            break;
        };
        let body = at + 6;
        let Some(stop) = body.checked_add(size).filter(|s| *s <= end) else {
            *bad = true;
            break;
        };
        let slice = &b[body..stop];
        let is_member = slice.len() >= 2 && slice[1] == 0x75;
        let (m, kids) = if is_member {
            (member(slice, origin + body as u64), Vec::new())
        } else if depth < MAX_DEPTH && size >= 6 {
            // a container: keep the children read before any that does not fit (some files
            // end a data set with bytes that are not blocks)
            let mut inner_bad = false;
            (None, tree(b, body, stop, origin, depth + 1, &mut inner_bad))
        } else {
            (None, Vec::new())
        };
        out.push(Node {
            id,
            offset: origin + body as u64,
            size: size as u64,
            member: m,
            children: kids,
        });
        at = stop;
    }
    out
}

pub(crate) fn text_of(n: &Node) -> Option<&str> {
    match &n.member {
        Some(Member::Text(s)) if !s.is_empty() => Some(s),
        _ => None,
    }
}

pub(crate) fn number_of(n: &Node) -> Option<f64> {
    match &n.member {
        Some(Member::F64(v)) => Some(*v),
        Some(Member::U32(v)) => Some(f64::from(*v)),
        Some(Member::U16(v)) => Some(f64::from(*v)),
        Some(Member::Flagged(m, true)) => match m.as_ref() {
            Member::F64(v) => Some(*v),
            Member::U16(v) => Some(f64::from(*v)),
            _ => None,
        },
        _ => None,
    }
}

/// Every node, depth first.
pub(crate) fn walk<'a>(nodes: &'a [Node], out: &mut Vec<&'a Node>) {
    for n in nodes {
        out.push(n);
        walk(&n.children, out);
    }
}

pub(crate) fn json_tree(nodes: &[Node]) -> Value {
    Value::Array(
        nodes
            .iter()
            .map(|n| {
                let mut o = Map::new();
                o.insert("id".into(), json!(n.id));
                match &n.member {
                    Some(Member::Text(s)) => {
                        o.insert("text".into(), json!(s));
                    }
                    Some(Member::F64(v)) => {
                        o.insert("value".into(), num(*v));
                    }
                    Some(Member::Pair(a, b)) => {
                        o.insert("value".into(), json!([num(*a), num(*b)]));
                    }
                    Some(Member::U32(v)) => {
                        o.insert("value".into(), json!(v));
                    }
                    Some(Member::U16(v)) => {
                        o.insert("value".into(), json!(v));
                    }
                    Some(Member::Array(_, n)) => {
                        o.insert("values".into(), json!(n));
                    }
                    Some(Member::Flagged(m, ok)) => {
                        let v = match m.as_ref() {
                            Member::F64(v) => num(*v),
                            Member::U16(v) => json!(v),
                            _ => Value::Null,
                        };
                        o.insert("value".into(), v);
                        o.insert("set".into(), json!(ok));
                    }
                    None => {}
                }
                if !n.children.is_empty() {
                    o.insert("children".into(), json_tree(&n.children));
                }
                Value::Object(o)
            })
            .collect(),
    )
}

/// `Thu Mar 09 09:19:21 2006`, optionally followed by `… (GMT+1:00)`, as ISO 8601.
pub(crate) fn pe_time(s: &str) -> Option<String> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() < 5 {
        return None;
    }
    let month = match parts[1].to_ascii_lowercase().as_str() {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    };
    let day: u32 = parts[2].parse().ok()?;
    let clock: Vec<u32> = parts[3].split(':').filter_map(|x| x.parse().ok()).collect();
    let year: u32 = parts[4].parse().ok()?;
    if clock.len() != 3
        || !(1..=31).contains(&day)
        || clock[0] > 23
        || clock[1] > 59
        || clock[2] > 60
    {
        return None;
    }
    let zone = s.rfind("(GMT").and_then(|i| {
        let z = s[i + 4..].trim_end_matches(')');
        let (sign, rest) = z.split_at(z.len().min(1));
        if sign != "+" && sign != "-" {
            return None;
        }
        let (h, m) = rest.split_once(':').unwrap_or((rest, "0"));
        let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
        Some(format!("{sign}{h:02}:{m:02}"))
    });
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}{}",
        clock[0],
        clock[1],
        clock[2],
        zone.unwrap_or_default()
    ))
}

/// The history records (block 121: user 35698, operation 35699, date 35700, arguments 35701,
/// description 35702), newest first.
pub(crate) fn history_records(all: &[&Node]) -> Vec<Value> {
    all.iter()
        .filter(|n| n.id == 121 && !n.children.is_empty())
        .map(|n| {
            let g = |id: u16| n.children.iter().find(|c| c.id == id).and_then(text_of);
            json!({
                "user": g(35698), "operation": g(35699), "date": g(35700),
                "arguments": g(35701), "description": g(35702),
            })
        })
        .collect()
}

/// The instrument record (block 123: model 35837, serial 35838, firmware 35839) and its
/// settings, into `extra`. The scan count (35840) is left to the caller.
pub(crate) fn instrument_record(all: &[&Node], extra: &mut BTreeMap<String, Value>) {
    let find = |id: u16| all.iter().find(|n| n.id == id).copied();
    for (id, key) in [
        (35837u16, "instrument"),
        (35838, "instrument_serial"),
        (35839, "instrument_firmware"),
        (35841, "detector"),
        (35842, "source"),
        (35843, "beamsplitter"),
        (35845, "apodization"),
        (35846, "spectrum_type"),
        (35847, "beam_type"),
        (35849, "phase_correction"),
        (35854, "accessory"),
    ] {
        if let Some(v) = find(id).and_then(text_of) {
            extra.insert(key.into(), json!(v.trim_start_matches('/').trim()));
        }
    }
    if let Some(v) = find(35844).and_then(number_of) {
        extra.insert("resolution_cm1".into(), num(v));
    }
    if let Some(v) = find(35882).and_then(number_of) {
        extra.insert("laser_wavenumber_cm1".into(), num(v));
    }
}

/// The text settings of the instrument record as method parameters.
pub(crate) fn instrument_words(extra: &BTreeMap<String, Value>, facts: &mut Facts) {
    for k in [
        "detector",
        "source",
        "beamsplitter",
        "apodization",
        "accessory",
    ] {
        if let Some(v) = extra.get(k).and_then(Value::as_str) {
            facts.word(
                k,
                v,
                &format!("instrument settings ({k})"),
                Source::Inferred,
            );
        }
    }
}

/// Our x quantity, unit and JCAMP data type for the x unit text.
pub(crate) fn x_axis(unit: &str) -> (&'static str, Option<&'static str>, &'static str) {
    match unit.to_ascii_lowercase().as_str() {
        "cm-1" => ("wavenumber", Some("1/cm"), "INFRARED SPECTRUM"),
        "nm" => ("wavelength", Some("nm"), "UV/VIS SPECTRUM"),
        "um" | "µm" => ("wavelength", Some("µm"), "INFRARED SPECTRUM"),
        _ => ("x", None, "UNKNOWN"),
    }
}

/// Our y quantity and unit for the y unit text.
pub(crate) fn y_axis(unit: &str) -> (&'static str, Option<String>) {
    match unit {
        "A" => ("absorbance", Some("AU".to_string())),
        "%T" => ("transmittance", Some("%".to_string())),
        "%R" => ("reflectance", Some("%".to_string())),
        "KM" => ("kubelka_munk", None),
        "" => ("intensity", None),
        u => ("intensity", Some(u.to_string())),
    }
}

/// Where the data set block (120) keeps its spectrum.
struct DataMembers {
    first: f64,
    last: f64,
    points: u64,
    step: Option<f64>,
    data_at: u64,
    stored: u64,
}

impl DataMembers {
    fn of(main: &Node) -> Result<Self> {
        let child = |id: u16| main.children.iter().find(|n| n.id == id);
        let (first, last) = match child(35698).and_then(|n| n.member.clone()) {
            Some(Member::Pair(a, b)) => (a, b),
            _ => return Err(Error::corrupt(FMT, "the x range (member 35698) is missing")),
        };
        let points = child(35701).and_then(number_of).unwrap_or(0.0) as u64;
        let step = child(35700).and_then(number_of);
        let (data_at, stored) = match child(35708)
            .and_then(|n| n.children.first().or(Some(n)))
            .and_then(|n| n.member.clone())
        {
            Some(Member::Array(at, n)) => (at, n),
            _ => return Err(Error::corrupt(FMT, "the data member (35708) is missing")),
        };
        Ok(DataMembers {
            first,
            last,
            points,
            step,
            data_at,
            stored,
        })
    }
}

/// Parse a PerkinElmer `.sp` file.
pub(crate) fn parse(f: &SourceFile, path: &Path, file_len: u64) -> Result<Parsed> {
    if file_len > MAX_TREE {
        return Err(Error::unsupported(
            FMT,
            format!("a {file_len}-byte .sp file"),
            "Files over 64 MiB are not read; single spectra are a few hundred kilobytes.",
        ));
    }
    let b = read_at(f, path, 0, file_len, file_len)?;
    if b.len() < 44 || b[..4] != PESP_MAGIC[..] {
        return Err(Error::corrupt(
            FMT,
            "not a PerkinElmer .sp file: it does not start with PEPE and a 40-byte description",
        ));
    }
    let description = crate::common::text_field(&b[4..44]);
    let mut bad = false;
    let nodes = tree(&b, 44, b.len(), 0, 0, &mut bad);
    let mut parsed = Parsed::default();
    if bad {
        parsed.findings.push(Finding::error(
            "truncated",
            "a block runs past the end of the file or its parent",
        ));
    }
    let main = nodes
        .iter()
        .find(|n| n.id == 120)
        .ok_or_else(|| Error::corrupt(FMT, "no data set block (id 120) after the description"))?;
    let data = DataMembers::of(main)?;
    if data.points == 0 || data.stored < data.points {
        parsed.findings.push(Finding::error(
            "short_data",
            format!("{} stored values for {} points", data.stored, data.points),
        ));
    }
    let points = data.points.min(data.stored);
    let child_text = |id: u16| {
        main.children
            .iter()
            .find(|n| n.id == id)
            .and_then(text_of)
            .unwrap_or("")
            .to_string()
    };
    let x_unit = child_text(35703);
    let y_unit = child_text(35704);
    let data_type = child_text(35707);
    let name = child_text(35713);
    let source_path = child_text(35709);
    let (xq, xu, dtype) = x_axis(&x_unit);
    let (yq, yu) = y_axis(&y_unit);

    let mut all = Vec::new();
    walk(std::slice::from_ref(main), &mut all);
    let history = history_records(&all);
    let mut extra = BTreeMap::new();
    instrument_record(&all, &mut extra);
    if let Some(v) = all
        .iter()
        .find(|n| n.id == 35840)
        .copied()
        .and_then(number_of)
    {
        extra.insert("scans".into(), json!(v as i64));
    }
    if let Some(s) = data.step {
        extra.insert("data_interval".into(), num(s));
    }
    let mut facts = Facts::default();
    Facts::text(&mut facts.vendor, "PerkinElmer", "format");
    record_acquisition(&history, &mut extra, &mut facts);
    if !name.is_empty() {
        extra.insert("title".into(), json!(name));
        Facts::text(&mut facts.sample_name, &name, "data set name (35713)");
    }
    if !data_type.is_empty() {
        extra.insert("spectrum_kind".into(), json!(data_type));
    }
    extra.insert("x_units_text".into(), json!(x_unit));
    extra.insert("y_units_text".into(), json!(y_unit));
    instrument_facts(&extra, &mut facts);
    parsed.facts = facts;
    parsed.sets.push(SpectrumSet {
        name: if name.is_empty() {
            yq.to_string()
        } else {
            name.clone()
        },
        x_quantity: xq,
        x_unit: xu.map(str::to_string),
        x: XValues::Regular {
            first: data.first,
            last: data.last,
        },
        y_name: yq.into(),
        y_unit: yu,
        points,
        count: 1,
        rows: Rows::Listed(vec![data.data_at]),
        stored: Stored::F64,
        scale: 1.0,
        data_type: dtype,
        extra,
    });
    if let Some(s) = data.step
        && points > 1
        && ((data.last - data.first) / (points - 1) as f64 - s).abs() > 1e-6 * s.abs().max(1.0)
    {
        parsed.findings.push(Finding::warning(
            "x_interval",
            format!("the stored interval {s} disagrees with the x range over {points} points"),
        ));
    }
    parsed.vendor = json!({
        "description": description,
        "source_path": source_path,
        "history": history,
        "blocks": json_tree(&nodes),
    });
    for n in &nodes {
        parsed.entries.push(LsEntry {
            kind: "block".into(),
            name: format!("block {}", n.id),
            offset: Some(n.offset.saturating_sub(6)),
            size: Some(n.size + 6),
            image: None,
            details: json!({"id": n.id, "children": n.children.len()}),
        });
    }
    parsed
        .notes
        .push(format!("PerkinElmer data set file: {description}"));
    for (k, v) in [
        ("traces[].sample_count", Source::PriorArt),
        ("traces[].extra.axis", Source::PriorArt),
        ("traces[].extra.y_quantity", Source::Inferred),
        ("traces[].extra.resolution_cm1", Source::Inferred),
        ("traces[].extra.scans", Source::Inferred),
        ("traces[].extra.instrument", Source::PriorArt),
        ("traces[].extra.acquired_at", Source::Inferred),
    ] {
        parsed.provenance.insert(k.into(), v);
    }
    Ok(parsed)
}

/// The acquisition (the "Created as New Dataset" record, else the oldest one): its date,
/// user and description.
fn record_acquisition(history: &[Value], extra: &mut BTreeMap<String, Value>, facts: &mut Facts) {
    let created = history
        .iter()
        .find(|h| {
            h["operation"]
                .as_str()
                .is_some_and(|o| o.to_ascii_lowercase().contains("created"))
        })
        .or_else(|| history.last());
    let Some(h) = created else {
        return;
    };
    if let Some(t) = h["date"].as_str().and_then(pe_time) {
        extra.insert("acquired_at".into(), json!(t));
        Facts::text(&mut facts.started_at, &t, "history record date (35700)");
    }
    if let Some(u) = h["user"].as_str() {
        extra.insert("operator".into(), json!(u));
        Facts::text(&mut facts.operator, u, "history record user (35698)");
    }
    if let Some(d) = h["description"].as_str() {
        Facts::text(&mut facts.comment, d, "history record description (35702)");
    }
}

/// The instrument model and serial, and the numeric and text settings, as facts.
fn instrument_facts(extra: &BTreeMap<String, Value>, facts: &mut Facts) {
    if let Some(v) = extra.get("instrument").and_then(Value::as_str) {
        Facts::text(&mut facts.model, v, "instrument record model (35837)");
    }
    if let Some(v) = extra.get("instrument_serial").and_then(Value::as_str) {
        Facts::text(&mut facts.serial, v, "instrument record serial (35838)");
    }
    let inf = Source::Inferred;
    if let Some(v) = extra.get("resolution_cm1").and_then(Value::as_f64) {
        facts.number(
            "resolution",
            v,
            Some("cm⁻¹"),
            "instrument settings (35844)",
            inf,
        );
    }
    if let Some(v) = extra.get("scans").and_then(Value::as_f64) {
        facts.number("scans", v, None, "instrument settings (35840)", inf);
    }
    if let Some(v) = extra.get("laser_wavenumber_cm1").and_then(Value::as_f64) {
        facts.number(
            "laser_wavenumber",
            v,
            Some("cm⁻¹"),
            "instrument settings (35882)",
            inf,
        );
    }
    instrument_words(extra, facts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        assert_eq!(
            pe_time("Thu Mar 09 09:19:21 2006").as_deref(),
            Some("2006-03-09T09:19:21")
        );
        assert_eq!(
            pe_time("tue oct 22 12:24:18 2019 GMT Daylight Time (GMT+1:00)").as_deref(),
            Some("2019-10-22T12:24:18+01:00")
        );
        assert_eq!(pe_time("nonsense"), None);
    }

    #[test]
    fn members() {
        let mut b = vec![0x23, 0x75, 3, 0];
        b.extend_from_slice(b"abc");
        assert_eq!(member(&b, 0), Some(Member::Text("abc".into())));
        let mut p = vec![0x1d, 0x75];
        p.extend_from_slice(&4000f64.to_le_bytes());
        p.extend_from_slice(&700f64.to_le_bytes());
        assert_eq!(member(&p, 0), Some(Member::Pair(4000.0, 700.0)));
        assert_eq!(member(&p[..8], 0), None);
        let mut fl = vec![0x1c, 0x75];
        fl.extend_from_slice(&4f64.to_le_bytes());
        fl.extend_from_slice(&[0x24, 0x75, 1, 0]);
        assert_eq!(
            member(&fl, 0),
            Some(Member::Flagged(Box::new(Member::F64(4.0)), true))
        );
    }
}
