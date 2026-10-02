//! SoftMax Pro 6/7 binary documents (`.sda`, `.pda`): a self-describing little-endian tree
//! (docs/formats/plate-readers.md, "SoftMax Pro documents"; provenance 2026-09-25).
//!
//! `\x0bBinary File` + u32 1, then entries `u32 tag` (type in bits 8–15) + 7-bit-length name +
//! value. Objects carry their size, so an unknown value type ends only the object it is in.
//! Plate sections follow the string `…SerializablePlateSectionData`; their reader settings are
//! a nested tree and their values a byte array of rows × columns f64 per read.

use openreadout_core::model::Finding;
use serde_json::json;

use super::cursor::Cursor;
use crate::model::{Block, Channel, Export, Kind, Mode, ReadType};
use crate::sheet::Container;

const MAGIC: &[u8] = b"\x0bBinary File";
/// Nesting deeper than this is not a SoftMax document (the corpus files nest 12 levels).
const MAX_DEPTH: usize = 64;
/// Entries per document; the corpus files hold about 10,000.
const MAX_NODES: usize = 4_000_000;

pub(crate) fn sniff(head: &[u8]) -> bool {
    head.starts_with(MAGIC) && head.get(12..16) == Some(&[1, 0, 0, 0])
}

/// A SoftMax Pro document rather than another program's `Binary File`: the `Document`
/// object names `ProtocolAndData` near the start.
pub(crate) fn sniff_document(head: &[u8]) -> bool {
    sniff(head) && super::cursor::find(head, b"ProtocolAndData", 0, 512).is_some()
}

/// One entry of the tree.
#[derive(Debug, Clone)]
pub(crate) enum Val<'a> {
    Str(String),
    F64(f64),
    I32(i32),
    Bytes(&'a [u8]),
    /// An object, or a byte array that holds a nested `Binary File`.
    Obj(Vec<Node<'a>>),
    Bool(u8),
    F32(f32),
    /// A value whose meaning we have not established (date/time ticks, 2- and 4-byte codes).
    Other,
}

#[derive(Debug, Clone)]
pub(crate) struct Node<'a> {
    pub(crate) name: String,
    pub(crate) val: Val<'a>,
}

impl<'a> Node<'a> {
    fn kids(&self) -> &[Node<'a>] {
        match &self.val {
            Val::Obj(k) => k,
            _ => &[],
        }
    }
    fn child(&self, name: &str) -> Option<&Node<'a>> {
        self.kids().iter().find(|k| k.name == name)
    }
    fn path(&self, names: &[&str]) -> Option<&Node<'a>> {
        let mut n = self;
        for p in names {
            n = n.child(p)?;
        }
        Some(n)
    }
    fn str_(&self) -> Option<&str> {
        match &self.val {
            Val::Str(s) => Some(s),
            _ => None,
        }
    }
    fn i32_(&self) -> Option<i32> {
        match self.val {
            Val::I32(v) => Some(v),
            _ => None,
        }
    }
    fn f64_(&self) -> Option<f64> {
        match self.val {
            Val::F64(v) => Some(v),
            _ => None,
        }
    }
    fn bool_(&self) -> Option<bool> {
        match self.val {
            Val::Bool(v) => Some(v != 0),
            _ => None,
        }
    }
    /// An enum wrapper object (`ReadType { value__ }`).
    fn enum_(&self) -> Option<i32> {
        self.child("value__")?.i32_()
    }
}

struct Parser {
    nodes: usize,
    /// Objects cut short (unknown value type, bad tag); their earlier children are kept.
    truncated: usize,
}

impl Parser {
    fn string(c: &mut Cursor) -> Option<String> {
        let mut n: usize = 0;
        let mut shift = 0u32;
        loop {
            let b = c.u8()?;
            n |= usize::from(b & 0x7f).checked_shl(shift)?;
            if b < 0x80 {
                break;
            }
            shift += 7;
            if shift > 28 {
                return None;
            }
        }
        let bytes = c.take(n)?;
        Some(String::from_utf8_lossy(bytes).into_owned())
    }

