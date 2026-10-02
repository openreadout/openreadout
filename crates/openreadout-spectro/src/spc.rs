//! Galactic / Thermo GRAMS SPC (`.spc`): a 512-byte header (256 in the old 0x4D format), an
//! optional x array, then one or more subfiles (32-byte header + y values), then an optional log
//! block. Layout from SpectroChemPy (CeCILL-B) and spc-io (MIT) read as documentation
//! (`docs/formats/galactic-spc.md`, `docs/provenance/galactic-spc.md`).

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::bytes::{le_f32, le_f64, le_u16, le_u32};
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::Source;
use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};
use serde_json::{Value, json};

use crate::common::{Facts, Parsed, Rows, SpectrumSet, SpectrumTable, Stored, XValues, num};

const FMT: &str = crate::SPC_FORMAT_ID;
/// New-format header length.
pub(crate) const SPC_HEADER_LEN: u64 = 512;
/// Old-format (0x4D) header length.
pub(crate) const SPC_OLD_HEADER_LEN: u64 = 256;
/// Subfile header length.
pub(crate) const SPC_SUBHEADER_LEN: u64 = 32;
/// Subfiles accepted at most.
pub(crate) const MAX_SPC_SUBFILES: u64 = 10_000_000;
/// Log text read at most, bytes.
const MAX_LOG_TEXT: u64 = 1 << 20;

// ftflgs bits
const TSPREC: u8 = 0x01;
const TMULTI: u8 = 0x04;
const TRANDM: u8 = 0x08;
const TORDRD: u8 = 0x10;
const TALABS: u8 = 0x20;
const TXYXYS: u8 = 0x40;
const TXVALS: u8 = 0x80;

/// True when `head` has the shape of a new (LSB) or old Galactic SPC header.
pub(crate) fn looks_like_spc(head: &[u8]) -> bool {
    match head.get(1) {
        Some(0x4B) => {
            if head.len() < 32 {
                return false;
            }
            let rd =
                |o: usize| u32::from_le_bytes([head[o], head[o + 1], head[o + 2], head[o + 3]]);
            let f64at = |o: usize| {
                let mut a = [0u8; 8];
                a.copy_from_slice(&head[o..o + 8]);
                f64::from_le_bytes(a)
            };
            let flags = head[0];
            let npts = rd(4);
            let nsub = rd(24);
            let (first, last) = (f64at(8), f64at(16));
            let (xt, yt) = (head[28], head[29]);
            // A zip (`PK\x03\x04`, e.g. .xlsx) also has 0x4B at byte 1; an XY-XY multifile must
            // also flag multifile and x values; the experiment code is small.
            !head.starts_with(b"PK")
                && (flags & TXYXYS == 0 || flags & (TMULTI | TXVALS) == TMULTI | TXVALS)
                && head[2] <= 20
                && (npts > 0 || flags & TXYXYS != 0)
                && nsub >= 1
                && first.is_finite()
                && last.is_finite()
                && (xt <= 30 || xt == 255)
                && (yt <= 26 || (128..=131).contains(&yt) || yt == 255)
        }
        Some(0x4D) => {
            if head.len() < 18 {
                return false;
            }
            let npts = f32::from_le_bytes([head[4], head[5], head[6], head[7]]);
            npts.is_finite() && npts >= 1.0 && npts.fract() == 0.0 && head[16] <= 30
        }
        _ => false,
    }
}

