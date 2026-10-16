//! CBF / imgCIF (Crystallographic Binary File): CIF text with MIME-style binary sections, as
//! PILATUS and EIGER detectors write it ("miniCBF", one image and a `# key value` header) and
//! as full imgCIF with the experiment in CIF categories. Images are byte-offset compressed (or
//! stored raw). See `docs/formats/cbf.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::diffraction::Geometry;
use openreadout_core::model::{
    ChannelInfo, CheckReport, DetectConfidence, FileInfo, Finding, FormatDescriptor, ImageInfo,
    InstrumentInfo, LsEntry,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, Detection, FormatReader, PlaneIndex};
use openreadout_core::source::{Input, SourceFile};
use openreadout_core::{Error, PixelType, Plane, Result};
use serde_json::{Map, Value, json};

use crate::common::{self, number};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "cbf";
/// Opens a binary section.
pub const BINARY_MARKER: &[u8] = b"--CIF-BINARY-FORMAT-SECTION--";
/// The four bytes right before the binary data of a section.
pub const START_MARKER: [u8; 4] = [0x0c, 0x1a, 0x04, 0xd5];
/// Text read at most before the first binary section (the CIF header).
const MAX_HEADER: u64 = 4 << 20;
/// Bytes searched after a section's data for the next marker or the end of the file.
const SCAN_CHUNK: u64 = 64 << 10;
/// Binary sections read at most.
const MAX_SECTIONS: usize = 4096;
/// MIME header lines of one section read at most.
const MAX_MIME_LINES: usize = 64;

/// The CBF reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct CbfReader;

