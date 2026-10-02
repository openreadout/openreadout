//! Renishaw WiRE `.wdf` files: a chain of named blocks (header, spectra, x list, origin lists,
//! map geometry, white-light image, property sets).
//!
//! Layout and vocabulary: `docs/formats/renishaw-wdf.md`; provenance:
//! `docs/provenance/renishaw-wdf.md`.

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::Source;
use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};
use serde_json::{Map, Value, json};

use crate::WDF_FORMAT_ID as FMT;
use crate::common::{
    Attachment, Facts, Le, MapSpec, Parsed, Rows, SpectrumSet, SpectrumTable, Stored, XValues,
    nm_of_wavenumber, num, read_at, text_field,
};

/// First four bytes of every `.wdf` file (the header block's name).
pub(crate) const WDF_MAGIC: &[u8; 4] = b"WDF1";
/// Most blocks walked.
const MAX_BLOCKS: usize = 100_000;
/// Largest property set decoded (bytes).
const MAX_PSET: u64 = 16 << 20;
/// Ticks (100 ns) from 1601-01-01 to the Unix epoch.
const FILETIME_UNIX: i64 = 116_444_736_000_000_000;

/// One block of the chain.
#[derive(Debug, Clone)]
struct Block {
    name: String,
    uid: i32,
    offset: u64,
    size: u64,
}

/// ISO 8601 UTC of a Windows FILETIME (100 ns ticks since 1601), when plausible.
pub(crate) fn filetime_iso(ticks: u64) -> Option<String> {
    let t = i64::try_from(ticks).ok()?;
    if t <= FILETIME_UNIX {
        return None;
    }
    let unix = (t - FILETIME_UNIX) / 10_000_000;
    let frac = (t - FILETIME_UNIX) % 10_000_000;
    let base = crate::omnic::iso_utc(unix);
    Some(format!(
        "{}.{:03}Z",
        base.trim_end_matches('Z'),
        frac / 10_000
    ))
}

/// A decoded property-set value.
#[derive(Debug, Clone)]
enum PVal {
    Int(i64),
    Float(f64),
    Text(String),
    Time(u64),
    Bytes(usize),
    Array(Vec<f64>),
    Set(Vec<(u16, PVal)>),
}

/// Decode property-set items: a type byte, a flag byte (0x80: array), a 16-bit key and the
/// value. `k` items name keys; `p` items nest a set. Returns the items, the names and whether
/// the whole buffer was understood.
fn pset_items(b: &[u8], names: &mut BTreeMap<u16, String>, depth: u32) -> (Vec<(u16, PVal)>, bool) {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 4 <= b.len() {
        let t = b[at];
        let flag = b[at + 1];
        let key = b.u16_at(at + 2).unwrap_or(0);
        at += 4;
        if t == 0 {
            // zero padding after the last item
            return (out, b[at - 4..].iter().all(|&c| c == 0));
        }
        let fixed = match t {
            b'?' | b'c' => Some(1usize),
            b's' | b'w' => Some(2),
            b'i' | b'l' | b'r' => Some(4),
            b'q' | b'd' | b't' => Some(8),
            _ => None,
        };
        if flag & 0x80 != 0 {
            // array: u32 count, then count elements
            let (Some(n), Some(w)) = (b.u32_at(at), fixed) else {
                return (out, false);
            };
            at += 4;
            let Some(bytes) = b.bytes_at(at, (n as usize).saturating_mul(w)) else {
                return (out, false);
            };
            let vals = bytes
                .chunks_exact(w)
                .map(|c| {
                    scalar(t, c).map_or(f64::NAN, |v| match v {
                        PVal::Int(i) => i as f64,
                        PVal::Float(f) => f,
                        PVal::Time(x) => x as f64,
                        _ => f64::NAN,
                    })
                })
                .collect();
            at += bytes.len();
            out.push((key, PVal::Array(vals)));
            continue;
        }
        if let Some(w) = fixed {
            let Some(bytes) = b.bytes_at(at, w) else {
                return (out, false);
            };
            if let Some(v) = scalar(t, bytes) {
                out.push((key, v));
            }
            at += w;
            continue;
        }
        let Some(n) = b.u32_at(at) else {
            return (out, false);
        };
        at += 4;
        let Some(bytes) = b.bytes_at(at, n as usize) else {
            return (out, false);
        };
        at += n as usize;
        match t {
            b'u' => {
                if flag & 0x40 != 0 {
                    out.push((key, PVal::Bytes(bytes.len())));
                } else {
                    out.push((key, PVal::Text(text_field(bytes))));
                }
            }
            b'k' => {
                names.insert(key, text_field(bytes));
            }
            b'b' => out.push((key, PVal::Bytes(bytes.len()))),
            b'p' => {
                if depth > 32 {
                    return (out, false);
                }
                let inner = if bytes.starts_with(b"PSET") {
                    bytes.get(8..).unwrap_or_default()
                } else {
                    bytes
                };
                let (items, ok) = pset_items(inner, names, depth + 1);
                out.push((key, PVal::Set(items)));
                if !ok {
                    return (out, false);
                }
            }
            _ => return (out, false),
        }
    }
    (out, at == b.len())
}

fn scalar(t: u8, b: &[u8]) -> Option<PVal> {
    Some(match t {
        b'?' | b'c' => PVal::Int(i64::from(*b.first()?)),
        b's' => PVal::Int(i64::from(b.i16_at(0)?)),
        b'w' => PVal::Int(i64::from(b.u16_at(0)?)),
        b'i' => PVal::Int(i64::from(b.i32_at(0)?)),
        b'l' => PVal::Int(i64::from(b.u32_at(0)?)),
        b'r' => PVal::Float(f64::from(b.f32_at(0)?)),
        b'q' | b'd' => PVal::Float(b.f64_at(0)?),
        b't' => PVal::Time(b.u64_at(0)?),
        _ => return None,
    })
}

