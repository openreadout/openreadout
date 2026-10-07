//! JASCO Spectra Manager `.jws` files (and `.jrs` from the instruments' firmware): FT-IR, Raman,
//! UV-Vis, circular dichroism and fluorescence spectra. Two containers share the extension: a
//! compound file (`Header`, `DataInfo`, `Y-Data`, `X-Data`, `SampleInfo`, `ModuleInfo`,
//! `MeasParam` streams) and a flat file (`L~S ` header, float32 data at the end). Layout:
//! `docs/formats/jasco-jws.md`.

use std::path::Path;

use openreadout_core::cfb::{CFB_MAGIC, Cfb};
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::Source;
use openreadout_core::source::{Input, SourceFile};
use openreadout_core::time::unix_to_iso8601;
use openreadout_core::{Error, Result};
use serde_json::{Map, Value, json};

use crate::common::{Facts, Le, Parsed, Rows, SpectrumSet, Stored, XValues, num, read_at};

const FMT: &str = crate::JWS_FORMAT_ID;
/// First bytes of the flat container.
pub(crate) const FLAT_MAGIC: &[u8; 4] = b"L~S ";
/// Header size of every flat file (the data follow it to the end of the file).
const FLAT_HEADER: u64 = 0x740;
/// Largest flat file read whole (the largest seen is 52 kB).
const MAX_FLAT: u64 = 256 << 20;
/// Largest stream read (the largest `Y-Data` seen is 242 kB).
const MAX_STREAM: u64 = 256 << 20;
/// Bytes of `DataInfo` for one channel; each further channel adds 44.
const DATAINFO_ONE: usize = 96;
/// OLE date JASCO writes for "no date" (1999-11-30).
const NO_DATE: f64 = 36_494.0;

/// What a channel code (compound `DataInfo` descriptor or flat y-mode byte) holds.
struct ChannelKind {
    /// Trace name and y quantity.
    name: &'static str,
    unit: Option<String>,
    /// The kind when the x axis is a wavelength.
    nm_type: &'static str,
}

/// A channel code in our vocabulary; `None` for codes not seen in any validated file.
fn channel_kind(code: u32, cd_scale: Option<&str>) -> Option<ChannelKind> {
    let k = |name, unit: Option<&str>, nm_type| ChannelKind {
        name,
        unit: unit.map(str::to_string),
        nm_type,
    };
    Some(match code {
        0x00 => k("transmittance", Some("%"), "UV/VIS SPECTRUM"),
        0x02 => k("reflectance", Some("%"), "UV/VIS SPECTRUM"),
        0x03 => k("absorbance", Some("AU"), "UV/VIS SPECTRUM"),
        0x08 => k("single_beam", None, "UV/VIS SPECTRUM"),
        0x09 => k("single_beam_reference", None, "UV/VIS SPECTRUM"),
        0x0A => k("single_beam_sample", None, "UV/VIS SPECTRUM"),
        0x0E => k("intensity", None, "FLUORESCENCE SPECTRUM"),
        0x1001 => k(
            "circular_dichroism",
            // the CD sensitivity the file records (`200 mdeg/1.0 dOD`) names the unit
            cd_scale.filter(|s| s.contains("mdeg")).map(|_| "mdeg"),
            "CIRCULAR DICHROISM SPECTRUM",
        ),
        0x2001 => k("ht_voltage", Some("V"), "CIRCULAR DICHROISM SPECTRUM"),
        _ => return None,
    })
}

/// The x axis of a compound-file descriptor: (quantity, unit, data type for IR/Raman).
fn x_axis(desc: u32) -> Option<(&'static str, Option<&'static str>, Option<&'static str>)> {
    match desc {
        0x1000_0100 => Some(("wavenumber", Some("1/cm"), Some("INFRARED SPECTRUM"))),
        0x1000_0101 => Some(("raman_shift", Some("1/cm"), Some("RAMAN SPECTRUM"))),
        0x1000_0103 => Some(("wavelength", Some("nm"), None)),
        // a time course (V-630); the time unit is not in a validated field
        0x2000_0203 => Some(("time", None, None)),
        _ => None,
    }
}

/// An OLE automation date (days since 1899-12-30, UTC in JASCO files) as ISO-8601 UTC.
fn ole_utc(days: f64) -> Option<String> {
    #[allow(clippy::float_cmp)] // an exact sentinel value
    let sentinel = days == NO_DATE;
    if sentinel || !days.is_finite() || !(1.0..2_958_465.0).contains(&days) {
        return None;
    }
    let ms = ((days - 25_569.0) * 86_400_000.0).round() as i64;
    let s = unix_to_iso8601(ms.div_euclid(1000), 0);
    // whole seconds: `…:SS.000Z` -> `…:SSZ`
    Some(s.replace(".000Z", "Z"))
}