impl FormatReader for CbfReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::CBF)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "CBF / imgCIF".into(),
            vendor: "open standard (IUCr imgCIF; PILATUS/EIGER miniCBF)".into(),
            extensions: vec!["cbf".into()],
            family: "diffraction".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::CBF.confidence,
            known_gaps: vec![
                "Byte-offset and uncompressed binary sections are decoded; packed, canonical and other CBFlib compressions are refused (exit 6)".into(),
                "Full imgCIF: only the wavelength is taken from the CIF categories; the axis and detector-geometry categories are listed raw (`info --view full`), not turned into a beam centre or distance".into(),
                "Each file is one image: the files of a rotation sweep are not grouped (use `batch` over them)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        let head = gz_peek(head);
        let head = head.as_deref().unwrap_or(&[]);
        if head.starts_with(b"###CBF") {
            return Some(det(DetectConfidence::Definite, None));
        }
        let ext = openreadout_core::reader::has_extension(path, &["cbf"])
            || path
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with(".cbf.gz");
        let has_cif = find(head, b"data_").is_some() || find(head, BINARY_MARKER).is_some();
        (ext && has_cif).then(|| {
            det(
                DetectConfidence::Likely,
                Some("CIF text with a .cbf extension but no ###CBF line"),
            )
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(CbfDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

fn det(confidence: DetectConfidence, note: Option<&str>) -> Detection {
    Detection {
        format_id: FORMAT_ID,
        confidence,
        note: note.map(str::to_string),
    }
}

/// The head of a file, decompressed when it is gzip-compressed.
fn gz_peek(head: &[u8]) -> Option<Vec<u8>> {
    if openreadout_core::gzip::is_gzip(head) {
        openreadout_core::gzip::peek(head, 4096)
    } else {
        Some(head.to_vec())
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// How a section's data is coded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conversion {
    /// `x-CBF_BYTE_OFFSET`.
    ByteOffset,
    /// `x-CBF_NONE`: samples stored as they are.
    Uncompressed,
}

/// One binary section: its MIME header and where its data lies.
#[derive(Debug, Clone)]
pub struct Section {
    /// Offset of the `--CIF-BINARY-FORMAT-SECTION--` line.
    pub marker_offset: u64,
    /// Offset of the first data byte (after the start marker).
    pub data_offset: u64,
    /// Bytes of coded data (`X-Binary-Size`).
    pub binary_size: u64,
    /// The MIME header lines, keys lower-cased.
    pub mime: BTreeMap<String, String>,
    /// How the data is coded.
    pub conversion: Conversion,
    /// The conversion as written, for messages.
    pub conversion_text: String,
    /// Sample type of the decoded values.
    pub element: PixelType,
    /// Big-endian samples (`BIG_ENDIAN`).
    pub big_endian: bool,
    /// Samples per row, rows, planes.
    pub dims: [u32; 3],
    /// `X-Binary-Number-of-Elements`.
    pub elements: u64,
    /// `Content-MD5`, base64.
    pub md5: Option<String>,
    /// `X-Binary-Size-Padding`.
    pub padding: Option<u64>,
}

/// The parsed file: CIF header, PILATUS header lines and binary sections.
#[derive(Debug, Clone, Default)]
pub struct CbfLayout {
    /// File length (decompressed).
    pub file_len: u64,
    /// The CBF version on the `###CBF: VERSION 1.5` line.
    pub version: Option<String>,
    /// The CIF data block name (`data_<name>`).
    pub block: Option<String>,
    /// `_array_data.header_convention` (`PILATUS_1.2`, `SLS_1.0`, ...).
    pub convention: Option<String>,
    /// The `# key value` lines of a miniCBF header, in file order.
    pub header_lines: Vec<String>,
    /// Single CIF items `_category.item value` before the first binary section.
    pub items: BTreeMap<String, String>,
    /// CIF loops before the first section: their tags and rows.
    pub loops: Vec<(Vec<String>, Vec<Vec<String>>)>,
    /// Binary sections in file order.
    pub sections: Vec<Section>,
    /// Problems found while parsing (reported by `info` notes and `check`).
    pub problems: Vec<(&'static str, String)>,
}

impl CbfLayout {
    /// Parse the CIF text and every binary section header of `file`.
    pub fn read(file: &SourceFile, file_len: u64) -> Result<Self> {
        let mut l = CbfLayout {
            file_len,
            ..CbfLayout::default()
        };
        let head_len = file_len.min(MAX_HEADER);
        let head = common::read_exact(file, 0, head_len, file_len, FORMAT_ID, "CIF header")?;
        let first = find(&head, BINARY_MARKER);
        let text_end = first.unwrap_or(head.len());
        let text = String::from_utf8_lossy(&head[..text_end]).into_owned();
        l.version = text
            .lines()
            .next()
            .and_then(|f| f.strip_prefix("###CBF:"))
            .and_then(|r| r.trim().strip_prefix("VERSION"))
            .and_then(|r| r.split([',', ' ']).find(|w| !w.is_empty()))
            .map(str::to_string);
        l.parse_cif(&text);
        let Some(first) = first else {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "no binary section (`--CIF-BINARY-FORMAT-SECTION--`) in the first {head_len} bytes: a CIF without an image"
                ),
            ));
        };
        let mut at = first as u64;
        while l.sections.len() < MAX_SECTIONS {
            let s = read_section(file, at, file_len, l.sections.len())?;
            let end = s
                .data_offset
                .checked_add(s.binary_size)
                .ok_or_else(|| Error::corrupt(FORMAT_ID, "section size overflows"))?;
            if end > file_len {
                l.problems.push((
                    "truncated",
                    format!(
                        "section {}: {} bytes of data at offset {} run past the end of the file ({file_len})",
                        l.sections.len(),
                        s.binary_size,
                        s.data_offset
                    ),
                ));
                l.sections.push(s);
                break;
            }
            l.sections.push(s);
            // The next section, if any, starts after this one's data and its closing marker.
            match next_marker(file, end, file_len)? {
                Some(next) => at = next,
                None => break,
            }
        }
        Ok(l)
    }

    fn parse_cif(&mut self, text: &str) {
        let toks = tokenize(text);
        let mut i = 0;
        while i < toks.len() {
            let t = &toks[i];
            if let Some(name) = t.strip_prefix("data_") {
                if self.block.is_none() {
                    self.block = Some(name.to_string());
                }
                i += 1;
            } else if t.eq_ignore_ascii_case("loop_") {
                i += 1;
                let mut tags = Vec::new();
                while i < toks.len() && toks[i].starts_with('_') {
                    tags.push(toks[i].to_ascii_lowercase());
                    i += 1;
                }
                let mut rows = Vec::new();
                let mut row = Vec::new();
                while i < toks.len()
                    && !toks[i].starts_with('_')
                    && !toks[i].eq_ignore_ascii_case("loop_")
                    && !toks[i].starts_with("data_")
                {
                    row.push(toks[i].clone());
                    if !tags.is_empty() && row.len() == tags.len() {
                        rows.push(std::mem::take(&mut row));
                    }
                    i += 1;
                }
                if !tags.is_empty() {
                    self.loops.push((tags, rows));
                }
            } else if t.starts_with('_') {
                let tag = t.to_ascii_lowercase();
                let value = toks.get(i + 1).cloned().unwrap_or_default();
                if tag == "_array_data.header_contents" {
                    self.header_lines = value
                        .lines()
                        .map(|l| l.trim().trim_start_matches('#').trim().to_string())
                        .filter(|l| !l.is_empty())
                        .collect();
                } else if tag == "_array_data.header_convention" {
                    self.convention = Some(value.clone());
                }
                self.items.insert(tag, value);
                i += 2;
            } else {
                i += 1;
            }
        }
    }

    /// The value of a CIF item, whether it is a single item or the first row of a loop.
    pub fn item(&self, tag: &str) -> Option<&str> {
        if let Some(v) = self.items.get(tag) {
            return Some(v.as_str());
        }
        self.loops.iter().find_map(|(tags, rows)| {
            let k = tags.iter().position(|t| t == tag)?;
            rows.first()?.get(k).map(String::as_str)
        })
    }

    /// The miniCBF header values our model uses.
    pub fn pilatus(&self) -> PilatusHeader {
        PilatusHeader::parse(&self.header_lines)
    }
}

/// Split CIF text into tokens: whitespace-separated words, quoted strings and `;` text fields;
/// comments dropped.
fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if let Some(first) = line.strip_prefix(';') {
            // A text field runs to the next line that starts with ';'.
            let mut field = first.to_string();
            for l in lines.by_ref() {
                if l.starts_with(';') {
                    break;
                }
                field.push('\n');
                field.push_str(l);
            }
            out.push(field);
            continue;
        }
        let b = line.as_bytes();
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                c if c.is_ascii_whitespace() => i += 1,
                b'#' => break,
                q @ (b'\'' | b'"') => {
                    // A quote ends at the same quote followed by whitespace or the line end.
                    let start = i + 1;
                    let mut j = start;
                    while j < b.len()
                        && !(b[j] == q && b.get(j + 1).is_none_or(u8::is_ascii_whitespace))
                    {
                        j += 1;
                    }
                    out.push(String::from_utf8_lossy(&b[start..j.min(b.len())]).into_owned());
                    i = j + 1;
                }
                _ => {
                    let start = i;
                    while i < b.len() && !b[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    out.push(String::from_utf8_lossy(&b[start..i]).into_owned());
                }
            }
        }
    }
    out
}