/// A property set as a JSON object: named keys by name, unnamed keys as `#<number>`.
fn pset_json(items: &[(u16, PVal)], names: &BTreeMap<u16, String>) -> Value {
    let mut m = Map::new();
    for (k, v) in items {
        let name = names
            .get(k)
            .filter(|n| !n.is_empty())
            .cloned()
            .unwrap_or_else(|| format!("#{k}"));
        let j = match v {
            PVal::Int(i) => json!(i),
            PVal::Float(f) => num(*f),
            PVal::Text(s) => json!(s),
            PVal::Time(t) => filetime_iso(*t).map_or_else(|| json!(t), |s| json!(s)),
            PVal::Bytes(n) => json!(format!("<{n} bytes>")),
            PVal::Array(a) => Value::Array(a.iter().map(|x| num(*x)).collect()),
            PVal::Set(s) => pset_json(s, names),
        };
        // keep the first of repeated names
        m.entry(name).or_insert(j);
    }
    Value::Object(m)
}

/// First value under `key` anywhere in `v` (depth first).
fn find<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(m) => {
            if let Some(x) = m.get(key) {
                return Some(x);
            }
            m.values().find_map(|c| find(c, key))
        }
        _ => None,
    }
}

fn find_text(v: &Value, key: &str) -> Option<String> {
    find(v, key).and_then(|x| match x {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

fn find_num(v: &Value, key: &str) -> Option<f64> {
    find(v, key).and_then(|x| match x {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s
            .trim()
            .trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.')
            .parse()
            .ok(),
        _ => None,
    })
}

/// A leading number of a text value such as `20µm` or `26°C`.
fn lead_number(s: &str) -> Option<f64> {
    let t: String = s
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    t.parse().ok()
}

/// Width, height and components of a JPEG from its SOF marker.
pub(crate) fn jpeg_size(b: &[u8]) -> Option<(u32, u32, u32)> {
    if b.get(..2)? != [0xff, 0xd8] {
        return None;
    }
    let mut at = 2usize;
    while at + 4 <= b.len() {
        if b[at] != 0xff {
            return None;
        }
        let marker = b[at + 1];
        if marker == 0xd8 || marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            at += 2;
            continue;
        }
        let len = usize::from(u16::from_be_bytes([b[at + 2], b[at + 3]]));
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            let h = u16::from_be_bytes([*b.get(at + 5)?, *b.get(at + 6)?]);
            let w = u16::from_be_bytes([*b.get(at + 7)?, *b.get(at + 8)?]);
            let n = *b.get(at + 9)?;
            return Some((u32::from(w), u32::from(h), u32::from(n)));
        }
        at += 2 + len;
    }
    None
}

/// Rational EXIF tags of the white-light image's first IFD and its EXIF IFD:
/// tag → values (numerator / denominator).
fn exif_rationals(b: &[u8]) -> BTreeMap<u16, Vec<f64>> {
    let mut out = BTreeMap::new();
    // APP1 "Exif\0\0"
    let Some(p) = b.windows(6).position(|w| w == b"Exif\0\0") else {
        return out;
    };
    let tiff = &b[p + 6..];
    let le = match tiff.get(..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return out,
    };
    let u16v = |at: usize| -> Option<u16> {
        let s = tiff.get(at..at + 2)?;
        Some(if le {
            u16::from_le_bytes([s[0], s[1]])
        } else {
            u16::from_be_bytes([s[0], s[1]])
        })
    };
    let u32v = |at: usize| -> Option<u32> {
        let s = tiff.get(at..at + 4)?;
        Some(if le {
            u32::from_le_bytes([s[0], s[1], s[2], s[3]])
        } else {
            u32::from_be_bytes([s[0], s[1], s[2], s[3]])
        })
    };
    let mut ifds = vec![u32v(4).unwrap_or(0) as usize];
    let mut seen = 0;
    while let Some(ifd) = ifds.pop() {
        seen += 1;
        if seen > 8 || ifd == 0 {
            continue;
        }
        let Some(n) = u16v(ifd) else { continue };
        for i in 0..usize::from(n).min(512) {
            let e = ifd + 2 + i * 12;
            let (Some(tag), Some(ty), Some(count), Some(val)) =
                (u16v(e), u16v(e + 2), u32v(e + 4), u32v(e + 8))
            else {
                break;
            };
            if tag == 0x8769 {
                ifds.push(val as usize);
            }
            if ty == 5 || ty == 10 {
                let mut vals = Vec::new();
                for k in 0..count.min(16) as usize {
                    let at = val as usize + k * 8;
                    if let (Some(a), Some(d)) = (u32v(at), u32v(at + 4)) {
                        let (a, d) = if ty == 10 {
                            (f64::from(a as i32), f64::from(d as i32))
                        } else {
                            (f64::from(a), f64::from(d))
                        };
                        vals.push(if d == 0.0 { f64::NAN } else { a / d });
                    }
                }
                out.insert(tag, vals);
            }
        }
    }
    out
}

/// The fields of the header block (`WDF1`).
struct Header {
    points: u64,
    capacity: u64,
    count: u64,
    accumulations: u32,
    y_list_len: u32,
    x_list_len: u64,
    origin_lists: u32,
    app: String,
    version: Vec<u16>,
    app_version: String,
    scan_type: u32,
    measurement_type: u32,
    start_ticks: u64,
    end_ticks: u64,
    spectral_unit: u32,
    laser_wn: f64,
    user: String,
    title: String,
}

impl Header {
    fn of(hdr: &[u8]) -> Self {
        let version: Vec<u16> = (0..4)
            .map(|i| hdr.u16_at(0x78 + 2 * i).unwrap_or(0))
            .collect();
        let app_version = version
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(".");
        Header {
            points: u64::from(hdr.u32_at(0x3c).unwrap_or(0)),
            capacity: hdr.u64_at(0x40).unwrap_or(0),
            count: hdr.u64_at(0x48).unwrap_or(0),
            accumulations: hdr.u32_at(0x50).unwrap_or(0),
            y_list_len: hdr.u32_at(0x54).unwrap_or(0),
            x_list_len: u64::from(hdr.u32_at(0x58).unwrap_or(0)),
            origin_lists: hdr.u32_at(0x5c).unwrap_or(0),
            app: text_field(hdr.bytes_at(0x60, 24).unwrap_or_default()),
            version,
            app_version,
            scan_type: hdr.u32_at(0x80).unwrap_or(0),
            measurement_type: hdr.u32_at(0x84).unwrap_or(0),
            start_ticks: hdr.u64_at(0x88).unwrap_or(0),
            end_ticks: hdr.u64_at(0x90).unwrap_or(0),
            spectral_unit: hdr.u32_at(0x98).unwrap_or(0),
            laser_wn: f64::from(hdr.f32_at(0x9c).unwrap_or(0.0)),
            user: text_field(hdr.bytes_at(0xd0, 0x20).unwrap_or_default()),
            title: text_field(hdr.bytes_at(0xf0, 0x110).unwrap_or_default()),
        }
    }

    /// The application name, `WiRE` when the header leaves it empty.
    fn application(&self) -> &str {
        if self.app.is_empty() {
            "WiRE"
        } else {
            &self.app
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "points_per_spectrum": self.points, "capacity": self.capacity, "count": self.count,
            "accumulations": self.accumulations, "y_list_length": self.y_list_len, "x_list_length": self.x_list_len,
            "origin_lists": self.origin_lists, "application": self.app, "application_version": self.app_version,
            "scan_type_code": self.scan_type, "measurement_type_code": self.measurement_type,
            "start": filetime_iso(self.start_ticks), "end": filetime_iso(self.end_ticks),
            "spectral_unit_code": self.spectral_unit, "laser_wavenumber_cm1": num(self.laser_wn),
            "user": self.user, "title": self.title,
        })
    }
}