    /// Entries in `data[pos..end]`.
    fn items<'a>(&mut self, data: &'a [u8], pos: usize, end: usize, depth: usize) -> Vec<Node<'a>> {
        let mut out = Vec::new();
        let mut c = Cursor::new(data, pos);
        if depth > MAX_DEPTH {
            self.truncated += 1;
            return out;
        }
        while c.pos < end {
            if self.nodes >= MAX_NODES {
                self.truncated += 1;
                break;
            }
            let start = c.pos;
            let Some(tag) = c.u32_le() else {
                self.truncated += 1;
                break;
            };
            if tag & 0xffff_00ff != 0 {
                self.truncated += 1;
                break;
            }
            let typ = (tag >> 8) & 0xff;
            self.nodes += 1;
            if typ == 0x14 {
                let (Some(size), Some(name)) = (c.u64_le(), Self::string(&mut c)) else {
                    self.truncated += 1;
                    break;
                };
                let obj_end = usize::try_from(size)
                    .ok()
                    .and_then(|s| start.checked_add(s))
                    .filter(|&e| e <= end && e > start);
                let (Some(obj_end), Some(_marker)) = (obj_end, c.u32_le()) else {
                    self.truncated += 1;
                    break;
                };
                let kids = if c.pos <= obj_end {
                    self.items(data, c.pos, obj_end, depth + 1)
                } else {
                    self.truncated += 1;
                    Vec::new()
                };
                out.push(Node {
                    name,
                    val: Val::Obj(kids),
                });
                c.pos = obj_end;
                continue;
            }
            let Some(name) = Self::string(&mut c) else {
                self.truncated += 1;
                break;
            };
            let val = match typ {
                0x04 => Self::string(&mut c).map(Val::Str),
                0x08 => c.f64_le().map(Val::F64),
                0x0c => c.i32_le().map(Val::I32),
                0x10 => c
                    .u64_le()
                    .and_then(|n| usize::try_from(n).ok())
                    .and_then(|n| c.take(n))
                    .map(|b| {
                        if b.starts_with(MAGIC) && b.len() >= 16 {
                            Val::Obj(self.items(b, 16, b.len(), depth + 1))
                        } else {
                            Val::Bytes(b)
                        }
                    }),
                0x18 => c.skip(8).map(|()| Val::Other),
                0x1c => c.f32_le().map(Val::F32),
                0x20 => c.u8().map(Val::Bool),
                0x28 => c.skip(2).map(|()| Val::Other),
                0x2c => c.skip(4).map(|()| Val::Other),
                _ => None,
            };
            let Some(val) = val else {
                self.truncated += 1;
                break;
            };
            if c.pos > end {
                self.truncated += 1;
                break;
            }
            out.push(Node { name, val });
        }
        out
    }
}

/// The document tree and how many objects were cut short.
pub(crate) fn tree(data: &[u8]) -> Option<(Vec<Node<'_>>, usize)> {
    if !sniff(data) {
        return None;
    }
    let mut p = Parser {
        nodes: 0,
        truncated: 0,
    };
    let nodes = p.items(data, 16, data.len(), 0);
    Some((nodes, p.truncated))
}

/// Plate section objects: the object after each `…SerializablePlateSectionData` string, and
/// the class names of every section in document order.
fn sections<'n, 'a>(
    nodes: &'n [Node<'a>],
    plates: &mut Vec<&'n Node<'a>>,
    classes: &mut Vec<String>,
) {
    for (i, n) in nodes.iter().enumerate() {
        if let Some(s) = n.str_()
            && s.starts_with("SoftMaxPro.DataPersistence.Serializable")
            && s.ends_with("SectionData")
        {
            classes.push(
                s.trim_start_matches("SoftMaxPro.DataPersistence.Serializable")
                    .to_string(),
            );
            if s.ends_with("PlateSectionData")
                && let Some(next) = nodes.get(i + 1)
                && matches!(next.val, Val::Obj(_))
            {
                plates.push(next);
            }
        }
        sections(n.kids(), plates, classes);
    }
}