/// Parse the MIME header of the section whose marker is at `at`.
fn read_section(file: &SourceFile, at: u64, file_len: u64, index: usize) -> Result<Section> {
    let len = (file_len - at).min(8192);
    let buf = common::read_exact(file, at, len, file_len, FORMAT_ID, "binary section header")?;
    let start = find(&buf, &START_MARKER).ok_or_else(|| {
        Error::corrupt_at(
            FORMAT_ID,
            at,
            format!("section {index}: no start marker (0C 1A 04 D5) after the MIME header"),
        )
    })?;
    let text = String::from_utf8_lossy(&buf[..start]);
    let mut mime = BTreeMap::new();
    let mut last: Option<String> = None;
    for line in text.lines().skip(1).take(MAX_MIME_LINES) {
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with([' ', '\t']) {
            // A continued header line (`Content-Type: ...;\n     conversions=...`).
            if let Some(k) = &last
                && let Some(v) = mime.get_mut(k)
            {
                let v: &mut String = v;
                v.push(' ');
                v.push_str(line.trim());
            }
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim().to_ascii_lowercase();
            mime.insert(k.clone(), v.trim().to_string());
            last = Some(k);
        }
    }
    let ct = mime.get("content-type").cloned().unwrap_or_default();
    let conv = ct
        .split(';')
        .find_map(|p| p.trim().strip_prefix("conversions="))
        .map_or_else(
            || "x-CBF_NONE".into(),
            |c| c.trim().trim_matches('"').to_string(),
        );
    let conversion = match conv.to_ascii_lowercase().as_str() {
        "x-cbf_byte_offset" => Conversion::ByteOffset,
        "x-cbf_none" => Conversion::Uncompressed,
        _ => {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("section {index}: compression {conv}"),
                "only byte-offset (x-CBF_BYTE_OFFSET) and uncompressed sections are decoded; convert the file with CBFlib's `convert_image` or another tool to byte-offset first",
            ));
        }
    };
    let num = |k: &str| -> Option<u64> { mime.get(k).and_then(|v| v.trim().parse().ok()) };
    let binary_size = num("x-binary-size").ok_or_else(|| {
        Error::corrupt_at(FORMAT_ID, at, format!("section {index}: no X-Binary-Size"))
    })?;
    let ty = mime.get("x-binary-element-type").map_or_else(
        || "signed 32-bit integer".into(),
        |t| t.trim_matches('"').to_ascii_lowercase(),
    );
    let element = match ty.as_str() {
        "signed 8-bit integer" => PixelType::Int8,
        "unsigned 8-bit integer" => PixelType::Uint8,
        "signed 16-bit integer" => PixelType::Int16,
        "unsigned 16-bit integer" => PixelType::Uint16,
        "signed 32-bit integer" => PixelType::Int32,
        "unsigned 32-bit integer" => PixelType::Uint32,
        "signed 32-bit real ieee" | "32-bit ieee floating point" => PixelType::Float,
        other => {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("section {index}: element type {other}"),
                "8-, 16- and 32-bit integers and 32-bit floats are read",
            ));
        }
    };
    if element == PixelType::Float && conversion == Conversion::ByteOffset {
        return Err(Error::unsupported(
            FORMAT_ID,
            format!("section {index}: byte-offset coded floating-point samples"),
            "byte-offset compression codes integers; this section's element type is a float",
        ));
    }
    let big_endian = mime
        .get("x-binary-element-byte-order")
        .is_some_and(|v| v.to_ascii_uppercase().contains("BIG"));
    let fast = num("x-binary-size-fastest-dimension");
    let second = num("x-binary-size-second-dimension");
    let third = num("x-binary-size-third-dimension");
    let elements = num("x-binary-number-of-elements").unwrap_or(0);
    let dims = match (fast, second) {
        (Some(f), Some(s)) => [f, s, third.unwrap_or(1).max(1)],
        (Some(f), None) => [f, 1, 1],
        _ => [elements, 1, 1],
    };
    let mut d = [0u32; 3];
    for (o, v) in d.iter_mut().zip(dims) {
        *o = u32::try_from(v).map_err(|_| {
            Error::corrupt_at(
                FORMAT_ID,
                at,
                format!("section {index}: dimension {v} too large"),
            )
        })?;
    }
    let product = u64::from(d[0])
        .checked_mul(u64::from(d[1]))
        .and_then(|v| v.checked_mul(u64::from(d[2])))
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "dimensions overflow"))?;
    if d.contains(&0) || (elements != 0 && product != elements) {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            at,
            format!(
                "section {index}: dimensions {} × {} × {} do not hold its {elements} elements",
                d[0], d[1], d[2]
            ),
        ));
    }
    if conversion == Conversion::ByteOffset && product > binary_size {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            at,
            format!(
                "section {index}: {product} byte-offset values cannot fit in {binary_size} bytes (at least one byte each)"
            ),
        ));
    }
    Ok(Section {
        marker_offset: at,
        data_offset: at + start as u64 + 4,
        binary_size,
        mime: mime.clone(),
        conversion,
        conversion_text: conv,
        element,
        big_endian,
        dims: d,
        elements: product,
        md5: mime.get("content-md5").cloned(),
        padding: num("x-binary-size-padding"),
    })
}