/// The x list block (`XLST`): type and unit codes and the values.
#[derive(Default)]
struct XList {
    type_code: u32,
    unit_code: u32,
    values: Vec<f64>,
}

/// The origin lists (`ORGN`): per-spectrum coordinates, times and flags.
#[derive(Default)]
struct Origins {
    columns: Vec<(String, Option<String>, Vec<f64>)>,
    meta: Vec<Value>,
    /// Stage x and y of each spectrum, µm.
    x_um: Option<Vec<f64>>,
    y_um: Option<Vec<f64>>,
}

/// A WiRE map analysis (`MAP ` block): its label, where its values are and how many.
struct MapResult {
    label: String,
    offset: u64,
    count: u64,
}

/// Parse a `.wdf` file.
pub(crate) fn parse(f: &SourceFile, path: &Path, file_len: u64) -> Result<Parsed> {
    let hdr = read_at(f, path, 0, 512, file_len)?;
    if hdr.len() < 512 || hdr[..4] != WDF_MAGIC[..] {
        return Err(Error::corrupt(
            FMT,
            "not a WiRE file: it does not start with a 512-byte WDF1 block",
        ));
    }
    let (blocks, mut findings) = block_chain(f, path, file_len)?;
    let block = |n: &str| blocks.iter().find(|b| b.name == n);
    let hd = Header::of(&hdr);
    let (points, capacity, count) = (hd.points, hd.capacity, hd.count);
    if count > capacity {
        findings.push(Finding::error(
            "bad_header",
            format!("the header counts {count} spectra but has room for {capacity}"),
        ));
    }
    if count < capacity {
        findings.push(Finding::warning(
            "incomplete",
            format!("the measurement stopped after {count} of {capacity} spectra"),
        ));
    }
    let xl = match block("XLST") {
        Some(b) => x_list(f, path, file_len, b, hd.x_list_len, &mut findings)?,
        None => XList::default(),
    };
    let mut parsed = Parsed {
        findings,
        ..Parsed::default()
    };
    let data = block("DATA");
    let count32 = u32::try_from(count.min(capacity)).unwrap_or(u32::MAX);
    if let Some(d) = data {
        let need = 16u64.saturating_add(capacity.saturating_mul(points).saturating_mul(4));
        if d.size < need {
            parsed.findings.push(Finding::warning(
                "short_data",
                format!(
                    "DATA holds {} bytes; {capacity} spectra of {points} points need {need}",
                    d.size
                ),
            ));
        }
    } else {
        parsed
            .findings
            .push(Finding::error("missing_data", "the file has no DATA block"));
    }
    if xl.values.len() as u64 != points && !xl.values.is_empty() {
        parsed.findings.push(Finding::warning(
            "x_list_size",
            format!(
                "the x list has {} values for {points} points per spectrum",
                xl.values.len()
            ),
        ));
    }
    let mut origins = match block("ORGN") {
        Some(b) => origin_lists(f, path, file_len, b, &hd, count32, &mut parsed)?,
        None => Origins::default(),
    };
    let (psets, map_results) = property_sets(f, path, file_len, &blocks, count32, &mut parsed)?;
    let pset = |k: &str| psets.get(k).cloned().unwrap_or(Value::Null);
    let (wxis, wxdm, wxcs, wxda) = (pset("WXIS"), pset("WXDM"), pset("WXCS"), pset("WXDA"));

    if let Some(d) = data
        && points > 0
        && count32 > 0
    {
        let extra = set_extra(&hd, &xl, &wxis, &wxdm, &wxcs, &wxda);
        let (xq, xu) = match (xl.type_code, xl.unit_code) {
            (_, 1) => ("raman_shift", Some("1/cm")),
            _ => ("x", None),
        };
        let yu = match hd.spectral_unit {
            6 => Some("counts"),
            _ => None,
        };
        let x = if xl.values.len() as u64 == points {
            XValues::Listed(xl.values.clone())
        } else {
            XValues::Regular {
                first: 0.0,
                last: points.saturating_sub(1) as f64,
            }
        };
        let listed = matches!(x, XValues::Listed(_));
        parsed.sets.push(SpectrumSet {
            name: match hd.measurement_type {
                3 => format!("Raman map ({count32} spectra)"),
                2 => format!("Raman series ({count32} spectra)"),
                _ if count32 > 1 => format!("Raman spectra ({count32})"),
                _ if !hd.title.is_empty() => hd.title.clone(),
                _ => "Raman spectrum".to_string(),
            },
            x_quantity: if listed { xq } else { "points" },
            x_unit: if listed { xu.map(str::to_string) } else { None },
            x,
            y_name: "intensity".into(),
            y_unit: yu.map(str::to_string),
            points,
            count: count32,
            rows: Rows::Strided {
                first: d.offset + 16,
                stride: points * 4,
            },
            stored: Stored::F32,
            scale: 1.0,
            data_type: if xq == "raman_shift" || hd.laser_wn > 0.0 {
                "RAMAN SPECTRUM"
            } else {
                "UNKNOWN"
            },
            extra,
        });
    }
    let mut columns = std::mem::take(&mut origins.columns);
    for r in &map_results {
        let b = read_at(f, path, r.offset, r.count * 4, file_len)?;
        columns.push((
            format!("wire_map: {}", r.label),
            None,
            b.as_chunks::<4>()
                .0
                .iter()
                .map(|c| f64::from(f32::from_le_bytes(*c)))
                .collect(),
        ));
    }
    // Only lists with one value per spectrum make the table, so its index column is bounded by
    // the bytes of those lists in the file, never by the header's count alone.
    if !parsed.sets.is_empty() && columns.iter().any(|c| c.2.len() == count32 as usize) {
        let mut cols = vec![(
            "spectrum".to_string(),
            None,
            (0..count32).map(f64::from).collect(),
        )];
        cols.extend(
            columns
                .into_iter()
                .filter(|c| c.2.len() == count32 as usize),
        );
        parsed.tables.push(SpectrumTable {
            name: "spectra".into(),
            trace: 0,
            columns: cols,
        });
    }
    let wmap = match block("WMAP") {
        Some(b) if !parsed.sets.is_empty() => {
            map_geometry(f, path, file_len, b, &origins, count32, &mut parsed)?
        }
        _ => Value::Null,
    };
    let white = match block("WHTL") {
        Some(b) => white_light(f, path, file_len, b, &mut parsed)?,
        None => Value::Null,
    };
    let comment = match block("TEXT") {
        Some(t) => Some(read_at(
            f,
            path,
            t.offset + 16,
            t.size.saturating_sub(16).min(1 << 20),
            file_len,
        )?),
        None => None,
    };
    parsed.facts = wdf_facts(&hd, &wxis, &wxdm, comment.as_deref(), parsed.sets.first());
    parsed.format_version = (!hd.app_version.is_empty() && hd.app_version != "0.0.0.0")
        .then(|| format!("{} {}", hd.application(), hd.app_version));
    parsed.vendor = json!({
        "header": hd.to_json(),
        "x_list": {"type_code": xl.type_code, "unit_code": xl.unit_code},
        "origin_lists": origins.meta,
        "map": wmap,
        "white_light": white,
        "property_sets": Value::Object(psets),
        "blocks": blocks.iter().map(|b| json!({"name": b.name, "uid": b.uid, "offset": b.offset, "size": b.size})).collect::<Vec<_>>(),
    });
    for b in &blocks {
        parsed.entries.push(LsEntry {
            kind: "block".into(),
            name: b.name.trim().to_string(),
            offset: Some(b.offset),
            size: Some(b.size),
            image: None,
            details: json!({"uid": b.uid}),
        });
    }
    for (k, v) in [
        ("traces[].sample_count", Source::PriorArt),
        ("traces[].sweep_count", Source::PriorArt),
        ("traces[].extra.axis", Source::PriorArt),
        ("traces[].extra.laser_wavelength_nm", Source::PriorArt),
        ("traces[].extra.accumulations", Source::PriorArt),
        ("traces[].extra.measurement_type", Source::PriorArt),
        ("traces[].extra.acquired_at", Source::Inferred),
        ("traces[].extra.exposure_time_s", Source::Inferred),
        ("traces[].extra.grating", Source::Inferred),
        ("traces[].extra.objective", Source::Inferred),
        ("traces[].extra.laser_power_percent", Source::Inferred),
        ("tables[].columns", Source::PriorArt),
        ("images[].size_x", Source::PriorArt),
        ("images[].size_y", Source::PriorArt),
        ("images[].physical_size", Source::PriorArt),
    ] {
        parsed.provenance.insert(k.into(), v);
    }
    Ok(parsed)
}