pub(crate) fn parse(data: &[u8]) -> Option<Export> {
    let (nodes, truncated) = tree(data)?;
    let mut ex = Export::new(
        Kind::SoftMaxPro,
        Container::Binary {
            kind: "softmax-pro-document",
        },
    );
    ex.put("document", "SoftMax Pro 6/7 binary document");
    if truncated > 0 {
        ex.findings.push(Finding::warning(
            "document_truncated",
            format!("{truncated} objects of the document could not be read to their end (an unknown value type or a damaged entry); values outside them are unaffected"),
        ));
    }
    let mut plates = Vec::new();
    let mut classes = Vec::new();
    sections(&nodes, &mut plates, &mut classes);
    ex.sections.insert("section_classes".into(), json!(classes));
    // experiment name: the second string of the `Document` object (`ProtocolAndData`, name)
    if let Some(doc) = find_obj(&nodes, "Document", 4) {
        let strs: Vec<&str> = doc.kids().iter().filter_map(Node::str_).take(2).collect();
        if let Some(name) = strs.get(1) {
            ex.experiment = Some((*name).to_string());
        }
    }
    let mut refused = Vec::new();
    for sec in plates {
        match plate(sec, &mut ex) {
            Ok(b) => ex.blocks.push(b),
            Err(why) => refused.push(why),
        }
    }
    if !refused.is_empty() {
        ex.sections.insert("refused_plates".into(), json!(refused));
        ex.findings.push(Finding::warning(
            "plate_not_decoded",
            format!(
                "{} plate section(s) are not decoded because their layout has not been validated: {}; export them as text from SoftMax Pro",
                refused.len(),
                refused.join("; ")
            ),
        ));
    }
    Some(ex)
}

fn find_obj<'n, 'a>(nodes: &'n [Node<'a>], name: &str, depth: usize) -> Option<&'n Node<'a>> {
    for n in nodes {
        if n.name == name && matches!(n.val, Val::Obj(_)) {
            return Some(n);
        }
        if depth > 0
            && let Some(f) = find_obj(n.kids(), name, depth - 1)
        {
            return Some(f);
        }
    }
    None
}