/// The offset of the next section marker at or after `from`, if any.
fn next_marker(file: &SourceFile, from: u64, file_len: u64) -> Result<Option<u64>> {
    let mut at = from;
    let overlap = BINARY_MARKER.len() as u64;
    while at < file_len {
        let len = (file_len - at).min(SCAN_CHUNK);
        let buf = common::read_exact(file, at, len, file_len, FORMAT_ID, "text after a section")?;
        // The section ends with `--CIF-BINARY-FORMAT-SECTION----`; skip that closing marker.
        let mut from_i = 0usize;
        while let Some(p) = find(&buf[from_i..], BINARY_MARKER) {
            let i = from_i + p;
            let after = buf.get(i + BINARY_MARKER.len()..i + BINARY_MARKER.len() + 2);
            if after == Some(b"--") {
                from_i = i + BINARY_MARKER.len() + 2;
                continue;
            }
            return Ok(Some(at + i as u64));
        }
        if len < SCAN_CHUNK {
            return Ok(None);
        }
        at += len - overlap;
    }
    Ok(None)
}

/// Decode a section's coded bytes into little-endian samples of its element type.
#[allow(clippy::many_single_char_names)] // s(ection), n, v, p, k: the coding's own terms
pub fn decode(s: &Section, coded: &[u8]) -> Result<Vec<u8>> {
    let n =
        usize::try_from(s.elements).map_err(|_| Error::corrupt(FORMAT_ID, "too many elements"))?;
    let bps = s.element.bytes_per_sample();
    let out_len = n
        .checked_mul(bps)
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "image size overflows"))?;
    match s.conversion {
        Conversion::Uncompressed => {
            let mut out = coded
                .get(..out_len)
                .ok_or_else(|| {
                    Error::corrupt(
                        FORMAT_ID,
                        format!(
                            "uncompressed section holds {} bytes, {out_len} needed",
                            coded.len()
                        ),
                    )
                })?
                .to_vec();
            if s.big_endian && bps > 1 {
                for c in out.chunks_exact_mut(bps) {
                    c.reverse();
                }
            }
            Ok(out)
        }
        Conversion::ByteOffset => {
            let mut out = Vec::with_capacity(out_len);
            let (lo, hi) = range_of(s.element);
            let mut v: i64 = 0;
            let mut p = 0usize;
            for k in 0..n {
                let d = byte_offset_delta(coded, &mut p).ok_or_else(|| {
                    Error::corrupt(
                        FORMAT_ID,
                        format!(
                            "byte-offset data ends after {k} of {n} values ({} bytes)",
                            coded.len()
                        ),
                    )
                })?;
                v = v
                    .checked_add(d)
                    .ok_or_else(|| Error::corrupt(FORMAT_ID, format!("value {k} overflows")))?;
                if v < lo || v > hi {
                    return Err(Error::corrupt(
                        FORMAT_ID,
                        format!(
                            "value {k} = {v} is outside the range of {}",
                            s.element.ome_name()
                        ),
                    ));
                }
                push_le(&mut out, v, bps);
            }
            Ok(out)
        }
    }
}

/// The next byte-offset delta at `*p`: one signed byte; −128 escapes to two bytes, −32768 to
/// four, and the smallest 32-bit value to eight (little-endian).
fn byte_offset_delta(b: &[u8], p: &mut usize) -> Option<i64> {
    let d8 = *b.get(*p)? as i8;
    *p += 1;
    if d8 != i8::MIN {
        return Some(i64::from(d8));
    }
    let d16 = i16::from_le_bytes(b.get(*p..*p + 2)?.try_into().ok()?);
    *p += 2;
    if d16 != i16::MIN {
        return Some(i64::from(d16));
    }
    let d32 = i32::from_le_bytes(b.get(*p..*p + 4)?.try_into().ok()?);
    *p += 4;
    if d32 != i32::MIN {
        return Some(i64::from(d32));
    }
    let d64 = i64::from_le_bytes(b.get(*p..*p + 8)?.try_into().ok()?);
    *p += 8;
    Some(d64)
}

fn range_of(t: PixelType) -> (i64, i64) {
    match t {
        PixelType::Int8 => (i64::from(i8::MIN), i64::from(i8::MAX)),
        PixelType::Uint8 => (0, i64::from(u8::MAX)),
        PixelType::Int16 => (i64::from(i16::MIN), i64::from(i16::MAX)),
        PixelType::Uint16 => (0, i64::from(u16::MAX)),
        PixelType::Uint32 => (0, i64::from(u32::MAX)),
        _ => (i64::from(i32::MIN), i64::from(i32::MAX)),
    }
}

fn push_le(out: &mut Vec<u8>, v: i64, bps: usize) {
    let b = v.to_le_bytes();
    out.extend_from_slice(&b[..bps]);
}

/// Byte-offset code `values` (for tests and fuzz seeds).
pub fn encode_byte_offset(values: &[i64]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut prev = 0i64;
    for &v in values {
        let d = v - prev;
        if let Ok(x) = i8::try_from(d)
            && x != i8::MIN
        {
            out.push(x as u8);
        } else {
            out.push(0x80);
            if let Ok(x) = i16::try_from(d)
                && x != i16::MIN
            {
                out.extend_from_slice(&x.to_le_bytes());
            } else {
                out.extend_from_slice(&i16::MIN.to_le_bytes());
                if let Ok(x) = i32::try_from(d)
                    && x != i32::MIN
                {
                    out.extend_from_slice(&x.to_le_bytes());
                } else {
                    out.extend_from_slice(&i32::MIN.to_le_bytes());
                    out.extend_from_slice(&d.to_le_bytes());
                }
            }
        }
        prev = v;
    }
    out
}