/// The chain of blocks from the start of the file, up to a block that is not one (recorded
/// as a finding); a block that runs past the end of the file is kept and recorded.
fn block_chain(f: &SourceFile, path: &Path, file_len: u64) -> Result<(Vec<Block>, Vec<Finding>)> {
    let mut blocks = Vec::new();
    let mut findings = Vec::new();
    let mut pos = 0u64;
    while pos.saturating_add(16) <= file_len && blocks.len() < MAX_BLOCKS {
        let h = read_at(f, path, pos, 16, file_len)?;
        let name = String::from_utf8_lossy(&h[..4]).to_string();
        let uid = h.i32_at(4).unwrap_or(0);
        let size = h.i64_at(8).unwrap_or(0);
        if size < 16 || !h[..4].iter().all(|c| c.is_ascii_graphic() || *c == b' ') {
            findings.push(
                Finding::error(
                    "bad_block",
                    format!(
                        "block at {pos} has name {name:?} and size {size}; the chain stops here"
                    ),
                )
                .at(pos),
            );
            break;
        }
        let size = size as u64;
        if pos + size > file_len {
            findings.push(
                Finding::error(
                    "truncated",
                    format!("block {name} at {pos} of {size} bytes runs past the end of the file ({file_len} bytes)"),
                )
                .at(pos),
            );
        }
        blocks.push(Block {
            name,
            uid,
            offset: pos,
            size,
        });
        pos += size;
    }
    Ok((blocks, findings))
}