/// Our x quantity and unit for an SPC x-type code.
fn x_axis(code: u8) -> (&'static str, Option<&'static str>, &'static str) {
    match code {
        1 => ("wavenumber", Some("1/cm"), "INFRARED SPECTRUM"),
        2 => ("wavelength", Some("µm"), "INFRARED SPECTRUM"),
        3 => ("wavelength", Some("nm"), "UV/VIS SPECTRUM"),
        4 => ("time", Some("s"), "UNKNOWN"),
        5 => ("time", Some("min"), "UNKNOWN"),
        6 => ("frequency", Some("Hz"), "UNKNOWN"),
        7 => ("frequency", Some("kHz"), "UNKNOWN"),
        8 => ("frequency", Some("MHz"), "UNKNOWN"),
        9 => ("mass_to_charge", Some("m/z"), "MASS SPECTRUM"),
        10 => ("chemical_shift", Some("ppm"), "NMR SPECTRUM"),
        11 => ("time", Some("d"), "UNKNOWN"),
        12 => ("time", Some("a"), "UNKNOWN"),
        13 => ("raman_shift", Some("1/cm"), "RAMAN SPECTRUM"),
        14 => ("energy", Some("eV"), "UNKNOWN"),
        16 => ("diode", None, "UNKNOWN"),
        17 => ("channel", None, "UNKNOWN"),
        18 => ("angle", Some("°"), "UNKNOWN"),
        19 => ("temperature", Some("°F"), "UNKNOWN"),
        20 => ("temperature", Some("°C"), "UNKNOWN"),
        21 => ("temperature", Some("K"), "UNKNOWN"),
        22 => ("points", None, "UNKNOWN"),
        23 => ("time", Some("ms"), "UNKNOWN"),
        24 => ("time", Some("µs"), "UNKNOWN"),
        25 => ("time", Some("ns"), "UNKNOWN"),
        26 => ("frequency", Some("GHz"), "UNKNOWN"),
        27 => ("wavelength", Some("cm"), "UNKNOWN"),
        28 => ("wavelength", Some("m"), "UNKNOWN"),
        29 => ("wavelength", Some("mm"), "UNKNOWN"),
        30 => ("time", Some("h"), "UNKNOWN"),
        _ => ("x", None, "UNKNOWN"),
    }
}

/// Our y quantity and unit for an SPC y-type code.
fn y_axis(code: u8) -> (&'static str, Option<&'static str>) {
    match code {
        1 => ("interferogram", None),
        2 => ("absorbance", Some("AU")),
        3 => ("kubelka_munk", None),
        4 => ("counts", None),
        5 => ("voltage", Some("V")),
        6 => ("angle", Some("°")),
        7 => ("current", Some("mA")),
        8 => ("length", Some("mm")),
        9 => ("voltage", Some("mV")),
        10 => ("log_1_r", None),
        11 => ("percent", Some("%")),
        12 => ("intensity", None),
        13 => ("relative_intensity", None),
        14 => ("energy", None),
        16 => ("level", Some("dB")),
        19 => ("temperature", Some("°F")),
        20 => ("temperature", Some("°C")),
        21 => ("temperature", Some("K")),
        22 => ("refractive_index", None),
        23 => ("extinction_coefficient", None),
        24 => ("real", None),
        25 => ("imaginary", None),
        26 => ("complex", None),
        128 => ("transmittance", None),
        129 => ("reflectance", None),
        130 => ("single_beam", None),
        131 => ("emission", None),
        _ => ("intensity", None),
    }
}

/// The instrument technique of the `fexper` code.
fn technique(code: u8) -> &'static str {
    match code {
        1 => "gas chromatogram",
        2 => "chromatogram",
        3 => "HPLC chromatogram",
        4 => "FT-IR, FT-NIR or FT-Raman spectrum",
        5 => "NIR spectrum",
        7 => "UV-Vis spectrum",
        8 => "X-ray diffraction",
        9 => "mass spectrum",
        10 => "NMR spectrum or FID",
        11 => "Raman spectrum",
        12 => "fluorescence spectrum",
        13 => "atomic spectrum",
        14 => "diode-array spectra",
        _ => "general",
    }
}

fn read(f: &SourceFile, path: &Path, at: u64, n: u64, file_len: u64) -> Result<Vec<u8>> {
    if at.checked_add(n).is_none_or(|e| e > file_len) {
        return Err(Error::corrupt_at(
            FMT,
            at,
            format!("{n} bytes at offset {at} run past the end of the file ({file_len} bytes)"),
        ));
    }
    let mut buf = vec![0u8; n as usize];
    f.read_exact_at(at, &mut buf)
        .map_err(|e| Error::io(path, e))?;
    Ok(buf)
}