/// The values of a PILATUS 1.2 style miniCBF header that the model uses (each as written in
/// SI units in the file, converted here).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PilatusHeader {
    /// `Detector:` name, without the serial number.
    pub detector: Option<String>,
    /// The serial number after `S/N`.
    pub serial: Option<String>,
    /// The time stamp line, as ISO 8601 when it could be read.
    pub timestamp: Option<String>,
    /// Pixel size in mm `[x, y]`.
    pub pixel_size_mm: Option<[f64; 2]>,
    /// Sensor material and thickness in mm.
    pub sensor: Option<String>,
    pub sensor_thickness_mm: Option<f64>,
    /// Seconds.
    pub exposure_time_s: Option<f64>,
    pub exposure_period_s: Option<f64>,
    /// Counts at which a pixel is overloaded.
    pub count_cutoff: Option<f64>,
    /// Energy threshold in eV.
    pub threshold_ev: Option<f64>,
    /// Å.
    pub wavelength_a: Option<f64>,
    /// mm.
    pub distance_mm: Option<f64>,
    /// Pixels, `[fast, slow]`.
    pub beam_xy_px: Option<[f64; 2]>,
    /// Degrees.
    pub start_angle_deg: Option<f64>,
    pub angle_increment_deg: Option<f64>,
    pub two_theta_deg: Option<f64>,
    /// `Oscillation_axis` as written.
    pub oscillation_axis: Option<String>,
    /// Goniometer angles (`Kappa`, `Phi`, `Chi`, `Omega`) in degrees.
    pub goniometer: BTreeMap<String, f64>,
    /// `Flux` (photons/s) and `Filter_transmission`.
    pub flux: Option<f64>,
    pub transmission: Option<f64>,
}

impl PilatusHeader {
    /// Read the `# key value` lines.
    pub fn parse(lines: &[String]) -> PilatusHeader {
        let mut h = PilatusHeader::default();
        let metres_to_mm = |s: &str| number(first_word(s)).map(|m| common::tidy(m * 1e3));
        for line in lines {
            let (key, rest) = split_key(line);
            match key.to_ascii_lowercase().as_str() {
                "detector" => {
                    let (name, serial) = match rest.split_once(", S/N") {
                        Some((n, s)) => (n.trim(), Some(s.trim().to_string())),
                        None => (rest.trim(), None),
                    };
                    h.detector = Some(name.to_string()).filter(|s| !s.is_empty());
                    h.serial = serial;
                }
                "pixel_size" => {
                    let parts: Vec<f64> = rest
                        .split('x')
                        .filter_map(|p| number(first_word(p.trim())))
                        .collect();
                    if let [x, y] = parts[..] {
                        h.pixel_size_mm = Some([common::tidy(x * 1e3), common::tidy(y * 1e3)]);
                    }
                }
                "exposure_time" => h.exposure_time_s = number(first_word(rest)),
                "exposure_period" => h.exposure_period_s = number(first_word(rest)),
                "count_cutoff" => h.count_cutoff = number(first_word(rest)),
                "threshold_setting" => h.threshold_ev = number(first_word(rest)),
                "wavelength" => h.wavelength_a = number(first_word(rest)),
                "detector_distance" => h.distance_mm = metres_to_mm(rest),
                "beam_xy" => {
                    let inner = rest
                        .split(')')
                        .next()
                        .unwrap_or("")
                        .trim_start_matches(['(', ' ']);
                    let v: Vec<f64> = inner.split(',').filter_map(number).collect();
                    if let [x, y] = v[..] {
                        h.beam_xy_px = Some([x, y]);
                    }
                }
                "start_angle" => h.start_angle_deg = number(first_word(rest)),
                "angle_increment" => h.angle_increment_deg = number(first_word(rest)),
                "detector_2theta" => h.two_theta_deg = number(first_word(rest)),
                "oscillation_axis" => h.oscillation_axis = Some(rest.trim().to_string()),
                "flux" => h.flux = number(first_word(rest)),
                "filter_transmission" => h.transmission = number(first_word(rest)),
                k @ ("kappa" | "phi" | "chi" | "omega") => {
                    if let Some(v) = number(first_word(rest)) {
                        h.goniometer.insert(k.to_string(), v);
                    }
                }
                _ => {
                    if let Some((material, t)) = line.split_once("sensor, thickness") {
                        h.sensor = Some(material.trim().to_string());
                        h.sensor_thickness_mm = metres_to_mm(t);
                    } else if h.timestamp.is_none()
                        && line.chars().next().is_some_and(|c| c.is_ascii_digit())
                    {
                        h.timestamp = Some(iso_time(line.trim()));
                    }
                }
            }
        }
        h
    }
}

fn first_word(s: &str) -> &str {
    s.split_whitespace().next().unwrap_or("")
}

/// `Key: rest`, `Key = rest` or `Key rest`.
fn split_key(line: &str) -> (&str, &str) {
    let line = line.trim();
    let end = line
        .find(|c: char| c.is_whitespace() || c == ':' || c == '=')
        .unwrap_or(line.len());
    let key = &line[..end];
    let rest = line[end..].trim_start_matches([':', '=', ' ', '\t']);
    (key, rest)
}

/// A PILATUS time stamp as ISO 8601: `2013-02-08T12:03:12.789` stays, `2009/Mar/12 13:06:50.123`
/// becomes `2009-03-12T13:06:50.123`; anything else is returned as written.
fn iso_time(s: &str) -> String {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    if let Some((date, time)) = s.split_once(' ')
        && let [y, m, d] = date.split('/').collect::<Vec<_>>()[..]
        && let Some(mi) = MONTHS
            .iter()
            .position(|x| m.to_ascii_lowercase().starts_with(x))
    {
        return format!("{y}-{:02}-{d:0>2}T{}", mi + 1, time.trim());
    }
    s.to_string()
}