/// The x list block: type and unit codes, then `len` float32 values.
fn x_list(
    f: &SourceFile,
    path: &Path,
    file_len: u64,
    b: &Block,
    len: u64,
    findings: &mut Vec<Finding>,
) -> Result<XList> {
    let bytes = read_at(f, path, b.offset + 16, 8 + len * 4, file_len)?;
    let values: Vec<f64> = bytes
        .get(8..)
        .unwrap_or_default()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f64::from(f32::from_le_bytes(*c)))
        .collect();
    if (values.len() as u64) < len {
        findings.push(Finding::error(
            "truncated",
            "the x list is shorter than the header says",
        ));
    }
    Ok(XList {
        type_code: bytes.u32_at(0).unwrap_or(0),
        unit_code: bytes.u32_at(4).unwrap_or(0),
        values,
    })
}

/// The origin lists: a 24-byte header (type, unit, name) and one 8-byte value per spectrum
/// each; stage X/Y/Z, times and other lists become table columns.
fn origin_lists(
    f: &SourceFile,
    path: &Path,
    file_len: u64,
    b: &Block,
    hd: &Header,
    count32: u32,
    parsed: &mut Parsed,
) -> Result<Origins> {
    let mut out = Origins::default();
    let head = read_at(f, path, b.offset + 16, 4, file_len)?;
    let n = head.u32_at(0).unwrap_or(0).min(64);
    let mut at = b.offset + 20;
    let per = hd.capacity.saturating_mul(8).saturating_add(24);
    for _ in 0..n {
        let lh = read_at(f, path, at, 24, file_len)?;
        if lh.len() < 24 {
            break;
        }
        let ty = lh.u32_at(0).unwrap_or(0);
        let unit = lh.u32_at(4).unwrap_or(0);
        let name = text_field(&lh[8..24]);
        let kind = ty & 0x7fff_ffff;
        let primary = ty & 0x8000_0000 != 0;
        let vals_b = read_at(
            f,
            path,
            at.saturating_add(24),
            u64::from(count32) * 8,
            file_len,
        )?;
        let ints = matches!(kind, 11 | 16 | 17);
        let raw: Vec<f64> = vals_b
            .as_chunks::<8>()
            .0
            .iter()
            .map(|&a| {
                if ints {
                    i64::from_le_bytes(a) as f64
                } else {
                    f64::from_le_bytes(a)
                }
            })
            .collect();
        out.meta
            .push(json!({"name": name, "type_code": kind, "unit_code": unit, "primary": primary}));
        match (kind, unit) {
            (11, 24) => {
                // FILETIME ticks → seconds from the first spectrum
                let base = raw.first().copied().unwrap_or(0.0);
                out.columns.push((
                    "time_s".into(),
                    Some("s".into()),
                    raw.iter().map(|v| (v - base) / 1e7).collect(),
                ));
            }
            (3..=5, _) => {
                // stage X/Y/Z by data type, whatever the list is named (`Z data` in
                // LiveTrack maps), in µm when the unit is a length
                let axis = ["x", "y", "z"][(kind - 3) as usize];
                let to_um = match unit {
                    5 => Some(1.0),
                    8 => Some(1e3),
                    9 => Some(1e6),
                    3 => Some(1e-3),
                    _ => None,
                };
                let (col, u, vals) = match to_um {
                    Some(f) => (
                        format!("{axis}_um"),
                        Some("µm".to_string()),
                        raw.iter().map(|v| v * f).collect::<Vec<f64>>(),
                    ),
                    None => (axis.to_string(), None, raw),
                };
                if kind == 3 && to_um.is_some() {
                    out.x_um = Some(vals.clone());
                }
                if kind == 4 && to_um.is_some() {
                    out.y_um = Some(vals.clone());
                }
                out.columns.push((col, u, vals));
            }
            (16 | 17, _) => {} // checksums and flags: vendor tree only
            _ => {
                let lname = name.to_ascii_lowercase();
                let base = if lname.trim().is_empty() {
                    format!("origin_{kind}")
                } else {
                    lname.trim().replace(' ', "_")
                };
                // other lists in µm (LiveTrack `Z actual`, `Z difference`)
                if unit == 5 {
                    out.columns
                        .push((format!("{base}_um"), Some("µm".into()), raw));
                } else {
                    out.columns.push((base, None, raw));
                }
            }
        }
        at = at.saturating_add(per);
    }
    if n != hd.origin_lists {
        parsed.findings.push(Finding::info(
            "origin_lists",
            format!("ORGN holds {n} lists, the header says {}", hd.origin_lists),
        ));
    }
    Ok(out)
}