/// A UTF-16LE string with a u32 byte length (terminating NUL included) at `at`; the text and the
/// offset after it.
fn utf16(b: &[u8], at: usize) -> Option<(String, usize)> {
    let n = b.u32_at(at)? as usize;
    if n > 1 << 16 {
        return None;
    }
    let start = at.checked_add(4)?;
    let raw = b.bytes_at(start, n)?;
    let units: Vec<u16> = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|&u| u != 0)
        .collect();
    Some((String::from_utf16_lossy(&units), start + n))
}

/// One `tag, type, value` record.
#[derive(Debug, Clone)]
struct Record {
    tag: u32,
    kind: u16,
    value: Value,
}

/// Up to `n` TLV records from `at`; stops at the first value type not understood (the rest is
/// kept undecoded) and returns the records and the bytes left.
fn records(b: &[u8], mut at: usize, n: u32) -> (Vec<Record>, usize) {
    let mut out = Vec::new();
    for _ in 0..n.min(4096) {
        let (Some(tag), Some(kind)) = (b.u32_at(at), b.u16_at(at + 4)) else {
            break;
        };
        let v = at + 6;
        let (value, next) = match kind {
            2 => (b.u16_at(v).map(|x| json!(x)), v + 2),
            3 => (b.u32_at(v).map(|x| json!(x)), v + 4),
            4 => (b.f32_at(v).map(|x| num(f64::from(x))), v + 4),
            5 | 7 => (b.f64_at(v).map(num), v + 8),
            8 => match utf16(b, v) {
                Some((s, end)) => (Some(json!(s)), end),
                None => (None, v),
            },
            _ => (None, v),
        };
        let Some(value) = value else {
            break;
        };
        out.push(Record { tag, kind, value });
        at = next;
    }
    (out, b.len().saturating_sub(at))
}

fn record(recs: &[Record], tag: u32) -> Option<&Record> {
    recs.iter().find(|r| r.tag == tag)
}

fn records_json(recs: &[Record]) -> Value {
    json!(
        recs.iter()
            .map(|r| json!({"tag": r.tag, "type": r.kind, "value": r.value}))
            .collect::<Vec<_>>()
    )
}

/// Detection by content: a compound file whose `Header` stream is JASCO's (`L~` … `SPCMAN`)
/// with `DataInfo` and `Y-Data` streams.
pub(crate) fn is_compound_jws(input: &Input) -> bool {
    let Ok(mut f) = input.open() else {
        return false;
    };
    let Ok(cfb) = Cfb::open(&mut f, input.path(), FMT) else {
        return false;
    };
    if cfb.stream("DataInfo").is_none() || cfb.stream("Y-Data").is_none() {
        return false;
    }
    cfb.stream("Header")
        .and_then(|e| cfb.read(&mut f, input.path(), e, 128).ok())
        .is_some_and(|h| header_is_jasco(&h))
}

fn header_is_jasco(h: &[u8]) -> bool {
    let text: String = h
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .filter(|&u| u != 0)
        .map(|u| char::from_u32(u32::from(u)).unwrap_or('?'))
        .collect();
    text.starts_with("L~") && text.contains("SPCMAN")
}

/// What `parse` found: the parse result, and for compound files the `Y-Data` bytes spectra are
/// read from.
pub(crate) struct Opened {
    pub(crate) parsed: Parsed,
    pub(crate) values: Option<Vec<u8>>,
}

/// Parse either container.
pub(crate) fn parse(f: &SourceFile, path: &Path, file_len: u64) -> Result<Opened> {
    let head = read_at(f, path, 0, 8, file_len)?;
    if head.starts_with(&CFB_MAGIC) {
        parse_compound(f, path)
    } else if head.starts_with(FLAT_MAGIC) {
        Ok(Opened {
            parsed: parse_flat(f, path, file_len)?,
            values: None,
        })
    } else {
        Err(Error::corrupt(
            FMT,
            "not a JASCO .jws file: it starts with neither a compound-file signature nor `L~S `",
        ))
    }
}

/// The `DataInfo` stream: the axis and the channel descriptors.
struct DataInfo {
    version: u32,
    channels: u32,
    /// 1: a regular grid from `first` by `step`; 0: an explicit `X-Data` axis.
    grid: u32,
    points: u32,
    first: f64,
    last: f64,
    step: f64,
    /// x, one per channel, then the slots repeat from the start.
    desc: Vec<u32>,
}