/// The packed collection date (minute 6 bits, hour 5, day 5, month 4, year 12).
fn packed_date(v: u32) -> Option<String> {
    let year = v >> 20;
    let month = (v >> 16) & 15;
    let day = (v >> 11) & 31;
    let hour = (v >> 6) & 31;
    let minute = v & 63;
    ((1950..=2100).contains(&year)
        && (1..=12).contains(&month)
        && (1..=31).contains(&day)
        && hour < 24
        && minute < 60)
        .then(|| format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:00"))
}

/// `key=value` lines of the log text.
fn log_pairs(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in text.split(['\r', '\n']) {
        if let Some((k, v)) = line.split_once('=') {
            let k = k.trim();
            let v = v.trim().trim_matches('"').trim();
            if !k.is_empty() && !out.contains_key(k) {
                out.insert(k.to_string(), v.to_string());
            }
        }
    }
    out
}

/// One subfile header.
#[derive(Debug, Clone, Copy)]
struct Sub {
    offset: u64,
    exp: i8,
    index: u16,
    time: f32,
    next: f32,
    scans: u32,
    w: f32,
}

fn sub_at(b: &[u8], offset: u64) -> Sub {
    Sub {
        offset,
        exp: b[1] as i8,
        index: le_u16(b, 2).unwrap_or(0),
        time: le_f32(b, 4).unwrap_or(0.0),
        next: le_f32(b, 8).unwrap_or(0.0),
        scans: le_u32(b, 20).unwrap_or(0),
        w: le_f32(b, 24).unwrap_or(0.0),
    }
}

/// Parse the headers of an SPC file.
pub(crate) fn parse(f: &SourceFile, path: &Path, file_len: u64) -> Result<Parsed> {
    let head = read(f, path, 0, file_len.min(SPC_HEADER_LEN), file_len)?;
    match head.get(1) {
        Some(0x4B) => parse_new(f, path, file_len, &head),
        Some(0x4D) => parse_old(file_len, &head),
        Some(0x4C) => Err(Error::unsupported(
            FMT,
            "big-endian (0x4C) SPC files",
            "this SPC file stores its numbers most-significant byte first; no development file does, so it is not read. Re-save it as SPC from GRAMS or Spectragryph, or export CSV.",
        )),
        _ => Err(Error::corrupt(
            FMT,
            "not a Galactic SPC file: byte 1 is not a known version (0x4B, 0x4C, 0x4D)",
        )),
    }
}

/// The fields of the new-format (0x4B) header.
struct Header {
    flags: u8,
    /// `fexper`: the instrument technique code.
    exper: u8,
    /// `fexp`: the y exponent (-128: float32 values).
    exp: i8,
    points: u64,
    first: f64,
    last: f64,
    subfiles: u64,
    x_type: u8,
    y_type: u8,
    z_type: u8,
    date: u32,
    resolution: String,
    source: String,
    peak_point: u16,
    comment: String,
    /// Axis labels from `fcatxt` (only when `TALABS` is set).
    labels: Vec<String>,
    log_offset: u64,
    mods: u32,
    method: String,
    z_increment: f32,
    w_planes: u32,
    w_increment: f32,
}

impl Header {
    /// `h` is at least `SPC_HEADER_LEN` bytes.
    fn of(h: &[u8]) -> Self {
        let flags = h[0];
        Header {
            flags,
            exper: h[2],
            exp: h[3] as i8,
            points: u64::from(le_u32(h, 4).unwrap_or(0)),
            first: le_f64(h, 8).unwrap_or(0.0),
            last: le_f64(h, 16).unwrap_or(0.0),
            subfiles: u64::from(le_u32(h, 24).unwrap_or(0)),
            x_type: h[28],
            y_type: h[29],
            z_type: h[30],
            date: le_u32(h, 32).unwrap_or(0),
            resolution: crate::common::text_field(&h[36..45]),
            source: crate::common::text_field(&h[45..54]),
            peak_point: le_u16(h, 54).unwrap_or(0),
            comment: crate::common::text_field(&h[88..218]),
            labels: if flags & TALABS != 0 {
                h[218..248]
                    .split(|&c| c == 0)
                    .take(3)
                    .map(crate::common::text_field)
                    .collect()
            } else {
                Vec::new()
            },
            log_offset: u64::from(le_u32(h, 248).unwrap_or(0)),
            mods: le_u32(h, 252).unwrap_or(0),
            method: crate::common::text_field(&h[264..312]),
            z_increment: le_f32(h, 312).unwrap_or(0.0),
            w_planes: le_u32(h, 316).unwrap_or(0),
            w_increment: le_f32(h, 320).unwrap_or(0.0),
        }
    }

    fn multi(&self) -> bool {
        self.flags & TMULTI != 0
    }

    fn float_y(&self) -> bool {
        self.exp == -128
    }

    /// Bytes per stored y value.
    fn value_width(&self) -> u64 {
        if self.float_y() || self.flags & TSPREC == 0 {
            4
        } else {
            2
        }
    }

    /// The exponent that scales subfile `s`: the main one for single-subfile files, each
    /// subfile's own in multifiles.
    fn exp_of(&self, s: &Sub) -> i8 {
        if self.multi() { s.exp } else { self.exp }
    }
}

/// The subfiles of a new-format file and where their data end.
struct Subfiles {
    subs: Vec<Sub>,
    stride: u64,
    data_end: u64,
}

fn parse_new(f: &SourceFile, path: &Path, file_len: u64, h: &[u8]) -> Result<Parsed> {
    if (h.len() as u64) < SPC_HEADER_LEN {
        return Err(Error::corrupt(
            FMT,
            format!("the file is {file_len} bytes, shorter than the 512-byte SPC header"),
        ));
    }
    let hd = Header::of(h);
    if hd.flags & TXYXYS != 0 {
        return Err(Error::unsupported(
            FMT,
            "SPC files with a separate x array per subfile (TXYXYS)",
            "each subfile of this file has its own x values (typically mass spectra); no development file has that layout, so it is not read.",
        ));
    }
    if hd.points == 0 {
        return Err(Error::corrupt(FMT, "the header declares 0 points"));
    }
    let nsub = if hd.multi() { hd.subfiles } else { 1 };
    if nsub == 0 || nsub > MAX_SPC_SUBFILES {
        return Err(Error::corrupt(
            FMT,
            format!("the header declares {} subfiles", hd.subfiles),
        ));
    }
    let mut parsed = Parsed::default();
    let mut at = SPC_HEADER_LEN;
    let x = if hd.flags & TXVALS != 0 {
        let b = read(f, path, at, hd.points * 4, file_len)?;
        at += hd.points * 4;
        XValues::Listed(
            b.as_chunks::<4>()
                .0
                .iter()
                .map(|c| f64::from(f32::from_le_bytes(*c)))
                .collect(),
        )
    } else {
        XValues::Regular {
            first: hd.first,
            last: hd.last,
        }
    };
    let Subfiles {
        subs,
        stride,
        data_end,
    } = read_subfiles(f, path, file_len, &hd, at, nsub, &mut parsed)?;
    let (stored, scales) = value_scales(&hd, &subs)?;
    let (xq, xu, mut data_type) = x_axis(hd.x_type);
    let (yq, yu) = y_axis(hd.y_type);
    if hd.exper == 11 {
        data_type = "RAMAN SPECTRUM";
    } else if hd.exper == 7 {
        data_type = "UV/VIS SPECTRUM";
    }
    let date = packed_date(hd.date);
    let mut extra = header_extra(&hd, &subs, date.as_deref());
    let (log, log_text) = read_log(f, path, file_len, hd.log_offset, &mut parsed)?;
    if !log.is_empty() {
        extra.insert("log".into(), json!(log));
    }
    parsed.facts = header_facts(&hd, date.as_deref(), &log);
    let count = subs.len() as u32;
    let set = SpectrumSet {
        name: if hd.comment.is_empty() {
            yq.to_string()
        } else {
            hd.comment.clone()
        },
        x_quantity: xq,
        x_unit: xu.map(str::to_string),
        x,
        y_name: yq.into(),
        y_unit: yu.map(str::to_string),
        points: hd.points,
        count,
        rows: Rows::Strided {
            first: subs[0].offset + SPC_SUBHEADER_LEN,
            stride,
        },
        stored,
        scale: scales[0],
        data_type,
        extra,
    };
    if scales.windows(2).any(|w| w[0].to_bits() != w[1].to_bits()) {
        parsed.row_scales.insert(0, scales);
    }
    parsed.sets.push(set);
    if hd.multi() {
        parsed.tables.push(subfile_table(&hd, &subs));
    }
    if hd.multi() && hd.subfiles != u64::from(count) && data_end <= file_len {
        parsed.findings.push(Finding::warning(
            "subfile_count",
            format!(
                "the header declares {} subfiles, {count} were read",
                hd.subfiles
            ),
        ));
    }
    let logoff = hd.log_offset;
    if logoff > 0 && data_end <= file_len && logoff < data_end {
        parsed.findings.push(Finding::warning(
            "log_overlaps_data",
            format!("the log block (offset {logoff}) starts inside the subfiles (which end at {data_end})"),
        ));
    }
    parsed.format_version = Some("new (0x4B)".into());
    parsed.vendor = header_json(&hd, &subs, &log_text);
    parsed.entries.push(LsEntry {
        kind: "header".into(),
        name: "SPC header".into(),
        offset: Some(0),
        size: Some(SPC_HEADER_LEN),
        image: None,
        details: json!({"version": "0x4B"}),
    });
    parsed.entries.push(LsEntry {
        kind: "subfiles".into(),
        name: format!("{count} subfile(s)"),
        offset: Some(subs[0].offset),
        size: Some(stride * u64::from(count)),
        image: None,
        details: json!({"points": hd.points, "value_bytes": hd.value_width()}),
    });
    if logoff > 0 {
        parsed.entries.push(LsEntry {
            kind: "log".into(),
            name: "log block".into(),
            offset: Some(logoff),
            size: Some(file_len.saturating_sub(logoff)),
            image: None,
            details: Value::Null,
        });
    }
    parsed.notes.push(format!(
        "Galactic SPC: {count} spectrum(s) of {} points ({}); values {}",
        hd.points,
        technique(hd.exper),
        if hd.float_y() {
            "stored as float32"
        } else {
            "stored as fixed-point integers scaled by 2^exponent"
        }
    ));
    provenance(&mut parsed);
    Ok(parsed)
}

/// The subfile headers from `at` on: all `nsub` of them, or (in a truncated file, recorded as
/// a finding) the complete ones.
fn read_subfiles(
    f: &SourceFile,
    path: &Path,
    file_len: u64,
    hd: &Header,
    at: u64,
    nsub: u64,
    parsed: &mut Parsed,
) -> Result<Subfiles> {
    let npts = hd.points;
    let stride = SPC_SUBHEADER_LEN + npts * hd.value_width();
    let data_end = at.saturating_add(stride.saturating_mul(nsub));
    if data_end > file_len {
        parsed.findings.push(Finding::error(
            "truncated",
            format!(
                "{nsub} subfile(s) of {npts} points need bytes up to {data_end}, the file has {file_len}"
            ),
        ));
    }
    let whole = if data_end <= file_len {
        nsub
    } else {
        file_len.saturating_sub(at) / stride
    };
    let mut subs = Vec::with_capacity(whole as usize);
    for k in 0..whole {
        let off = at + k * stride;
        let b = read(f, path, off, SPC_SUBHEADER_LEN, file_len)?;
        subs.push(sub_at(&b, off));
    }
    if subs.is_empty() {
        return Err(Error::corrupt(FMT, "no complete subfile in the file"));
    }
    Ok(Subfiles {
        subs,
        stride,
        data_end,
    })
}

/// How the y values are stored, and the scale of each subfile.
fn value_scales(hd: &Header, subs: &[Sub]) -> Result<(Stored, Vec<f64>)> {
    let float_y = hd.float_y();
    if !float_y && hd.multi() && subs.iter().any(|s| s.exp == -128) {
        return Err(Error::unsupported(
            FMT,
            "SPC multifiles mixing float and fixed-point subfiles",
            "some subfiles of this fixed-point file are marked as float; no development file does that.",
        ));
    }
    Ok(if float_y {
        (Stored::F32, vec![1.0; subs.len()])
    } else if hd.value_width() == 2 {
        (
            Stored::I16,
            subs.iter()
                .map(|s| 2f64.powi(i32::from(hd.exp_of(s)) - 16))
                .collect(),
        )
    } else {
        (
            Stored::I32,
            subs.iter()
                .map(|s| 2f64.powi(i32::from(hd.exp_of(s)) - 32))
                .collect(),
        )
    })
}

/// The header fields kept on the spectrum set.
fn header_extra(hd: &Header, subs: &[Sub], date: Option<&str>) -> BTreeMap<String, Value> {
    let mut extra = BTreeMap::new();
    extra.insert("technique".into(), json!(technique(hd.exper)));
    extra.insert("technique_code".into(), json!(hd.exper));
    extra.insert("x_type_code".into(), json!(hd.x_type));
    extra.insert("y_type_code".into(), json!(hd.y_type));
    if hd.multi() {
        extra.insert("z_type_code".into(), json!(hd.z_type));
        let (zq, zu, _) = x_axis(hd.z_type);
        extra.insert("z_quantity".into(), json!(zq));
        if let Some(u) = zu {
            extra.insert("z_unit".into(), json!(u));
        }
    }
    if !hd.labels.is_empty() {
        extra.insert("axis_labels".into(), json!(hd.labels));
    }
    extra.insert(
        "value_encoding".into(),
        json!(if hd.float_y() {
            "float32"
        } else if hd.value_width() == 2 {
            "fixed16"
        } else {
            "fixed32"
        }),
    );
    if !hd.float_y() {
        let exps: Vec<i8> = subs.iter().map(|s| hd.exp_of(s)).collect();
        if exps.windows(2).all(|w| w[0] == w[1]) {
            extra.insert("exponent".into(), json!(exps[0]));
        }
    }
    for (key, text) in [
        ("resolution", &hd.resolution),
        ("source_instrument", &hd.source),
        ("comment", &hd.comment),
        ("method", &hd.method),
    ] {
        if !text.is_empty() {
            extra.insert(key.into(), json!(text));
        }
    }
    if hd.peak_point != 0 {
        extra.insert("peak_point".into(), json!(hd.peak_point));
    }
    if hd.mods != 0 {
        extra.insert("modification_flags".into(), json!(hd.mods));
    }
    if let Some(d) = date {
        extra.insert("recorded_at".into(), json!(d));
    }
    if hd.w_planes > 0 {
        extra.insert("w_planes".into(), json!(hd.w_planes));
        extra.insert("w_increment".into(), num(f64::from(hd.w_increment)));
    }
    if hd.multi() && hd.z_increment != 0.0 {
        extra.insert("z_increment".into(), num(f64::from(hd.z_increment)));
    }
    extra
}

/// The log block at `logoff` (0: none): its `key=value` pairs and its text.
fn read_log(
    f: &SourceFile,
    path: &Path,
    file_len: u64,
    logoff: u64,
    parsed: &mut Parsed,
) -> Result<(BTreeMap<String, String>, String)> {
    if logoff == 0 {
        return Ok((BTreeMap::new(), String::new()));
    }
    if logoff + 20 > file_len {
        parsed.findings.push(Finding::warning(
            "log_out_of_bounds",
            format!("the log offset {logoff} lies past the end of the file"),
        ));
        return Ok((BTreeMap::new(), String::new()));
    }
    let lb = read(f, path, logoff, 20, file_len)?;
    let sizd = u64::from(le_u32(&lb, 0).unwrap_or(0));
    let txto = u64::from(le_u32(&lb, 8).unwrap_or(0));
    let start = logoff.saturating_add(txto);
    let end = logoff.saturating_add(sizd).min(file_len);
    if txto < 64 || start >= end {
        return Ok((BTreeMap::new(), String::new()));
    }
    let tb = read(f, path, start, (end - start).min(MAX_LOG_TEXT), file_len)?;
    let text = crate::common::text_field(&tb);
    Ok((log_pairs(&text), text))
}

fn header_facts(hd: &Header, date: Option<&str>, log: &BTreeMap<String, String>) -> Facts {
    let mut facts = Facts::default();
    if !hd.source.is_empty() {
        Facts::text(
            &mut facts.model,
            &hd.source,
            "source instrument text (fsource)",
        );
    }
    if let Some(d) = date {
        Facts::text(&mut facts.started_at, d, "collection date (fdate)");
    }
    if !hd.comment.is_empty() {
        Facts::text(&mut facts.comment, &hd.comment, "comment (fcmnt)");
    }
    if !hd.method.is_empty() {
        Facts::text(&mut facts.method_name, &hd.method, "method (fmethod)");
    }
    for key in ["Operator", "USER", "User"] {
        if let Some(v) = log.get(key) {
            Facts::text(&mut facts.operator, v, &format!("log text ({key})"));
        }
    }
    facts
}

/// One row per subfile of a multifile: index, z value, scans and w.
fn subfile_table(hd: &Header, subs: &[Sub]) -> SpectrumTable {
    let ordered = hd.flags & (TORDRD | TRANDM) != 0;
    let z: Vec<f64> = subs
        .iter()
        .map(|s| {
            if ordered {
                f64::from(s.time)
            } else {
                // evenly spaced: first subfile's time + index × increment
                let inc = if hd.z_increment == 0.0 {
                    f64::from(subs[0].next) - f64::from(subs[0].time)
                } else {
                    f64::from(hd.z_increment)
                };
                f64::from(subs[0].time) + f64::from(s.index) * inc
            }
        })
        .collect();
    let mut columns = vec![
        (
            "subfile_index".to_string(),
            None,
            subs.iter().map(|s| f64::from(s.index)).collect(),
        ),
        ("z".to_string(), x_axis(hd.z_type).1.map(str::to_string), z),
        (
            "scans".to_string(),
            None,
            subs.iter().map(|s| f64::from(s.scans)).collect(),
        ),
    ];
    if hd.w_planes > 0 {
        columns.push((
            "w".to_string(),
            None,
            subs.iter().map(|s| f64::from(s.w)).collect(),
        ));
    }
    SpectrumTable {
        name: "subfiles".into(),
        trace: 0,
        columns,
    }
}

fn header_json(hd: &Header, subs: &[Sub], log_text: &str) -> Value {
    json!({
        "version": "0x4B",
        "flags": hd.flags,
        "technique_code": hd.exper,
        "exponent": hd.exp,
        "points": hd.points,
        "first_x": num(hd.first),
        "last_x": num(hd.last),
        "subfiles": hd.subfiles,
        "x_type": hd.x_type,
        "y_type": hd.y_type,
        "z_type": hd.z_type,
        "date_packed": hd.date,
        "resolution": hd.resolution,
        "source": hd.source,
        "peak_point": hd.peak_point,
        "comment": hd.comment,
        "axis_labels": hd.labels,
        "log_offset": hd.log_offset,
        "modification_flags": hd.mods,
        "method": hd.method,
        "z_increment": num(f64::from(hd.z_increment)),
        "w_planes": hd.w_planes,
        "w_increment": num(f64::from(hd.w_increment)),
        "log_text": log_text,
        "subfile_headers": subs.iter().take(1000).map(|s| json!({
            "offset": s.offset, "exponent": s.exp, "index": s.index, "time": num(f64::from(s.time)),
            "next": num(f64::from(s.next)), "scans": s.scans, "w": num(f64::from(s.w)),
        })).collect::<Vec<Value>>(),
    })
}

fn parse_old(file_len: u64, h: &[u8]) -> Result<Parsed> {
    if (h.len() as u64) < SPC_OLD_HEADER_LEN {
        return Err(Error::corrupt(
            FMT,
            format!("the file is {file_len} bytes, shorter than the 256-byte old SPC header"),
        ));
    }
    let flags = h[0];
    if flags & (TMULTI | TSPREC | TXVALS) != 0 {
        return Err(Error::unsupported(
            FMT,
            "old-format (0x4D) SPC multifiles, 16-bit values or x arrays",
            "this old-format SPC file uses a layout no development file has; re-save it in the new SPC format from GRAMS or Spectragryph.",
        ));
    }
    let exp = i16::from_le_bytes([h[2], h[3]]);
    let npts_f = le_f32(h, 4).unwrap_or(0.0);
    if !(npts_f.is_finite() && npts_f >= 1.0 && npts_f.fract() == 0.0) {
        return Err(Error::corrupt(
            FMT,
            format!("old-format point count {npts_f} is not a positive integer"),
        ));
    }
    let npts = npts_f as u64;
    let first = f64::from(le_f32(h, 8).unwrap_or(0.0));
    let last = f64::from(le_f32(h, 12).unwrap_or(0.0));
    let (xtype, ytype) = (h[16], h[17]);
    let year_word = le_u16(h, 18).unwrap_or(0);
    let (month, day, hour, minute) = (h[20], h[21], h[22], h[23]);
    let resolution = crate::common::text_field(&h[24..32]);
    let comment = crate::common::text_field(&h[64..194]);
    let data_at = SPC_OLD_HEADER_LEN;
    let mut parsed = Parsed::default();
    if data_at + npts * 4 > file_len {
        parsed.findings.push(Finding::error(
            "truncated",
            format!(
                "{npts} points need bytes up to {}, the file has {file_len}",
                data_at + npts * 4
            ),
        ));
    }
    let (xq, xu, data_type) = x_axis(xtype);
    let (yq, yu) = y_axis(ytype);
    let year = year_word & 0x0FFF;
    let date = ((1950..=2100).contains(&year)
        && (1..=12).contains(&month)
        && (1..=31).contains(&day)
        && hour < 24
        && minute < 60)
        .then(|| format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:00"));
    let mut extra = BTreeMap::new();
    extra.insert("x_type_code".into(), json!(xtype));
    extra.insert("y_type_code".into(), json!(ytype));
    extra.insert("value_encoding".into(), json!("fixed32-word-swapped"));
    extra.insert("exponent".into(), json!(exp));
    if !resolution.is_empty() {
        extra.insert("resolution".into(), json!(resolution));
    }
    if !comment.is_empty() {
        extra.insert("comment".into(), json!(comment));
    }
    if let Some(d) = &date {
        extra.insert("recorded_at".into(), json!(d));
    }
    let mut facts = Facts::default();
    if let Some(d) = &date {
        Facts::text(&mut facts.started_at, d, "collection date (old header)");
    }
    if !comment.is_empty() {
        Facts::text(&mut facts.comment, &comment, "comment (old header)");
    }
    parsed.facts = facts;
    parsed.sets.push(SpectrumSet {
        name: if comment.is_empty() {
            yq.to_string()
        } else {
            comment.clone()
        },
        x_quantity: xq,
        x_unit: xu.map(str::to_string),
        x: XValues::Regular { first, last },
        y_name: yq.into(),
        y_unit: yu.map(str::to_string),
        points: npts,
        count: 1,
        rows: Rows::Listed(vec![data_at]),
        stored: Stored::I32WordSwapped,
        scale: 2f64.powi(i32::from(exp) - 32),
        data_type,
        extra,
    });
    parsed.format_version = Some("old (0x4D)".into());
    parsed.vendor = json!({
        "version": "0x4D",
        "flags": flags,
        "exponent": exp,
        "points": npts,
        "first_x": num(first),
        "last_x": num(last),
        "x_type": xtype,
        "y_type": ytype,
        "year_word": year_word,
        "month": month, "day": day, "hour": hour, "minute": minute,
        "resolution": resolution,
        "comment": comment,
    });
    parsed.entries.push(LsEntry {
        kind: "header".into(),
        name: "SPC header (old format)".into(),
        offset: Some(0),
        size: Some(SPC_OLD_HEADER_LEN),
        image: None,
        details: json!({"version": "0x4D"}),
    });
    parsed.notes.push(format!(
        "Galactic SPC, old format (0x4D): one spectrum of {npts} points, 32-bit fixed-point values"
    ));
    provenance(&mut parsed);
    Ok(parsed)
}

fn provenance(parsed: &mut Parsed) {
    for (k, v) in [
        ("format_version", Source::PriorArt),
        ("traces[].sample_count", Source::PriorArt),
        ("traces[].sweep_count", Source::PriorArt),
        ("traces[].extra.axis", Source::PriorArt),
        ("traces[].extra.y_quantity", Source::PriorArt),
        ("traces[].extra.technique", Source::PriorArt),
        ("traces[].extra.recorded_at", Source::PriorArt),
        ("traces[].extra.log", Source::Inferred),
        ("traces[].channels[].scale", Source::PriorArt),
        ("tables[].columns", Source::PriorArt),
    ] {
        parsed.provenance.insert(k.into(), v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_logs() {
        // 2023-11-08 13:00 as packed by OMNIC
        let v = (2023u32 << 20) | (11 << 16) | (8 << 11) | (13 << 6);
        assert_eq!(packed_date(v).as_deref(), Some("2023-11-08T13:00:00"));
        assert_eq!(packed_date(0), None);
        let l = log_pairs("NAME = \"PF1801\"\r\nSCANS = 128\r\n\0");
        assert_eq!(l.get("NAME").map(String::as_str), Some("PF1801"));
        assert_eq!(l.get("SCANS").map(String::as_str), Some("128"));
    }

    #[test]
    fn sniffing() {
        let mut h = vec![0u8; 512];
        h[1] = 0x4B;
        h[4..8].copy_from_slice(&10u32.to_le_bytes());
        h[24..28].copy_from_slice(&1u32.to_le_bytes());
        h[28] = 1;
        h[29] = 2;
        assert!(looks_like_spc(&h));
        // a zip (.xlsx) header also has 0x4B at byte 1 and must not look like SPC
        let mut z = h.clone();
        z[..4].copy_from_slice(b"PK\x03\x04");
        assert!(!looks_like_spc(&z));
        // XY-XY without the multifile and x-values flags is not a consistent SPC header
        let mut x = h.clone();
        x[0] = TXYXYS;
        assert!(!looks_like_spc(&x));
        h[29] = 77;
        assert!(!looks_like_spc(&h));
        let mut o = vec![0u8; 32];
        o[1] = 0x4D;
        o[4..8].copy_from_slice(&2388f32.to_le_bytes());
        assert!(looks_like_spc(&o));
        o[4..8].copy_from_slice(&1.5f32.to_le_bytes());
        assert!(!looks_like_spc(&o));
    }
}