/// The property sets of the settings blocks, as JSON by block (`MAP <uid>` for map
/// analyses), and the map analyses whose values are one float32 per spectrum.
fn property_sets(
    f: &SourceFile,
    path: &Path,
    file_len: u64,
    blocks: &[Block],
    count32: u32,
    parsed: &mut Parsed,
) -> Result<(Map<String, Value>, Vec<MapResult>)> {
    let mut psets = Map::new();
    let mut map_results = Vec::new();
    for b in blocks {
        if !matches!(
            b.name.as_str(),
            "WXDA" | "WXDM" | "WXCS" | "WXIS" | "ZLDC" | "MAP " | "WXDB"
        ) || b.size > MAX_PSET
        {
            continue;
        }
        let bytes = read_at(f, path, b.offset + 16, b.size.saturating_sub(16), file_len)?;
        if !bytes.starts_with(b"PSET") {
            continue;
        }
        let n = bytes.u32_at(4).unwrap_or(0) as usize;
        let body = bytes
            .get(8..8 + n.min(bytes.len().saturating_sub(8)))
            .unwrap_or_default();
        let mut names = BTreeMap::new();
        let (items, ok) = pset_items(body, &mut names, 0);
        if !ok {
            parsed.findings.push(Finding::info(
                "property_set",
                format!(
                    "property set of block {} (uid {}) is decoded only in part",
                    b.name, b.uid
                ),
            ));
        }
        let j = pset_json(&items, &names);
        if b.name == "MAP " {
            // analysis results: after the property set, a 64-bit count and one float32 per spectrum
            let data_at = b.offset + 16 + 8 + n as u64;
            let cnt = read_at(f, path, data_at, 8, file_len)?
                .u64_at(0)
                .unwrap_or(0);
            let label = j
                .get("#411")
                .and_then(Value::as_str)
                .unwrap_or("map")
                .to_string();
            if cnt == u64::from(count32) && data_at + 8 + cnt * 4 <= b.offset + b.size {
                map_results.push(MapResult {
                    label,
                    offset: data_at + 8,
                    count: cnt,
                });
            }
        }
        let key = if b.name == "MAP " {
            format!("MAP {}", b.uid)
        } else {
            b.name.clone()
        };
        psets.insert(key, j);
    }
    Ok((psets, map_results))
}

/// The header fields and instrument settings (from the property sets) kept on the spectrum
/// set.
fn set_extra(
    hd: &Header,
    xl: &XList,
    wxis: &Value,
    wxdm: &Value,
    wxcs: &Value,
    wxda: &Value,
) -> BTreeMap<String, Value> {
    let mut extra = BTreeMap::new();
    if let Some(nm) = nm_of_wavenumber(hd.laser_wn) {
        extra.insert("laser_wavelength_nm".into(), num(nm));
        extra.insert("laser_wavenumber_cm1".into(), num(hd.laser_wn));
    }
    if hd.accumulations > 0 {
        extra.insert("accumulations".into(), json!(hd.accumulations));
    }
    if let Some(ms) = find_num(wxdm, "Exposure Time") {
        extra.insert("exposure_time_s".into(), num(ms / 1000.0));
    }
    let config = wxis.get("System Configuration");
    if let Some(g) = config
        .and_then(|c| c.get("Grating"))
        .and_then(Value::as_str)
    {
        extra.insert("grating".into(), json!(g));
        // the grating's calibration for the laser in use
        if let Some(gd) = wxcs
            .get("Gratings")
            .and_then(|gs| gs.get(g))
            .and_then(|v| find_num(v, "Groove Density (lines/mm)"))
        {
            extra.insert("grating_grooves_per_mm".into(), num(gd));
        }
    }
    if let Some(l) = config.and_then(|c| c.get("#1019")).and_then(Value::as_str) {
        extra.insert("laser_name".into(), json!(l));
    }
    let scope = wxis.get("Microscope");
    if let Some(o) = scope.and_then(|c| c.get("#1023")).and_then(Value::as_str) {
        extra.insert("objective".into(), json!(o));
    }
    if let Some(m) = scope.and_then(|c| c.get("#1022")).and_then(Value::as_f64) {
        extra.insert("objective_magnification".into(), num(m));
    }
    if let Some(p) = find_text(wxis, "ND Transmission %").and_then(|s| lead_number(&s)) {
        extra.insert("laser_power_percent".into(), num(p));
    }
    if let Some(s) = find(wxis, "Slits")
        .and_then(|s| s.get("Opening"))
        .and_then(Value::as_str)
        .and_then(lead_number)
    {
        extra.insert("slit_opening_um".into(), num(s));
    }
    if let Some(m) = config
        .and_then(|c| c.get("FocusMode"))
        .and_then(Value::as_str)
    {
        extra.insert("focus_mode".into(), json!(m));
    }
    let ccd = wxcs.get("CCD");
    if let Some(c) = ccd.and_then(|c| c.get("CCD")).and_then(Value::as_str) {
        extra.insert("detector".into(), json!(c));
    }
    if let Some(t) = ccd
        .and_then(|c| c.get("Temperature"))
        .and_then(Value::as_f64)
    {
        extra.insert("detector_temperature_c".into(), num(t));
    }
    if let Some(s) = find_text(wxda, "CCD serial number") {
        extra.insert("detector_serial".into(), json!(s));
    }
    extra.insert(
        "measurement_type".into(),
        json!(match hd.measurement_type {
            0 => "unspecified",
            1 => "single",
            2 => "series",
            3 => "map",
            _ => "other",
        }),
    );
    extra.insert("measurement_type_code".into(), json!(hd.measurement_type));
    extra.insert("scan_type_code".into(), json!(hd.scan_type));
    if let Some(t) = match hd.scan_type {
        1 => Some("static"),
        6 => Some("StreamLine"),
        7 => Some("StreamLineHR"),
        _ => None,
    } {
        extra.insert("scan_type".into(), json!(t));
    }
    if let Some(t) = filetime_iso(hd.start_ticks) {
        extra.insert("acquired_at".into(), json!(t));
    }
    if let Some(t) = filetime_iso(hd.end_ticks) {
        extra.insert("ended_at".into(), json!(t));
    }
    if !hd.title.is_empty() {
        extra.insert("title".into(), json!(hd.title));
    }
    if !hd.user.is_empty() {
        extra.insert("operator".into(), json!(hd.user));
    }
    extra.insert("x_list_type_code".into(), json!(xl.type_code));
    extra.insert("x_list_unit_code".into(), json!(xl.unit_code));
    extra.insert("spectral_unit_code".into(), json!(hd.spectral_unit));
    if hd.count < hd.capacity {
        extra.insert("capacity".into(), json!(hd.capacity));
    }
    extra
}