/// An opened CBF file.
#[derive(Debug)]
pub struct CbfDataset {
    path: PathBuf,
    file: SourceFile,
    layout: CbfLayout,
    gzip: Option<(u64, Option<String>)>,
}

impl CbfDataset {
    /// Open a CBF file (plain or gzip-compressed) and parse its text and section headers.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let o = common::open(input, FORMAT_ID)?;
        let layout = CbfLayout::read(&o.file, o.len)?;
        Ok(CbfDataset {
            path: input.path().to_path_buf(),
            file: o.file,
            layout,
            gzip: o.gzip,
        })
    }

    /// The parsed structure.
    pub fn layout(&self) -> &CbfLayout {
        &self.layout
    }

    fn geometry(&self) -> Geometry {
        let l = &self.layout;
        let h = l.pilatus();
        let wavelength_a = h.wavelength_a.or_else(|| {
            l.item("_diffrn_radiation_wavelength.wavelength")
                .and_then(number)
        });
        let negative_is_masked = h.detector.as_deref().is_some_and(|d| {
            let d = d.to_ascii_uppercase();
            d.contains("PILATUS") || d.contains("EIGER")
        });
        Geometry {
            radiation: common::radiation_of(wavelength_a),
            wavelength_a,
            distance_mm: h.distance_mm,
            beam_center_px: h.beam_xy_px,
            pixel_size_mm: h.pixel_size_mm,
            two_theta_deg: h.two_theta_deg.filter(|t| *t != 0.0),
            scan_axis: h
                .oscillation_axis
                .as_deref()
                .and_then(|a| a.split([',', ' ']).next())
                .map(str::to_ascii_lowercase)
                .filter(|a| !a.is_empty()),
            oscillation_start_deg: h.start_angle_deg,
            oscillation_width_deg: h.angle_increment_deg,
            exposure_s: h.exposure_time_s,
            overload: h.count_cutoff,
            masked_values: Vec::new(),
            negative_is_masked,
        }
    }

    fn image_info(&self, index: u32, s: &Section) -> ImageInfo {
        let l = &self.layout;
        let h = l.pilatus();
        let mut im = ImageInfo::new(index, s.dims[0], s.dims[1], s.element);
        im.size_z = s.dims[2];
        im.name = l.block.clone().or_else(|| {
            self.path
                .file_stem()
                .map(|n| openreadout_core::gzip::strip_gz(&n.to_string_lossy()).to_string())
        });
        im.channels = vec![ChannelInfo {
            index: 0,
            ..ChannelInfo::default()
        }];
        im.acquired_at = h.timestamp.clone().filter(|t| t.contains('T'));
        if h.detector.is_some() {
            let maker = h.detector.as_deref().and_then(|d| {
                let u = d.to_ascii_uppercase();
                (u.starts_with("PILATUS") || u.starts_with("EIGER")).then(|| "DECTRIS".to_string())
            });
            im.instrument = Some(InstrumentInfo {
                manufacturer: maker,
                model: h.detector.clone(),
                software: None,
                software_version: None,
                detector: h.detector.clone(),
            });
        }
        self.geometry().store(&mut im);
        let mut cbf = Map::new();
        cbf.insert(
            "compression".into(),
            json!(match s.conversion {
                Conversion::ByteOffset => "byte_offset",
                Conversion::Uncompressed => "none",
            }),
        );
        if let Some(c) = &l.convention {
            cbf.insert("header_convention".into(), json!(c));
        }
        if let Some(v) = &h.serial {
            cbf.insert("detector_serial".into(), json!(v));
        }
        if let Some(v) = &h.sensor {
            cbf.insert("sensor".into(), json!(v));
        }
        if let Some(v) = h.sensor_thickness_mm {
            cbf.insert("sensor_thickness_mm".into(), json!(v));
        }
        if let Some(v) = h.threshold_ev {
            cbf.insert("threshold_ev".into(), json!(v));
        }
        if let Some(v) = h.exposure_period_s {
            cbf.insert("exposure_period_s".into(), json!(v));
        }
        if let Some(v) = h.flux {
            cbf.insert("flux".into(), json!(v));
        }
        if let Some(v) = h.transmission {
            cbf.insert("transmission".into(), json!(v));
        }
        if !h.goniometer.is_empty() {
            cbf.insert("goniometer_deg".into(), json!(h.goniometer));
        }
        if let Some(t) = h.timestamp.as_ref().filter(|t| !t.contains('T')) {
            cbf.insert("timestamp".into(), json!(t));
        }
        im.extra.insert("cbf".into(), Value::Object(cbf));
        im.finish()
    }

    fn read_coded(&mut self, s: &Section) -> Result<Vec<u8>> {
        common::read_exact(
            &self.file,
            s.data_offset,
            s.binary_size,
            self.layout.file_len,
            FORMAT_ID,
            "binary section",
        )
    }
}