/// One plate section as a block, or why it is refused.
fn plate(sec: &Node, ex: &mut Export) -> Result<Block, String> {
    let name = sec
        .child("Name")
        .and_then(Node::str_)
        .unwrap_or("")
        .to_string();
    let label = if name.is_empty() {
        "unnamed plate".to_string()
    } else {
        format!("plate {name:?}")
    };
    let kids = sec.kids();
    // the reader: class name string, model string, then the nested settings tree
    let reader_class = kids
        .iter()
        .filter_map(Node::str_)
        .find(|s| s.starts_with("SoftMaxPro.Readers."))
        .map(str::to_string);
    let settings = kids.iter().find_map(|k| k.child("ReaderSettings"));
    let info = kids.iter().find(|k| k.child("DeviceInfo").is_some());
    let data = kids.iter().find(|k| {
        let items = k.kids();
        items.len() > 9 && items[0].str_() == Some(name.as_str()) && items[1].i32_().is_some()
    });
    let settings = settings.ok_or_else(|| format!("{label}: no reader settings"))?;
    let Some(data) = data else {
        return Err(format!("{label}: holds no plate data (not read)"));
    };
    let read_type = settings.child("ReadType").and_then(Node::enum_);
    let read_mode = settings.child("ReadMode").and_then(Node::enum_);
    if read_type != Some(0) {
        return Err(format!(
            "{label}: read type {} (only endpoint reads are validated)",
            read_type.map_or_else(|| "missing".into(), |v| v.to_string())
        ));
    }
    let mode = match read_mode {
        Some(0) => Mode::Fluorescence,
        Some(1) => Mode::Absorbance,
        Some(2) => Mode::Luminescence,
        other => {
            return Err(format!(
                "{label}: read mode {} (fluorescence, absorbance and luminescence are validated)",
                other.map_or_else(|| "missing".into(), |v| v.to_string())
            ));
        }
    };
    let wl_list: Vec<&Node> = settings
        .path(&["WavelengthSettings"])
        .map(|w| {
            w.kids()
                .iter()
                .filter(|k| k.name.ends_with("Wavelengths") || k.name == "WavelengthList")
                .flat_map(|l| l.kids().iter())
                .collect()
        })
        .unwrap_or_default();
    if wl_list.len() != 1 {
        return Err(format!(
            "{label}: {} wavelengths (one is validated)",
            wl_list.len()
        ));
    }
    let wl = wl_list[0];
    let items = data.kids();
    let ints: Vec<Option<i32>> = items[1..6].iter().map(Node::i32_).collect();
    let (Some(cols), Some(rows)) = (ints[3], ints[4]) else {
        return Err(format!("{label}: plate size missing"));
    };
    let (Ok(cols), Ok(rows)) = (u32::try_from(cols), u32::try_from(rows)) else {
        return Err(format!("{label}: bad plate size"));
    };
    if cols == 0 || rows == 0 || cols > 48 || rows > 32 {
        return Err(format!("{label}: plate size {rows} x {cols}"));
    }
    let spec = settings.path(&["Plate", "Microplate", "PlateSpecification"]);
    if let Some(spec) = spec {
        let n = spec.child("NumberOfWells").and_then(Node::i32_);
        let sc = spec.child("NumberOfColumns").and_then(Node::i32_);
        let sr = spec.child("NumberOfRows").and_then(Node::i32_);
        if sc.is_some_and(|v| v != cols as i32) || sr.is_some_and(|v| v != rows as i32) {
            return Err(format!(
                "{label}: data are {rows} x {cols} but the plate type is {} x {}",
                sr.unwrap_or(0),
                sc.unwrap_or(0)
            ));
        }
        if let Some(n) = n
            && n != (rows * cols) as i32
        {
            return Err(format!(
                "{label}: plate type declares {n} wells, data {rows} x {cols}"
            ));
        }
    }
    let wls = items
        .iter()
        .find(|k| matches!(k.val, Val::Obj(_)) && k.kids().first().is_some_and(|f| f.name == "0"));
    let wls = wls.map(Node::kids).unwrap_or_default();
    if wls.len() != 1 {
        return Err(format!(
            "{label}: {} wavelength data sets (one is validated)",
            wls.len()
        ));
    }
    let reads = wls[0]
        .kids()
        .iter()
        .find(|k| matches!(k.val, Val::Obj(_)))
        .map(Node::kids)
        .unwrap_or_default();
    if reads.len() != 1 {
        return Err(format!(
            "{label}: {} reads per wavelength (one is validated)",
            reads.len()
        ));
    }
    let rd = reads[0].kids();
    let temp = rd.iter().find_map(|k| match k.val {
        Val::F32(t) => Some(f64::from(t)),
        _ => None,
    });
    let raw = rd.iter().find_map(|k| match k.val {
        Val::Bytes(b) => Some(b),
        _ => None,
    });
    let n = (rows * cols) as usize;
    let Some(raw) = raw.filter(|b| b.len() == n * 8) else {
        return Err(format!(
            "{label}: the value array is not {rows} x {cols} numbers"
        ));
    };
    let mut block = Block::new(name.clone(), name.clone());
    block.rows = rows;
    block.cols = cols;
    block.declared_wells = Some(rows * cols);
    block.read_type = Some(ReadType::Endpoint);
    block.temperature_c = temp.map(|t| (t * 100.0).round() / 100.0);
    let unit = match mode {
        Mode::Absorbance => "OD",
        Mode::Fluorescence => "RFU",
        _ => "RLU",
    };
    let mut ch = Channel::new(mode_label(mode, wl), mode);
    ch.unit = Some(unit.into());
    match mode {
        Mode::Absorbance => ch.wavelength_nm = wl.child("Wavelength").and_then(Node::f64_),
        Mode::Fluorescence => {
            ch.excitation_nm = wl.child("ExcitationWavelength").and_then(Node::f64_);
            ch.emission_nm = wl.child("EmissionWavelength").and_then(Node::f64_);
            if let Some(cut) = wl.child("CutoffFilter").and_then(Node::f64_) {
                ch.settings.insert("cutoff_nm".into(), json!(cut));
            }
        }
        _ => {
            let all = wl.child("IsAll").and_then(Node::bool_) == Some(true);
            ch.emission_nm = wl
                .child("Wavelength")
                .and_then(Node::f64_)
                .filter(|w| *w > 0.0 && !all);
        }
    }
    if let Some(s) = settings.child("SensitivitySettings") {
        if matches!(mode, Mode::Fluorescence)
            && let Some(bottom) = s.child("IsReadFromBottom").and_then(Node::bool_)
        {
            ch.settings.insert(
                "optics".into(),
                json!(if bottom { "Bottom" } else { "Top" }),
            );
        }
        if s.child("IsFlashesPerReadEnable").and_then(Node::bool_) == Some(true)
            && let Some(f) = s.child("FlashesPerRead").and_then(Node::i32_)
        {
            ch.settings.insert("reads_per_well".into(), json!(f));
        }
    }
    let chi = block.channel(ch);
    for (i, v) in raw.as_chunks::<8>().0.iter().enumerate() {
        let v = f64::from_le_bytes(*v);
        let (r, col) = ((i / cols as usize) as u32, (i % cols as usize) as u32);
        let text = (!v.is_finite()).then(|| "no value".to_string());
        block.push_value(r, col, chi, None, v, text);
    }
    // the well-flag bytes after the reads: all zero in every corpus file (meaning unknown)
    if let Some(flags) = items.iter().rev().find_map(|k| match k.val {
        Val::Bytes(f) if f.len() == n => Some(f),
        _ => None,
    }) && flags.iter().any(|&f| f != 0)
    {
        block.findings.push(Finding::info(
            "well_flags_set",
            format!(
                "{name}: {} wells carry a flag byte whose meaning is not established (masked wells?); their values are reported as stored",
                flags.iter().filter(|&&f| f != 0).count()
            ),
        ));
    }
    if let Some(p) = settings.path(&["Plate", "Microplate", "MicroplateName"])
        && let Some(s) = p.str_()
    {
        block.plate_type = Some(s.to_string());
    }
    if let Some(cls) = reader_class {
        block
            .extra
            .insert("reader_settings_class".into(), json!(cls));
    }
    if let Some(info) = info {
        for key in ["DeviceInfo", "TemperatureInfo", "ReadDetails"] {
            if let Some(s) = info.child(key).and_then(Node::str_) {
                let t = s.trim();
                if !t.is_empty() {
                    block.extra.insert(
                        match key {
                            "DeviceInfo" => "device_info",
                            "TemperatureInfo" => "temperature_info",
                            _ => "read_details",
                        }
                        .into(),
                        json!(t),
                    );
                }
            }
        }
        if let Some(d) = info.child("DeviceInfo").and_then(Node::str_) {
            let mut lines = d.lines();
            if ex.model.is_none()
                && let Some(m) = lines.next()
            {
                ex.model = Some(m.trim().to_string());
            }
        }
        if let Some(rd) = info.child("ReadDetails").and_then(Node::str_)
            && let Some(when) = rd.split("Start Read :").nth(1)
        {
            // `1:24 PM 9/1/2021` or `16:29 09/06/2020`: clock, then date (the PC's locale)
            let when = when.trim();
            let parts: Vec<&str> = when.split_whitespace().collect();
            let ampm = parts
                .get(1)
                .is_some_and(|p| p.eq_ignore_ascii_case("AM") || p.eq_ignore_ascii_case("PM"));
            let (time, date) = if ampm {
                (parts.get(..2).map(|p| p.join(" ")), parts.get(2))
            } else {
                (parts.first().map(|p| (*p).to_string()), parts.get(1))
            };
            if let (Some(time), Some(date)) = (time, date)
                && let Some((iso, assumed)) = crate::datetime::combine(date, Some(&time))
            {
                block.started_at = Some(iso.clone());
                if ex.acquired_at.is_none() {
                    ex.acquired_at = Some(iso);
                    ex.acquired_raw = Some(when.to_string());
                    ex.date_order_assumed = assumed;
                }
            }
        }
    }
    Ok(block)
}