impl DataInfo {
    /// Reads and checks the head and descriptors (version 3, 1 to 3 channels, the validated
    /// descriptor pattern).
    fn of(di: &[u8]) -> Result<Self> {
        let (Some(version), Some(channels), Some(grid), Some(points)) =
            (di.u32_at(0), di.u32_at(12), di.u32_at(16), di.u32_at(20))
        else {
            return Err(Error::corrupt(
                FMT,
                "`DataInfo` shorter than its 24-byte head",
            ));
        };
        let (Some(first), Some(last), Some(step)) = (di.f64_at(24), di.f64_at(32), di.f64_at(40))
        else {
            return Err(Error::corrupt(FMT, "`DataInfo` shorter than its x range"));
        };
        let desc: Vec<u32> = (0..4).filter_map(|i| di.u32_at(48 + 4 * i)).collect();
        if desc.len() < 4 {
            return Err(Error::corrupt(
                FMT,
                "`DataInfo` shorter than its channel descriptors",
            ));
        }
        if version != 3 {
            return Err(Error::unsupported(
                FMT,
                format!("`DataInfo` version {version}"),
                "Only version 3 (every file seen) is read; please share this file so the layout can be checked.",
            ));
        }
        if channels == 0 || channels > 3 {
            return Err(Error::unsupported(
                FMT,
                format!("{channels} channels"),
                "Files with 1 to 3 channels (CD, HT, absorbance) have been validated; others are refused.",
            ));
        }
        let n = channels as usize;
        let expected: Vec<u32> = (0..4)
            .map(|i| {
                if i <= n {
                    desc[i]
                } else {
                    desc[(i - n - 1) % (n + 1)]
                }
            })
            .collect();
        // an axis descriptor (0x10…/0x20…) in a channel slot: fewer channels than declared
        if desc != expected || desc[1..=n].iter().any(|d| d >> 24 != 0) {
            return Err(Error::unsupported(
                FMT,
                format!("channel descriptors {desc:08x?} for {channels} channel(s)"),
                "The descriptor slots do not follow the validated pattern (x, one per channel, repeating); please share this file.",
            ));
        }
        Ok(DataInfo {
            version,
            channels,
            grid,
            points,
            first,
            last,
            step,
            desc,
        })
    }

    /// The channel descriptors.
    fn channel_codes(&self) -> &[u32] {
        &self.desc[1..=self.channels as usize]
    }

    fn to_json(&self) -> Value {
        json!({"version": self.version, "channels": self.channels, "grid_flag": self.grid, "points": self.points,
               "first_x": num(self.first), "last_x": num(self.last), "step": num(self.step),
               "descriptors": self.desc.iter().map(|d| format!("{d:#010x}")).collect::<Vec<_>>()})
    }
}

/// The x axis of the data: a regular grid, or the `X-Data` stream (read by `x_data` only
/// then).
fn x_values(
    info: &DataInfo,
    x_data: impl FnOnce() -> Option<Vec<u8>>,
    parsed: &mut Parsed,
) -> Result<XValues> {
    let points = u64::from(info.points);
    let (first, last, step) = (info.first, info.last, info.step);
    match info.grid {
        1 => {
            if !step.is_finite() || step == 0.0 || !first.is_finite() {
                return Err(Error::corrupt(
                    FMT,
                    format!("regular grid with first {first}, step {step}"),
                ));
            }
            let computed = first + step * (points - 1) as f64;
            if (computed - last).abs() > step.abs() {
                parsed.findings.push(Finding::warning(
                    "x_range",
                    format!("stored last x {last} disagrees with first + step × (points − 1) = {computed}"),
                ));
            }
            Ok(XValues::Regular {
                first,
                last: computed,
            })
        }
        0 => {
            let xb = x_data().ok_or_else(|| {
                Error::corrupt(
                    FMT,
                    "an irregular axis (grid flag 0) without an `X-Data` stream",
                )
            })?;
            if xb.len() as u64 != points * 4 {
                return Err(Error::corrupt(
                    FMT,
                    format!(
                        "`X-Data` holds {} bytes for {} points",
                        xb.len(),
                        info.points
                    ),
                ));
            }
            Ok(XValues::Listed(
                xb.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| f64::from(f32::from_le_bytes(*c)))
                    .collect(),
            ))
        }
        g => Err(Error::unsupported(
            FMT,
            format!("`DataInfo` grid flag {g}"),
            "Only regular grids (1) and explicit `X-Data` axes (0) have been validated.",
        )),
    }
}