/// The map geometry block (`WMAP`): a map of the spectra, placed by their stage coordinates
/// on the map grid, or in storage order when those do not fit it. Returns the block as JSON.
fn map_geometry(
    f: &SourceFile,
    path: &Path,
    file_len: u64,
    b: &Block,
    origins: &Origins,
    count32: u32,
    parsed: &mut Parsed,
) -> Result<Value> {
    let m = read_at(f, path, b.offset + 16, 48, file_len)?;
    let flags = m.u32_at(0).unwrap_or(0);
    let origin =
        [m.f32_at(8), m.f32_at(12), m.f32_at(16)].map(|v| f64::from(v.unwrap_or(f32::NAN)));
    let step = [m.f32_at(20), m.f32_at(24), m.f32_at(28)].map(|v| f64::from(v.unwrap_or(f32::NAN)));
    let size = [m.u32_at(32), m.u32_at(36), m.u32_at(40)].map(|v| v.unwrap_or(0));
    let wmap =
        json!({"flags": flags, "origin": origin.map(num), "step": step.map(num), "size": size});
    let (w, h) = (size[0], size[1]);
    let pixels = u64::from(w).saturating_mul(u64::from(h));
    if w <= 1 || h <= 1 || pixels > 1 << 28 {
        return Ok(wmap);
    }
    let mut grid = vec![None; pixels as usize];
    let mut placed = 0u64;
    let mut clash = false;
    if let (Some(xs), Some(ys)) = (&origins.x_um, &origins.y_um)
        && step[0].abs() > 0.0
        && step[1].abs() > 0.0
    {
        for k in 0..count32 as usize {
            let (Some(x), Some(y)) = (xs.get(k), ys.get(k)) else {
                break;
            };
            let c = ((x - origin[0]) / step[0]).round();
            let r = ((y - origin[1]) / step[1]).round();
            if c < 0.0 || r < 0.0 || c >= f64::from(w) || r >= f64::from(h) {
                clash = true;
                break;
            }
            let p = r as usize * w as usize + c as usize;
            if grid[p].is_some() {
                clash = true;
                break;
            }
            grid[p] = Some(k as u32);
            placed += 1;
        }
    } else {
        clash = true;
    }
    if clash {
        // fall back to the storage order: rows of `w` spectra (x fastest)
        grid = vec![None; pixels as usize];
        placed = 0;
        for k in 0..u64::from(count32).min(pixels) {
            grid[k as usize] = Some(k as u32);
            placed += 1;
        }
        parsed.notes.push(
            "map pixels placed in storage order (the stage coordinates do not fall on the map grid)".into(),
        );
    }
    let mut mx = BTreeMap::new();
    mx.insert(
        "map_origin_um".into(),
        json!([num(origin[0]), num(origin[1])]),
    );
    mx.insert("map_step_um".into(), json!([num(step[0]), num(step[1])]));
    mx.insert("spectra_placed".into(), json!(placed));
    mx.insert("wmap_flags".into(), json!(flags));
    parsed.maps.push(MapSpec {
        name: format!("Raman map {w} × {h}"),
        set: 0,
        width: w,
        height: h,
        pixels: grid,
        pixel_um: (Some(step[0].abs()), Some(step[1].abs())),
        extra: mx,
    });
    Ok(wmap)
}

/// The white-light image block (`WHTL`): a JPEG attachment, decoded as a photo when it is
/// grey or RGB; its EXIF tags give the field of view and origin. Returns those as JSON.
fn white_light(
    f: &SourceFile,
    path: &Path,
    file_len: u64,
    b: &Block,
    parsed: &mut Parsed,
) -> Result<Value> {
    let head = read_at(
        f,
        path,
        b.offset + 16,
        b.size.saturating_sub(16).min(1 << 16),
        file_len,
    )?;
    let mut ex = BTreeMap::new();
    let r = exif_rationals(&head);
    // 0xA20E/0xA20F: focal-plane resolution = field of view (µm); 0xFEA0: origin; 0xFEA1: field of view
    if let (Some(w), Some(h)) = (
        r.get(&0xa20e).and_then(|v| v.first()),
        r.get(&0xa20f).and_then(|v| v.first()),
    ) {
        ex.insert("field_of_view_um".into(), json!([num(*w), num(*h)]));
    }
    if let Some(o) = r.get(&0xfea0).filter(|v| v.len() >= 2) {
        ex.insert("origin_um".into(), json!([num(o[0]), num(o[1])]));
    }
    let dims = jpeg_size(&head);
    if let Some((w, h, _)) = dims {
        ex.insert("width".into(), json!(w));
        ex.insert("height".into(), json!(h));
    }
    let white = json!(ex);
    if let Some((w, h, n)) = dims.filter(|d| d.0 > 0 && d.1 > 0 && matches!(d.2, 1 | 3)) {
        let fov = ex
            .get("field_of_view_um")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let px = |i: usize, n: u32| fov.get(i).and_then(Value::as_f64).map(|v| v / f64::from(n));
        parsed.photos.push(crate::common::PhotoSpec {
            name: "white-light image".into(),
            attachment: parsed.attachments.len(),
            width: w,
            height: h,
            components: n,
            pixel_um: (px(0, w), px(1, h)),
            extra: ex.clone(),
        });
    }
    parsed.attachments.push(Attachment {
        name: "white-light image".into(),
        content_type: "JPEG".into(),
        extension: "jpg".into(),
        offset: b.offset + 16,
        size: b.size.saturating_sub(16),
        extra: ex,
    });
    Ok(white)
}