fn mode_label(mode: Mode, wl: &Node) -> String {
    let f = |k: &str| {
        wl.child(k)
            .and_then(Node::f64_)
            .map_or_else(|| "?".into(), crate::sheet::fmt_num)
    };
    match mode {
        Mode::Absorbance => format!("Absorbance {}", f("Wavelength")),
        Mode::Fluorescence => format!(
            "Fluorescence ex {} em {}",
            f("ExcitationWavelength"),
            f("EmissionWavelength")
        ),
        _ => "Luminescence".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(typ: u8, name: &str, val: &[u8]) -> Vec<u8> {
        let mut v = vec![0, typ, 0, 0, name.len() as u8];
        v.extend_from_slice(name.as_bytes());
        v.extend_from_slice(val);
        v
    }

    fn obj(name: &str, kids: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = kids.concat();
        let size = 4 + 8 + 1 + name.len() + 4 + body.len();
        let mut v = vec![0, 0x14, 0, 0];
        v.extend_from_slice(&(size as u64).to_le_bytes());
        v.push(name.len() as u8);
        v.extend_from_slice(name.as_bytes());
        v.extend_from_slice(&0xec8bu32.to_le_bytes());
        v.extend_from_slice(&body);
        v
    }

    fn string(s: &str) -> Vec<u8> {
        let mut v = vec![s.len() as u8];
        v.extend_from_slice(s.as_bytes());
        v
    }

    #[test]
    fn a_two_well_luminescence_plate() {
        let settings = obj(
            "ReaderSettings",
            &[
                obj("ReadType", &[entry(0x0c, "value__", &0i32.to_le_bytes())]),
                obj("ReadMode", &[entry(0x0c, "value__", &2i32.to_le_bytes())]),
                obj(
                    "WavelengthSettings",
                    &[obj(
                        "LuminescenceWavelengths",
                        &[obj(
                            "LuminescenceWavelengths0",
                            &[
                                entry(0x20, "IsAll", &[1]),
                                entry(0x08, "Wavelength", &0f64.to_le_bytes()),
                            ],
                        )],
                    )],
                ),
            ],
        );
        let mut nested = b"\x0bBinary File\x01\0\0\0".to_vec();
        nested.extend_from_slice(&settings);
        let mut raw = Vec::new();
        for v in [1.5f64, 2.5] {
            raw.extend_from_slice(&v.to_le_bytes());
        }
        let bytes = |b: &[u8]| {
            let mut v = (b.len() as u64).to_le_bytes().to_vec();
            v.extend_from_slice(b);
            v
        };
        let read = obj(
            "0",
            &[
                entry(0x0c, "", &0i32.to_le_bytes()),
                entry(0x1c, "", &25.0f32.to_le_bytes()),
                entry(0x08, "", &0f64.to_le_bytes()),
                entry(0x10, "", &bytes(&raw)),
            ],
        );
        let data = obj(
            "",
            &[
                entry(0x04, "", &string("P1")),
                entry(0x0c, "", &0i32.to_le_bytes()),
                entry(0x0c, "", &1i32.to_le_bytes()),
                entry(0x0c, "", &1i32.to_le_bytes()),
                entry(0x0c, "", &2i32.to_le_bytes()),
                entry(0x0c, "", &1i32.to_le_bytes()),
                entry(0x08, "", &0f64.to_le_bytes()),
                entry(0x08, "", &0f64.to_le_bytes()),
                obj(
                    "",
                    &[obj(
                        "0",
                        &[entry(0x0c, "", &0i32.to_le_bytes()), obj("", &[read])],
                    )],
                ),
                entry(0x10, "", &bytes(&[0, 0])),
            ],
        );
        let section = obj(
            "",
            &[
                entry(0x04, "Name", &string("P1")),
                entry(
                    0x04,
                    "",
                    &string("SoftMaxPro.Readers.SettingsModel.TestSettings"),
                ),
                entry(0x10, "", &bytes(&nested)),
                data,
            ],
        );
        let mut doc = b"\x0bBinary File\x01\0\0\0".to_vec();
        doc.extend_from_slice(&entry(
            0x04,
            "",
            &string("SoftMaxPro.DataPersistence.SerializablePlateSectionData"),
        ));
        doc.extend_from_slice(&section);
        assert!(sniff(&doc));
        let ex = parse(&doc).unwrap();
        assert!(ex.findings.is_empty(), "{:?}", ex.findings);
        let b = &ex.blocks[0];
        assert_eq!(b.name, "P1");
        assert_eq!(b.channels[0].mode, Mode::Luminescence);
        assert_eq!(b.obs.len(), 2);
        assert_eq!(b.obs[1].value.to_bits(), 2.5f64.to_bits());
        assert_eq!((b.obs[1].row, b.obs[1].col), (0, 1));
        assert_eq!(b.temperature_c, Some(25.0));
        // cutting the document anywhere never panics
        for cut in 0..doc.len() {
            let _ = parse(&doc[..cut]);
        }
    }
}