/// When the measurement started, and the field it came from.
type Started = Option<(String, &'static str)>;

fn parse_compound(f: &SourceFile, path: &Path) -> Result<Opened> {
    let mut file = f.clone();
    let cfb = Cfb::open(&mut file, path, FMT)?;
    let mut read = |name: &str| -> Option<Vec<u8>> {
        let e = cfb.stream(name)?.clone();
        cfb.read(&mut file, path, &e, MAX_STREAM).ok()
    };
    let header = read("Header").unwrap_or_default();
    if !header_is_jasco(header.get(..128).unwrap_or(&header)) {
        return Err(Error::unsupported(
            FMT,
            "a compound file without JASCO's `Header` stream (`L~` … `SPCMAN`)",
            "Only JASCO Spectra Manager .jws files are read; this compound file is something else.",
        ));
    }
    let di = read("DataInfo")
        .ok_or_else(|| Error::corrupt(FMT, "no `DataInfo` stream (the axis and channels)"))?;
    let ydata = read("Y-Data").ok_or_else(|| Error::corrupt(FMT, "no `Y-Data` stream"))?;
    let info = DataInfo::of(&di)?;
    let mut parsed = Parsed::default();
    let want_len = DATAINFO_ONE + 44 * (info.channels as usize - 1);
    if di.len() != want_len {
        parsed.findings.push(Finding::warning(
            "datainfo_size",
            format!(
                "`DataInfo` is {} bytes; {want_len} expected for {} channel(s)",
                di.len(),
                info.channels
            ),
        ));
    }
    let Some((xq, xu, x_type)) = x_axis(info.desc[0]) else {
        return Err(Error::unsupported(
            FMT,
            format!("x-axis descriptor {:#010x}", info.desc[0]),
            "Only wavenumber, Raman shift, wavelength and time axes have been validated; please share this file.",
        ));
    };
    let points = u64::from(info.points);
    if points < 1 {
        return Err(Error::corrupt(FMT, "`DataInfo` declares no points"));
    }
    let want = points
        .checked_mul(4)
        .and_then(|b| b.checked_mul(u64::from(info.channels)))
        .ok_or_else(|| Error::corrupt(FMT, "point count overflows"))?;
    if ydata.len() as u64 != want {
        return Err(Error::corrupt(
            FMT,
            format!(
                "`Y-Data` holds {} bytes; {} channel(s) × {} float32 points need {want}",
                ydata.len(),
                info.channels,
                info.points
            ),
        ));
    }
    let x = x_values(&info, || read("X-Data"), &mut parsed)?;

    let mut facts = Facts::default();
    let mut vendor = Map::new();
    vendor.insert("container".into(), json!("compound file"));
    let header_text = header_text(&header);
    vendor.insert("header".into(), json!(header_text));
    parsed.format_version = header_text
        .split_whitespace()
        .filter(|w| w.starts_with("SPCMAN") || w.starts_with('R'))
        .take(2)
        .map(str::to_string)
        .reduce(|a, b| format!("{a} {b}"));
    vendor.insert("data_info".into(), info.to_json());
    let module_id =
        read("ModuleInfo").and_then(|m| module_info(&m, &mut facts, &mut vendor, &mut parsed));
    let mut started = read("SampleInfo")
        .and_then(|s| sample_info(&s, &mut facts, &mut vendor))
        .flatten();
    let cd_scale = read("MeasParam").and_then(|mp| {
        measurement_parameters(&mp, module_id, &mut facts, &mut started, &mut vendor)
    });
    if let Some(b) = read("BaseInfo") {
        base_info(&b, &mut started, &mut vendor, &mut parsed);
    }
    if let Some((d, from)) = started {
        facts.started_at = Some((d, from.to_string()));
    }
    facts.vendor = Some(("JASCO".into(), "file format".into()));
    vendor.insert(
        "streams".into(),
        json!(
            cfb.entries
                .iter()
                .filter(|e| e.is_stream)
                .map(|e| json!({"path": e.path, "size": e.size}))
                .collect::<Vec<_>>()
        ),
    );
    for e in cfb.entries.iter().filter(|e| e.is_stream) {
        parsed.entries.push(LsEntry {
            kind: "stream".into(),
            name: e.path.clone(),
            offset: None,
            size: Some(e.size),
            image: None,
            details: Value::Null,
        });
    }
    for p in &cfb.problems {
        parsed
            .findings
            .push(Finding::warning("compound_file", p.clone()));
    }
    let axis = Axis {
        quantity: xq,
        unit: xu,
        data_type: x_type,
        x,
        points,
    };
    let title = facts.sample_name.as_ref().map(|(s, _)| s.as_str());
    push_channels(&info, &axis, cd_scale.as_deref(), title, &mut parsed);
    parsed.facts = facts;
    parsed.vendor = json!({ "jasco": Value::Object(vendor) });
    parsed.notes.push(
        "values as stored (float32); JASCO marks invalid points with −1.18e-38, kept as written"
            .into(),
    );
    parsed.provenance.insert("traces".into(), Source::Inferred);
    Ok(Opened {
        parsed,
        values: Some(ydata),
    })
}

/// The first 128 bytes of `Header` (UTF-16LE) as words.
fn header_text(header: &[u8]) -> String {
    header
        .get(..128)
        .unwrap_or(header)
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .map(|u| match char::from_u32(u32::from(u)) {
            Some(c) if c.is_ascii_graphic() => c,
            _ => ' ',
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `ModuleInfo`: the instrument module's name, id, model and serial. Returns the module id.
fn module_info(
    m: &[u8],
    facts: &mut Facts,
    vendor: &mut Map<String, Value>,
    parsed: &mut Parsed,
) -> Option<u16> {
    if let Some((name, at)) = utf16(m, 4)
        && let (Some(_), Some(id)) = (m.u16_at(at), m.u16_at(at + 2))
        && let Some((model, at)) = utf16(m, at + 4)
    {
        let serial = utf16(m, at).map(|(s, _)| s).unwrap_or_default();
        Facts::text(&mut facts.model, &model, "ModuleInfo model");
        Facts::text(&mut facts.serial, &serial, "ModuleInfo serial");
        vendor.insert(
            "module".into(),
            json!({"version": m.u32_at(0), "name": name, "id": id, "model": model, "serial": serial}),
        );
        Some(id)
    } else {
        parsed.findings.push(Finding::warning(
            "unreadable_module_info",
            "`ModuleInfo` does not parse: instrument model and serial are unknown",
        ));
        None
    }
}

/// `SampleInfo`: the sample name, the comment and the records after them (record 1: the
/// measurement date). `None` when the name and comment do not parse; else the date found.
fn sample_info(s: &[u8], facts: &mut Facts, vendor: &mut Map<String, Value>) -> Option<Started> {
    let (name, at) = utf16(s, 4)?;
    let (comment, at) = utf16(s, at)?;
    Facts::text(&mut facts.sample_name, &name, "SampleInfo sample name");
    Facts::text(&mut facts.comment, &comment, "SampleInfo comment");
    let (recs, rest) = s
        .u32_at(at)
        .map_or((Vec::new(), 0), |k| records(s, at + 4, k));
    let started = record(&recs, 1)
        .filter(|r| r.kind == 5 || r.kind == 7)
        .and_then(|r| r.value.as_f64())
        .and_then(ole_utc)
        .map(|d| (d, "SampleInfo record 1 (OLE date, UTC)"));
    vendor.insert(
        "sample".into(),
        json!({"name": name, "comment": comment, "records": records_json(&recs), "undecoded_bytes": rest}),
    );
    Some(started)
}

/// `MeasParam`: the measurement records (FT-IR parameters for module 9). Returns the CD
/// sensitivity text (tag 20 of CD instruments, `200 mdeg/1.0 dOD`).
fn measurement_parameters(
    mp: &[u8],
    module_id: Option<u16>,
    facts: &mut Facts,
    started: &mut Started,
    vendor: &mut Map<String, Value>,
) -> Option<String> {
    let k = mp.u32_at(8)?;
    let (recs, rest) = records(mp, 12, k);
    let cd_scale = record(&recs, 20)
        .and_then(|r| r.value.as_str())
        .map(str::to_string);
    if module_id == Some(9) {
        ftir_parameters(&recs, facts, started);
    }
    vendor.insert(
        "measurement_parameters".into(),
        json!({"module": module_id, "records": records_json(&recs), "undecoded_bytes": rest}),
    );
    cd_scale
}

/// `BaseInfo`: the original path and the saved and measured dates. The measured date is the
/// start when no other field gave one (Raman files).
fn base_info(
    b: &[u8],
    started: &mut Started,
    vendor: &mut Map<String, Value>,
    parsed: &mut Parsed,
) {
    let Some((original, at)) = utf16(b, 21) else {
        return;
    };
    // the file's two clocks disagree (seen: 4.5 h in a JASCOFiles.jl FT-IR file): which one
    // the software shows as the measurement time is not validated
    if let (Some((d, _)), Some(m)) = (&*started, b.f64_at(at + 8).and_then(ole_utc))
        && let (Some(a), Some(c)) = (
            openreadout_core::time::iso8601_to_unix(d),
            openreadout_core::time::iso8601_to_unix(&m),
        )
        && (a - c).abs() > 3600.0
    {
        parsed.findings.push(Finding::warning(
            "time_fields_disagree",
            format!("the measurement time {d} and the BaseInfo measured time {m} differ by more than an hour; which one Spectra Manager shows is not validated"),
        ));
    }
    if started.is_none()
        && let Some(d) = b.f64_at(at + 8).and_then(ole_utc)
    {
        *started = Some((d, "BaseInfo measured date (OLE date, UTC)"));
    }
    vendor.insert(
        "base_info".into(),
        json!({"original_path": original,
               "saved_at": b.f64_at(at).and_then(ole_utc),
               "measured_at": b.f64_at(at + 8).and_then(ole_utc)}),
    );
}

/// The x axis shared by the channels of a compound file.
struct Axis {
    quantity: &'static str,
    unit: Option<&'static str>,
    /// The data type the axis implies (IR and Raman), else the channel's.
    data_type: Option<&'static str>,
    x: XValues,
    points: u64,
}

/// One spectrum set per channel, channel-major in `Y-Data`.
fn push_channels(
    info: &DataInfo,
    axis: &Axis,
    cd_scale: Option<&str>,
    title: Option<&str>,
    parsed: &mut Parsed,
) {
    let points = axis.points;
    for (k, &code) in info.channel_codes().iter().enumerate() {
        let kind = channel_kind(code, cd_scale);
        if kind.is_none() {
            parsed.findings.push(Finding::warning(
                "unknown_channel_code",
                format!("channel {k} has code {code:#x}, not seen in any validated file: its values are returned unnamed"),
            ));
        }
        let (name, unit, nm_type) = kind.map_or_else(
            || (format!("channel_{code:#x}"), None, "UNKNOWN"),
            |c| (c.name.to_string(), c.unit, c.nm_type),
        );
        let data_type = match (axis.data_type, axis.quantity) {
            (Some(t), _) => t,
            (None, "time") if matches!(code, 0x00 | 0x03) => "UV/VIS KINETICS",
            (None, "time") => "KINETICS",
            (None, _) => nm_type,
        };
        let mut extra = std::collections::BTreeMap::new();
        extra.insert("channel_code".into(), json!(format!("{code:#x}")));
        if let Some(s) = title {
            extra.insert("title".into(), json!(s));
        }
        if axis.quantity == "time" {
            extra.insert(
                "x_note".into(),
                json!("a time course: the time unit is not recorded in a field that has been validated"),
            );
        }
        parsed.sets.push(SpectrumSet {
            name: name.clone(),
            x_quantity: axis.quantity,
            x_unit: axis.unit.map(str::to_string),
            x: axis.x.clone(),
            y_name: name,
            y_unit: unit,
            points,
            count: 1,
            rows: Rows::Listed(vec![k as u64 * points * 4]),
            stored: Stored::F32,
            scale: 1.0,
            data_type,
            extra,
        });
    }
}

/// FT-IR acquisition parameters (`MeasParam` of module 9), named from the Spectra Manager exports'
/// footers.
fn ftir_parameters(recs: &[Record], facts: &mut Facts, started: &mut Started) {
    let inf = Source::Inferred;
    let f = |tag| record(recs, tag).and_then(|r| r.value.as_f64());
    let s = |tag| {
        record(recs, tag)
            .and_then(|r| r.value.as_str())
            .map(str::to_string)
    };
    if let Some(v) = f(1) {
        facts.number("accumulations", v, None, "MeasParam tag 1", inf);
    }
    if let Some(v) = f(2) {
        facts.number("resolution", v, Some("cm⁻¹"), "MeasParam tag 2", inf);
    }
    if let Some(v) = f(3) {
        facts.number("aperture", v, Some("mm"), "MeasParam tag 3", inf);
    }
    if let Some(v) = f(4) {
        facts.number("scan_speed", v, Some("mm/s"), "MeasParam tag 4", inf);
    }
    if let Some(v) = f(6) {
        facts.number("gain", v, None, "MeasParam tag 6", inf);
    }
    if let Some(v) = f(32) {
        facts.number("filter", v, Some("Hz"), "MeasParam tag 32", inf);
    }
    if let Some(v) = s(47) {
        facts.word("light_source", &v, "MeasParam tag 47", inf);
    }
    if let Some(v) = s(48) {
        facts.word("detector", &v, "MeasParam tag 48", inf);
    }
    if started.is_none()
        && let Some(d) = record(recs, 12)
            .filter(|r| r.kind == 5)
            .and_then(|r| r.value.as_f64())
            .and_then(ole_utc)
    {
        *started = Some((d, "MeasParam tag 12 (OLE date, UTC)"));
    }
}

/// A NUL-terminated text field of the flat header.
fn flat_text(b: &[u8], at: usize, width: usize) -> String {
    b.bytes_at(at, width)
        .map(crate::common::text_field)
        .unwrap_or_default()
}

fn parse_flat(f: &SourceFile, path: &Path, file_len: u64) -> Result<Parsed> {
    if file_len > MAX_FLAT {
        return Err(Error::unsupported(
            FMT,
            format!("a {file_len}-byte flat .jws file"),
            "Flat JASCO files are single spectra of a few kilobytes; this one is not read.",
        ));
    }
    let b = read_at(f, path, 0, file_len.min(0x740), file_len)?;
    if b.len() < 0x300 {
        return Err(Error::corrupt(
            FMT,
            "flat .jws header shorter than 768 bytes",
        ));
    }
    let id = flat_text(&b, 0x08, 16);
    let version = flat_text(&b, 0x20, 16);
    if !(id == "SPECMAN" || id == "SPECIRM")
        || version != "R2.0.0"
        || b.bytes_at(0xA1, 3) != Some(&[1u8, 0, 0x10][..])
    {
        return Err(Error::unsupported(
            FMT,
            format!("a flat JASCO container `{id} {version}` with an unexpected axis descriptor"),
            "Only SPECMAN/SPECIRM R2.0.0 files with a wavenumber or wavelength axis have been validated; please share this file.",
        ));
    }
    let (Some(nch), Some(npoints), Some(first), Some(last), Some(step), Some(xunit), Some(len)) = (
        b.u16_at(0x82),
        b.i32_at(0x84),
        b.f64_at(0x88),
        b.f64_at(0x90),
        b.f64_at(0x98),
        b.u8_at(0xA0),
        b.i64_at(0xC8),
    ) else {
        return Err(Error::corrupt(FMT, "flat .jws header truncated"));
    };
    // One channel code per channel after the x descriptor (the codes of the compound files'
    // `DataInfo`). Validated: one channel of %T, %R, absorbance or single-beam, or CD with HT.
    let codes: Vec<u32> = (0..usize::from(nch.min(8)))
        .map_while(|i| b.u32_at(0xA4 + 4 * i))
        .collect();
    let validated = match codes.as_slice() {
        [c] => matches!(c, 0 | 2 | 3 | 9 | 10),
        [0x1001, 0x2001] => true,
        _ => false,
    };
    if !validated || codes.len() != usize::from(nch) {
        return Err(Error::unsupported(
            FMT,
            format!(
                "a flat JASCO file with {nch} channel(s), codes [{}]",
                codes
                    .iter()
                    .map(|c| format!("{c:#x}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            "Validated flat files hold one %T, %R, absorbance or single-beam channel, or circular dichroism (0x1001) with HT voltage (0x2001). Export the spectrum from Spectra Manager as text, or share this file.",
        ));
    }
    let nch = codes.len() as u64;
    let points = u64::try_from(npoints)
        .ok()
        .filter(|&p| p > 0)
        .ok_or_else(|| Error::corrupt(FMT, format!("{npoints} points")))?;
    let len = u64::try_from(len).map_err(|_| Error::corrupt(FMT, "negative data length"))?;
    if points.checked_mul(4 * nch) != Some(len) || len > file_len || file_len - len < 0x300 {
        return Err(Error::corrupt(
            FMT,
            format!(
                "data length {len} for {nch} channel(s) of {npoints} float32 points in a {file_len}-byte file"
            ),
        ));
    }
    if !step.is_finite() || step == 0.0 || !first.is_finite() {
        return Err(Error::corrupt(FMT, format!("first x {first}, step {step}")));
    }
    // the data run to the end of the file after a 1856-byte header in every file seen: a file
    // cut short (or grown) would shift the values, so anything else is refused
    if file_len - len != FLAT_HEADER {
        return Err(Error::corrupt(
            FMT,
            format!(
                "{} bytes between the header and the {len}-byte data block; every flat file has a {FLAT_HEADER}-byte header (truncated or extended file?)",
                file_len - len
            ),
        ));
    }
    let (xq, xu, ir) = match xunit {
        0 => ("wavenumber", "1/cm", true),
        3 => ("wavelength", "nm", false),
        u => {
            return Err(Error::unsupported(
                FMT,
                format!("x-unit code {u}"),
                "Only wavenumber (0) and wavelength (3) axes have been validated; please share this file.",
            ));
        }
    };
    // every code passed the validated list above, so each has a kind
    let kinds: Vec<(u32, ChannelKind)> = codes
        .iter()
        .filter_map(|&c| channel_kind(c, None).map(|k| (c, k)))
        .collect();
    let mut parsed = Parsed::default();
    let computed = first + step * (points - 1) as f64;
    if (computed - last).abs() > step.abs() {
        parsed.findings.push(Finding::warning(
            "x_range",
            format!("stored last x {last} disagrees with first + step × (points − 1) = {computed}"),
        ));
    }
    let model = flat_text(&b, 0x140, 32);
    let serial = flat_text(&b, 0x160, 32);
    let title = flat_text(&b, 0x180, 64);
    let comment = flat_text(&b, 0x1C0, 64);
    let epoch = b.i32_at(0x2C0).unwrap_or(0);
    let mut facts = Facts {
        vendor: Some(("JASCO".into(), "file format".into())),
        ..Facts::default()
    };
    Facts::text(&mut facts.model, &model, "flat header model (0x140)");
    Facts::text(&mut facts.serial, &serial, "flat header serial (0x160)");
    Facts::text(&mut facts.sample_name, &title, "flat header title (0x180)");
    Facts::text(&mut facts.comment, &comment, "flat header comment (0x1C0)");
    if epoch > 0 {
        facts.started_at = Some((
            unix_to_iso8601(i64::from(epoch), 0).replace(".000Z", "Z"),
            "flat header time (0x2C0, Unix seconds UTC)".into(),
        ));
    }
    parsed.format_version = Some(format!("{id} {version}"));
    // channel-major: each channel's float32 values follow the previous channel's
    for (i, (code, kind)) in kinds.into_iter().enumerate() {
        let mut extra = std::collections::BTreeMap::new();
        extra.insert("channel_code".into(), json!(format!("{code:#x}")));
        if !title.is_empty() {
            extra.insert("title".into(), json!(title));
        }
        parsed.sets.push(SpectrumSet {
            name: kind.name.to_string(),
            x_quantity: xq,
            x_unit: Some(xu.into()),
            x: XValues::Regular {
                first,
                last: computed,
            },
            y_name: kind.name.to_string(),
            y_unit: kind.unit,
            points,
            count: 1,
            rows: Rows::Listed(vec![file_len - len + 4 * points * i as u64]),
            stored: Stored::F32,
            scale: 1.0,
            data_type: if ir {
                "INFRARED SPECTRUM"
            } else {
                kind.nm_type
            },
            extra,
        });
    }
    parsed.facts = facts;
    parsed.vendor = json!({"jasco": {
        "container": "flat",
        "format": id, "version": version,
        "architecture": flat_text(&b, 0x30, 16), "compiler": flat_text(&b, 0x40, 16),
        "points": npoints, "first_x": num(first), "last_x": num(last), "step": num(step),
        "x_unit_code": xunit, "y_mode_code": codes[0], "channel_codes": codes,
        "model": model, "serial": serial, "title": title, "comment": comment,
        "unix_time": epoch,
    }});
    parsed.entries.push(LsEntry {
        kind: "block".into(),
        name: "header".into(),
        offset: Some(0),
        size: Some(file_len - len),
        image: None,
        details: Value::Null,
    });
    parsed.entries.push(LsEntry {
        kind: "block".into(),
        name: "data".into(),
        offset: Some(file_len - len),
        size: Some(len),
        image: None,
        details: json!({"points": npoints, "dtype": "float32"}),
    });
    parsed.notes.push("values as stored (float32)".into());
    parsed.provenance.insert("traces".into(), Source::Inferred);
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)] // exact values
    use super::*;

    #[test]
    fn dates_strings_records() {
        assert_eq!(
            ole_utc(44_931.450_532_407_405).as_deref(),
            Some("2023-01-05T10:48:46Z")
        );
        assert_eq!(ole_utc(NO_DATE), None);
        assert_eq!(ole_utc(f64::NAN), None);
        let mut b = Vec::new();
        b.extend(8u32.to_le_bytes());
        for u in "J-1".encode_utf16().chain(std::iter::once(0)) {
            b.extend(u.to_le_bytes());
        }
        assert_eq!(utf16(&b, 0), Some(("J-1".to_string(), 12)));
        assert_eq!(utf16(&b[..6], 0), None);
        // tag 1 (u32 50), tag 2 (f32 0.5), tag 9 (unknown type 11)
        let mut r = Vec::new();
        r.extend(1u32.to_le_bytes());
        r.extend(3u16.to_le_bytes());
        r.extend(50u32.to_le_bytes());
        r.extend(2u32.to_le_bytes());
        r.extend(4u16.to_le_bytes());
        r.extend(0.5f32.to_le_bytes());
        r.extend(9u32.to_le_bytes());
        r.extend(11u16.to_le_bytes());
        r.extend([0u8; 6]);
        let (recs, rest) = records(&r, 0, 3);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[1].value.as_f64(), Some(0.5));
        assert_eq!(rest, 12);
    }

    #[test]
    fn codes() {
        assert_eq!(
            channel_kind(0x1001, Some("200 mdeg/1.0 dOD"))
                .unwrap()
                .unit
                .as_deref(),
            Some("mdeg")
        );
        assert_eq!(channel_kind(0x1001, None).unwrap().unit, None);
        assert!(channel_kind(0x77, None).is_none());
        assert_eq!(x_axis(0x1000_0103).map(|x| x.0), Some("wavelength"));
        assert!(x_axis(0x3000_0000).is_none());
        let mut h = Vec::new();
        for u in "L~\u{0}SPCMAN2".encode_utf16() {
            h.extend(u.to_le_bytes());
        }
        assert!(header_is_jasco(&h));
        assert!(!header_is_jasco(b"L\0~\0X\0"));
    }
}