/// Experiment facts: the header, the instrument serial and measurement name from the property
/// sets, the `TEXT` block as comment, and the spectrum set's settings as parameters.
fn wdf_facts(
    hd: &Header,
    wxis: &Value,
    wxdm: &Value,
    text: Option<&[u8]>,
    set: Option<&SpectrumSet>,
) -> Facts {
    let mut facts = Facts::default();
    Facts::text(&mut facts.vendor, "Renishaw", "format");
    Facts::text(
        &mut facts.software,
        hd.application(),
        "header application name",
    );
    if hd.version.iter().any(|v| *v != 0) {
        Facts::text(
            &mut facts.software_version,
            &hd.app_version,
            "header application version",
        );
    }
    Facts::text(&mut facts.operator, &hd.user, "header user name");
    Facts::text(&mut facts.sample_name, &hd.title, "header title");
    if let Some(t) = filetime_iso(hd.start_ticks) {
        Facts::text(
            &mut facts.started_at,
            &t,
            "header start time (FILETIME at 0x88)",
        );
    }
    if let Some(s) = wxis
        .get("Serial Numbers")
        .and_then(|c| c.get("#1021"))
        .and_then(Value::as_str)
    {
        Facts::text(&mut facts.serial, s, "WXIS Serial Numbers");
    }
    if let Some(b) = text {
        Facts::text(&mut facts.comment, &text_field(b), "TEXT block");
    }
    if let Some(m) = find_text(wxdm, "#1001") {
        Facts::text(&mut facts.method_name, &m, "WXDM measurement name");
    }
    if let Some(s) = set {
        set_parameters(&s.extra, &mut facts);
    }
    facts
}

/// The spectrum set's numeric and text settings as method parameters.
fn set_parameters(e: &BTreeMap<String, Value>, facts: &mut Facts) {
    let inf = Source::Inferred;
    let pa = Source::PriorArt;
    for (k, unit, from, src) in [
        (
            "laser_wavelength_nm",
            Some("nm"),
            "header laser wavenumber (0x9C)",
            pa,
        ),
        ("exposure_time_s", Some("s"), "WXDM Exposure Time", inf),
        ("accumulations", None, "header accumulations (0x50)", pa),
        (
            "laser_power_percent",
            Some("%"),
            "WXIS ND Transmission %",
            inf,
        ),
        (
            "grating_grooves_per_mm",
            None,
            "WXCS Groove Density (lines/mm)",
            inf,
        ),
        ("objective_magnification", None, "WXIS Microscope", inf),
        ("slit_opening_um", Some("µm"), "WXIS Slits Opening", inf),
        (
            "detector_temperature_c",
            Some("°C"),
            "WXCS CCD Temperature",
            inf,
        ),
    ] {
        if let Some(v) = e.get(k).and_then(Value::as_f64) {
            let name = k
                .trim_end_matches("_nm")
                .trim_end_matches("_s")
                .trim_end_matches("_percent")
                .trim_end_matches("_um")
                .trim_end_matches("_c");
            facts.number(name, v, unit, from, src);
        }
    }
    for (k, from) in [
        ("grating", "WXIS System Configuration Grating"),
        ("objective", "WXIS Microscope"),
        ("laser_name", "WXIS System Configuration"),
        ("detector", "WXCS CCD"),
        ("focus_mode", "WXIS System Configuration FocusMode"),
        ("scan_type", "header scan type (0x80)"),
        ("measurement_type", "header measurement type (0x84)"),
    ] {
        if let Some(v) = e.get(k).and_then(Value::as_str) {
            facts.word(k, v, from, inf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filetime() {
        // 2020-01-01T00:00:00Z
        assert_eq!(
            filetime_iso(132_223_104_000_000_000).as_deref(),
            Some("2020-01-01T00:00:00.000Z")
        );
        assert_eq!(filetime_iso(0), None);
    }

    #[test]
    fn psets() {
        let mut b = Vec::new();
        // i key 0x8001 = 42
        b.extend_from_slice(&[b'i', 0, 0x01, 0x80]);
        b.extend_from_slice(&42i32.to_le_bytes());
        // u key 0x0005 = "abc"
        b.extend_from_slice(&[b'u', 0, 0x05, 0x00]);
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(b"abc");
        // r array key 0x8002 = [1.5, 2.5]
        b.extend_from_slice(&[b'r', 0x80, 0x02, 0x80]);
        b.extend_from_slice(&2u32.to_le_bytes());
        b.extend_from_slice(&1.5f32.to_le_bytes());
        b.extend_from_slice(&2.5f32.to_le_bytes());
        // k names
        b.extend_from_slice(&[b'k', 0, 0x01, 0x80]);
        b.extend_from_slice(&6u32.to_le_bytes());
        b.extend_from_slice(b"Answer");
        let mut names = BTreeMap::new();
        let (items, ok) = pset_items(&b, &mut names, 0);
        assert!(ok);
        let j = pset_json(&items, &names);
        assert_eq!(j["Answer"], 42);
        assert_eq!(j["#5"], "abc");
        assert_eq!(j["#32770"], json!([1.5, 2.5]));
        // truncated: stops cleanly
        let (_, ok) = pset_items(&b[..10], &mut BTreeMap::new(), 0);
        assert!(!ok);
    }

    #[test]
    fn jpeg_dims() {
        let mut j = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0, 0];
        j.extend_from_slice(&[0xff, 0xc0, 0x00, 0x0b, 8, 0x01, 0xe0, 0x02, 0x80, 3]);
        assert_eq!(jpeg_size(&j), Some((640, 480, 3)));
        assert_eq!(jpeg_size(b"nope"), None);
    }
}