impl Dataset for CbfDataset {
    fn info(&self) -> Result<FileInfo> {
        let images: Vec<ImageInfo> = self
            .layout
            .sections
            .iter()
            .enumerate()
            .map(|(i, s)| self.image_info(i as u32, s))
            .collect();
        let mut notes: Vec<String> = self
            .layout
            .problems
            .iter()
            .map(|(_, d)| d.clone())
            .collect();
        if let Some((compressed, problem)) = &self.gzip {
            notes.push(format!(
                "gzip-compressed: {compressed} bytes decompress to {}; offsets in this output refer to the decompressed file",
                self.layout.file_len
            ));
            if let Some(p) = problem {
                notes.push(format!("gzip: {p}"));
            }
        }
        if let Some(g) = images.first().and_then(Geometry::of)
            && g.negative_is_masked
        {
            notes.push("negative counts mark pixels without a measurement (−1 module gaps, −2 bad pixels); previews draw them as no data".into());
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.gzip.as_ref().map_or(self.layout.file_len, |g| g.0),
            format: CbfReader.descriptor(),
            format_version: self.layout.version.clone(),
            plane_count: images.iter().map(|i| i.plane_count).sum(),
            images,
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            notes,
        })
    }

    fn experiment(&self) -> Option<openreadout_core::Experiment> {
        openreadout_core::diffraction::experiment(&self.info().ok()?.images)
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let l = &self.layout;
        let loops: Vec<Value> = l
            .loops
            .iter()
            .map(|(tags, rows)| json!({"tags": tags, "rows": rows.iter().take(64).collect::<Vec<_>>()}))
            .collect();
        Ok(json!({
            "version": l.version,
            "data_block": l.block,
            "header_convention": l.convention,
            "header": l.header_lines,
            "items": l.items,
            "loops": loops,
            "sections": l.sections.iter().map(|s| json!({
                "mime": s.mime,
                "data_offset": s.data_offset,
            })).collect::<Vec<_>>(),
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("images[].size_x", Source::Spec),
            ("images[].size_y", Source::Spec),
            ("images[].size_z", Source::Spec),
            ("images[].pixel_type", Source::Spec),
            ("images[].name", Source::Spec),
            ("images[].acquired_at", Source::PriorArt),
            ("images[].instrument", Source::PriorArt),
            ("images[].extra.cbf", Source::PriorArt),
            ("images[].extra.diffraction.wavelength_a", Source::PriorArt),
            ("images[].extra.diffraction.distance_mm", Source::PriorArt),
            (
                "images[].extra.diffraction.beam_center_px",
                Source::PriorArt,
            ),
            ("images[].extra.diffraction.pixel_size_mm", Source::PriorArt),
            (
                "images[].extra.diffraction.oscillation_start_deg",
                Source::PriorArt,
            ),
            (
                "images[].extra.diffraction.oscillation_width_deg",
                Source::PriorArt,
            ),
            ("images[].extra.diffraction.exposure_s", Source::PriorArt),
            ("images[].extra.diffraction.overload", Source::PriorArt),
            (
                "images[].extra.diffraction.negative_is_masked",
                Source::PriorArt,
            ),
            ("images[].extra.diffraction.radiation", Source::Inferred),
            ("images[].extra.diffraction.scan_axis", Source::Inferred),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let l = &self.layout;
        let mut out = vec![LsEntry {
            kind: "metadata".into(),
            name: "cif-header".into(),
            offset: Some(0),
            size: l.sections.first().map(|s| s.marker_offset),
            image: None,
            details: json!({"data_block": l.block, "header_convention": l.convention, "header_lines": l.header_lines.len(), "items": l.items.len(), "loops": l.loops.len()}),
        }];
        for (i, s) in l.sections.iter().enumerate() {
            out.push(LsEntry {
                kind: "image".into(),
                name: format!("binary section {i}"),
                offset: Some(s.data_offset),
                size: Some(s.binary_size),
                image: Some(i as u32),
                details: json!({
                    "conversion": s.conversion_text,
                    "element_type": s.element.ome_name(),
                    "dims": s.dims,
                    "md5": s.md5,
                }),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, image: u32, idx: PlaneIndex) -> Result<Plane> {
        let s = self
            .layout
            .sections
            .get(image as usize)
            .cloned()
            .ok_or_else(|| {
                Error::Usage(format!(
                    "image index {image} out of range (the file holds {} image(s))",
                    self.layout.sections.len()
                ))
            })?;
        if idx.c != 0 || idx.t != 0 || idx.z >= s.dims[2] {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} out of range (c<1, z<{}, t<1)",
                idx.c, idx.z, idx.t, s.dims[2]
            )));
        }
        let coded = self.read_coded(&s)?;
        let all = decode(&s, &coded)?;
        let bps = s.element.bytes_per_sample();
        let plane = s.dims[0] as usize * s.dims[1] as usize * bps;
        let start = idx.z as usize * plane;
        let data = all
            .get(start..start + plane)
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "plane outside the decoded data"))?
            .to_vec();
        Ok(Plane {
            width: s.dims[0],
            height: s.dims[1],
            pixel_type: s.element,
            samples_per_pixel: 1,
            data,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed("CIF text up to the first binary section parses; every section has a MIME header, a start marker and dimensions that hold its element count");
        r.performed("every section's data lies inside the file");
        r.performed("Content-MD5 of every section's coded bytes");
        r.performed("byte-offset data decodes to exactly the declared number of values, each within the element type");
        r.performed("masked (negative) and overloaded pixels counted");
        for (code, d) in &self.layout.problems {
            r.push(Finding::error(code, d.clone()));
        }
        if let Some((_, Some(p))) = &self.gzip {
            r.push(Finding::error("truncated", format!("gzip: {p}")));
        }
        let g = self.geometry();
        let sections = self.layout.sections.clone();
        for (i, s) in sections.iter().enumerate() {
            let end = s.data_offset.saturating_add(s.binary_size);
            if end > self.layout.file_len {
                continue; // reported as truncated above
            }
            let coded = self.read_coded(s)?;
            if let Some(want) = &s.md5 {
                let got = common::base64(&common::md5(&coded));
                if &got != want {
                    r.push(
                        Finding::error(
                            "md5_mismatch",
                            format!(
                                "section {i}: Content-MD5 is {want} but the data hashes to {got}"
                            ),
                        )
                        .at(s.data_offset),
                    );
                }
            }
            match decode(s, &coded) {
                Err(e) => r.push(
                    Finding::error("bad_data", format!("section {i}: {e}")).at(s.data_offset),
                ),
                Ok(v) => {
                    if s.conversion == Conversion::ByteOffset {
                        // How many coded bytes the values used: the rest must be padding.
                        let mut p = 0usize;
                        for _ in 0..s.elements {
                            if byte_offset_delta(&coded, &mut p).is_none() {
                                break;
                            }
                        }
                        if p < coded.len() {
                            r.push(Finding::warning(
                                "unused_bytes",
                                format!(
                                    "section {i}: {} of {} coded bytes follow the last value",
                                    coded.len() - p,
                                    coded.len()
                                ),
                            ));
                        }
                    }
                    let bps = s.element.bytes_per_sample();
                    let (mut masked, mut over) = (0u64, 0u64);
                    for c in v.chunks_exact(bps) {
                        let x = s.element.sample_f64(c);
                        if g.is_masked(x) {
                            masked += 1;
                        } else if g.is_overloaded(x) {
                            over += 1;
                        }
                    }
                    if masked > 0 {
                        r.push(Finding::info(
                            "masked_pixels",
                            format!("section {i}: {masked} pixels hold no measurement (negative values: module gaps and bad pixels)"),
                        ));
                    }
                    if over > 0 {
                        r.push(Finding::info(
                            "overloads",
                            format!(
                                "section {i}: {over} pixels at or above the count cutoff ({})",
                                g.overload.unwrap_or_default()
                            ),
                        ));
                    }
                }
            }
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_offset_round_trips_every_escape() {
        let values = [
            0i64,
            5,
            -3,
            200,
            -200,
            40_000,
            -40_000,
            3_000_000_000,
            -1,
            i64::from(i32::MIN) + 7,
        ];
        let coded = encode_byte_offset(&values);
        let mut p = 0;
        let mut v = 0i64;
        for want in values {
            v += byte_offset_delta(&coded, &mut p).unwrap();
            assert_eq!(v, want);
        }
        assert_eq!(p, coded.len());
        assert!(byte_offset_delta(&coded, &mut p).is_none());
        // A two-byte escape cut short.
        let mut p = 0;
        assert!(byte_offset_delta(&[0x80, 0x01], &mut p).is_none());
    }

    #[test]
    fn tokenizer_reads_text_fields_quotes_and_loops() {
        let text = "###CBF: VERSION 1.5\ndata_scan_0001\n_array_data.header_convention \"PILATUS_1.2\"\n_array_data.header_contents\n;\n# Detector: PILATUS 6M, S/N 60-0100\n# Wavelength 0.97950 A\n;\nloop_\n_diffrn_radiation_wavelength.id\n_diffrn_radiation_wavelength.wavelength\n WAVELENGTH1 1.2 # comment\n";
        let mut l = CbfLayout::default();
        l.parse_cif(text);
        assert_eq!(l.block.as_deref(), Some("scan_0001"));
        assert_eq!(l.convention.as_deref(), Some("PILATUS_1.2"));
        assert_eq!(l.header_lines.len(), 2);
        assert_eq!(
            l.item("_diffrn_radiation_wavelength.wavelength"),
            Some("1.2")
        );
        let h = l.pilatus();
        assert_eq!(h.detector.as_deref(), Some("PILATUS 6M"));
        assert_eq!(h.serial.as_deref(), Some("60-0100"));
        assert_eq!(h.wavelength_a, Some(0.9795));
    }

    #[test]
    fn pilatus_header_values() {
        let lines: Vec<String> = [
            "Detector: PILATUS 6M-F, S/N 60-0114-F",
            "2013-02-08T12:03:12.789",
            "Pixel_size 172e-6 m x 172e-6 m",
            "Silicon sensor, thickness 0.000320 m",
            "Exposure_time 0.1975000 s",
            "Count_cutoff 495976 counts",
            "Threshold_setting: 6327 eV",
            "Detector_distance 0.19018 m",
            "Beam_xy (1235.34, 1279.08) pixels",
            "Start_angle 0.0000 deg.",
            "Angle_increment 0.2000 deg.",
            "Phi 0.0000 deg.",
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        let h = PilatusHeader::parse(&lines);
        assert_eq!(h.timestamp.as_deref(), Some("2013-02-08T12:03:12.789"));
        assert_eq!(h.pixel_size_mm, Some([0.172, 0.172]));
        assert_eq!(h.sensor.as_deref(), Some("Silicon"));
        assert_eq!(h.sensor_thickness_mm, Some(0.32));
        assert_eq!(h.distance_mm, Some(190.18));
        assert_eq!(h.beam_xy_px, Some([1235.34, 1279.08]));
        assert_eq!(h.count_cutoff, Some(495_976.0));
        assert_eq!(h.threshold_ev, Some(6327.0));
        assert_eq!(h.angle_increment_deg, Some(0.2));
        assert_eq!(h.goniometer.get("phi"), Some(&0.0));
        assert_eq!(
            iso_time("2009/Mar/12 13:06:50.123"),
            "2009-03-12T13:06:50.123"
        );
    }
}
