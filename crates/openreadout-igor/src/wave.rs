//! One Igor binary wave: the binary header, the wave header and the optional sections after the
//! data (formula, note, extended units, dimension labels, string indices), for every version
//! (1, 2, 3, 5). A wave is the whole of an `.ibw` file or the data of a wave record in a packed
//! experiment; [`parse_wave`] reads it from any offset of a [`SourceFile`]. Layout and
//! vocabulary: `docs/formats/igor.md`.
//!
//! Every size read from the file is checked against the bytes the wave may occupy before it is
//! used; a section that runs past them becomes a [`Problem`], not a panic.

use openreadout_core::bytes::{Endian, until_nul};
use openreadout_core::pixel::PixelType;
use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};

use crate::FORMAT_ID;

/// Bytes of a version-5 binary header.
pub const BIN_HEADER5: u64 = 64;
/// Bytes of a version-5 wave header without its data field.
pub const WAVE_HEADER5: u64 = 320;
/// Bytes of a version-1–3 wave header without its data field.
pub const WAVE_HEADER2: u64 = 110;
/// Padding after the data of version-1–3 waves.
pub const PADDING2: u64 = 16;
/// Notes, formulas and units longer than this are cut (with a problem).
pub const MAX_TEXT: u64 = 4 << 20;
/// Dimension-label bytes read at most per dimension.
pub const MAX_LABEL_BYTES: u64 = 8 << 20;
/// Bytes of one dimension-label field.
pub const LABEL_FIELD: usize = 32;

/// Sample type of a numeric wave (the wave-type bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberType {
    /// 32-bit float.
    Float32,
    /// 64-bit float.
    Float64,
    /// 8-bit integer.
    Int8,
    /// 16-bit integer.
    Int16,
    /// 32-bit integer.
    Int32,
    /// 64-bit integer.
    Int64,
}

impl NumberType {
    /// Bytes of one (real) value.
    pub fn bytes(self) -> u64 {
        match self {
            NumberType::Int8 => 1,
            NumberType::Int16 => 2,
            NumberType::Float32 | NumberType::Int32 => 4,
            NumberType::Float64 | NumberType::Int64 => 8,
        }
    }
}

/// What a wave holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaveKind {
    /// Text: concatenated bytes with cumulative end offsets in the string indices.
    Text,
    /// Numbers.
    Numeric {
        /// Sample type.
        number: NumberType,
        /// Unsigned integers.
        unsigned: bool,
        /// Real and imaginary part per value.
        complex: bool,
    },
}

impl WaveKind {
    /// Decode the wave-type field; `None` for bit patterns that name no type.
    pub fn from_type(t: i16) -> Option<WaveKind> {
        if t == 0 {
            return Some(WaveKind::Text);
        }
        let t = t as u16;
        let complex = t & 0x01 != 0;
        let unsigned = t & 0x40 != 0;
        let number = match t & !0x41 {
            0x02 => NumberType::Float32,
            0x04 => NumberType::Float64,
            0x08 => NumberType::Int8,
            0x10 => NumberType::Int16,
            0x20 => NumberType::Int32,
            0x80 => NumberType::Int64,
            _ => return None,
        };
        if unsigned && matches!(number, NumberType::Float32 | NumberType::Float64) {
            return None;
        }
        Some(WaveKind::Numeric {
            number,
            unsigned,
            complex,
        })
    }

    /// Bytes of one element (both parts of a complex value).
    pub fn element_bytes(self) -> u64 {
        match self {
            WaveKind::Text => 1,
            WaveKind::Numeric {
                number, complex, ..
            } => number.bytes() * if complex { 2 } else { 1 },
        }
    }

    /// The pixel type of an image of this wave; `None` for text and complex integers.
    pub fn pixel_type(self) -> Option<PixelType> {
        let WaveKind::Numeric {
            number,
            unsigned,
            complex,
        } = self
        else {
            return None;
        };
        Some(match (number, unsigned, complex) {
            (NumberType::Float32, _, false) => PixelType::Float,
            (NumberType::Float64, _, false) => PixelType::Double,
            (NumberType::Float32, _, true) => PixelType::ComplexFloat,
            (NumberType::Float64, _, true) => PixelType::ComplexDouble,
            (_, _, true) => return None,
            (NumberType::Int8, false, _) => PixelType::Int8,
            (NumberType::Int8, true, _) => PixelType::Uint8,
            (NumberType::Int16, false, _) => PixelType::Int16,
            (NumberType::Int16, true, _) => PixelType::Uint16,
            (NumberType::Int32, false, _) => PixelType::Int32,
            (NumberType::Int32, true, _) => PixelType::Uint32,
            (NumberType::Int64, false, _) => PixelType::Int64,
            (NumberType::Int64, true, _) => PixelType::Uint64,
        })
    }

    /// NumPy-style dtype of one stored (real) value.
    pub fn dtype(self) -> &'static str {
        match self {
            WaveKind::Text => "text",
            WaveKind::Numeric {
                number, unsigned, ..
            } => match (number, unsigned) {
                (NumberType::Float32, _) => "float32",
                (NumberType::Float64, _) => "float64",
                (NumberType::Int8, false) => "int8",
                (NumberType::Int8, true) => "uint8",
                (NumberType::Int16, false) => "int16",
                (NumberType::Int16, true) => "uint16",
                (NumberType::Int32, false) => "int32",
                (NumberType::Int32, true) => "uint32",
                (NumberType::Int64, false) => "int64",
                (NumberType::Int64, true) => "uint64",
            },
        }
    }

    /// Our name of the type, for notes and the assurance profile (`float32`, `complex float64`,
    /// `text`).
    pub fn label(self) -> String {
        match self {
            WaveKind::Numeric { complex: true, .. } => format!("complex {}", self.dtype()),
            _ => self.dtype().to_string(),
        }
    }
}

/// A structural problem found while parsing.
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    /// Stable code (`docs/formats/igor.md` § check).
    pub code: &'static str,
    /// What is wrong.
    pub detail: String,
    /// File offset, when known.
    pub offset: Option<u64>,
    /// True for problems that make values unreadable (errors); false for warnings.
    pub fatal: bool,
}

impl Problem {
    fn error(code: &'static str, detail: impl Into<String>, offset: u64) -> Self {
        Problem {
            code,
            detail: detail.into(),
            offset: Some(offset),
            fatal: true,
        }
    }
    fn warning(code: &'static str, detail: impl Into<String>, offset: u64) -> Self {
        Problem {
            code,
            detail: detail.into(),
            offset: Some(offset),
            fatal: false,
        }
    }
}

/// A parsed wave.
#[derive(Debug, Clone)]
pub struct Wave {
    /// Binary-wave version: 1, 2, 3 or 5.
    pub version: u16,
    /// Byte order of every value.
    pub endian: Endian,
    /// File offset of the binary header.
    pub start: u64,
    /// Bytes the wave occupies (to the end of its last section).
    pub end: u64,
    /// The raw wave-type field.
    pub wave_type: i16,
    /// What the wave holds; `None` when the type field names no known type.
    pub kind: Option<WaveKind>,
    /// Wave name.
    pub name: String,
    /// Number of elements (all dimensions).
    pub points: u64,
    /// Elements per dimension (rows, columns, layers, chunks); 0 = unused.
    pub dims: [u64; 4],
    /// Scaling step per dimension (`value = offset + step × index`).
    pub steps: [f64; 4],
    /// Scaling offset per dimension.
    pub offsets: [f64; 4],
    /// Data units.
    pub data_units: String,
    /// Units per dimension.
    pub dim_units: [String; 4],
    /// Full-scale top and bottom values, when the file marks them valid.
    pub full_scale: Option<(f64, f64)>,
    /// Creation time, seconds since 1904-01-01 (writer's local time); 0 = unknown.
    pub created: u32,
    /// Last modification time, as `created`.
    pub modified: u32,
    /// File offset of the data.
    pub data_offset: u64,
    /// Bytes of data the file holds for the wave.
    pub data_len: u64,
    /// Dependency formula.
    pub formula: Option<String>,
    /// Wave note (lines separated by carriage returns).
    pub note: String,
    /// Dimension labels: per dimension, the label of the dimension, then one per element.
    pub labels: [Vec<String>; 4],
    /// File offset and bytes of the string indices (text waves).
    pub string_indices: Option<(u64, u64)>,
    /// The checksum of the headers sums to zero.
    pub checksum_ok: bool,
    /// Problems found while parsing.
    pub problems: Vec<Problem>,
}

/// Byte order of a binary wave from its first two bytes: version 1–5 stored big-endian starts
/// with a zero byte.
pub fn wave_endian(head: &[u8]) -> Option<(Endian, u16)> {
    let b = head.get(..2)?;
    let (e, v) = if b[0] == 0 {
        (Endian::Big, u16::from_be_bytes([b[0], b[1]]))
    } else {
        (Endian::Little, u16::from_le_bytes([b[0], b[1]]))
    };
    matches!(v, 1 | 2 | 3 | 5).then_some((e, v))
}

/// Bytes of the binary header of `version`.
pub fn bin_header_len(version: u16) -> u64 {
    match version {
        1 => 8,
        2 => 16,
        3 => 20,
        _ => BIN_HEADER5,
    }
}

/// Bytes the checksum covers.
pub fn checksum_len(version: u16) -> u64 {
    match version {
        5 => BIN_HEADER5 + WAVE_HEADER5,
        v => bin_header_len(v) + WAVE_HEADER2 + PADDING2,
    }
}

/// True when the 16-bit sum of `b` in byte order `e` is zero.
pub fn checksum_zero(b: &[u8], e: Endian) -> bool {
    let mut sum: u16 = 0;
    for w in b.chunks_exact(2) {
        let v = match e {
            Endian::Little => u16::from_le_bytes([w[0], w[1]]),
            Endian::Big => u16::from_be_bytes([w[0], w[1]]),
        };
        sum = sum.wrapping_add(v);
    }
    sum == 0
}

/// Text of a header field or section: UTF-8 when it is valid, else Latin-1 (older Igor versions
/// wrote the platform's 8-bit encoding).
pub fn igor_text(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => openreadout_core::bytes::latin1(b),
    }
}

fn field_text(b: &[u8]) -> String {
    igor_text(until_nul(b)).trim().to_string()
}

/// Seconds since 1904-01-01 as `YYYY-MM-DDTHH:MM:SS` (no zone: Igor stores the writer's local
/// time); `None` for 0.
pub fn igor_time(secs: u32) -> Option<String> {
    if secs == 0 {
        return None;
    }
    // 1904-01-01 is 24107 days before 1970-01-01
    let unix = i64::from(secs) - 24_107 * 86_400;
    let iso = openreadout_core::time::unix_to_iso8601(unix, 0);
    Some(iso.get(..19).unwrap_or(&iso).to_string())
}

fn read_at(f: &SourceFile, offset: u64, len: u64) -> Result<Vec<u8>> {
    let n = usize::try_from(len).map_err(|_| Error::Other("read larger than memory".into()))?;
    let mut buf = vec![0u8; n];
    f.read_exact_at(offset, &mut buf).map_err(|e| {
        Error::corrupt_at(FORMAT_ID, offset, format!("cannot read {len} bytes: {e}"))
    })?;
    Ok(buf)
}

/// Parse the wave whose binary header starts at `start`; it may occupy bytes up to `limit` (the
/// file length, or the end of its packed-file record).
pub fn parse_wave(f: &SourceFile, start: u64, limit: u64) -> Result<Wave> {
    let avail = limit.saturating_sub(start);
    let head2 = read_at(f, start, avail.min(2))?;
    let Some((endian, version)) = wave_endian(&head2) else {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            start,
            "not an Igor binary wave: the version (first two bytes) is not 1, 2, 3 or 5",
        ));
    };
    let cover = checksum_len(version);
    let header_len = bin_header_len(version)
        + if version == 5 {
            WAVE_HEADER5
        } else {
            WAVE_HEADER2
        };
    if avail < header_len {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            start,
            format!(
                "the version-{version} wave headers need {header_len} bytes; {avail} are left"
            ),
        ));
    }
    let h = read_at(f, start, header_len.max(cover.min(avail)))?;
    let e = endian;
    let i16_ = |o: usize| e.i16(&h, o).unwrap_or(0);
    let i32_ = |o: usize| e.i32(&h, o).unwrap_or(0);
    let u32_ = |o: usize| e.u32(&h, o).unwrap_or(0);
    let f64_ = |o: usize| e.f64(&h, o).unwrap_or(f64::NAN);
    let mut problems = Vec::new();
    let checksum_ok = h
        .get(..usize::try_from(cover).unwrap_or(usize::MAX))
        .is_some_and(|b| checksum_zero(b, e));
    if !checksum_ok {
        problems.push(Problem::warning(
            "checksum",
            format!("the first {cover} bytes of the wave do not sum to zero as 16-bit integers"),
            start,
        ));
    }
    // binary header: section sizes
    let wfm_size;
    let mut note_size = 0i64;
    let mut formula_size = 0i64;
    let mut eunits_size = 0i64;
    let mut dim_eunits = [0i64; 4];
    let mut dim_labels = [0i64; 4];
    let mut sindices_size = 0i64;
    match version {
        1 => wfm_size = i64::from(i32_(2)),
        2 => {
            wfm_size = i64::from(i32_(2));
            note_size = i64::from(i32_(6));
        }
        3 => {
            wfm_size = i64::from(i32_(2));
            note_size = i64::from(i32_(6));
            formula_size = i64::from(i32_(10));
        }
        _ => {
            wfm_size = i64::from(i32_(4));
            formula_size = i64::from(i32_(8));
            note_size = i64::from(i32_(12));
            eunits_size = i64::from(i32_(16));
            for d in 0..4 {
                dim_eunits[d] = i64::from(i32_(20 + 4 * d));
                dim_labels[d] = i64::from(i32_(36 + 4 * d));
            }
            sindices_size = i64::from(i32_(52));
        }
    }
    let w = bin_header_len(version) as usize;
    let wave_type;
    let name;
    let points;
    let mut dims = [0u64; 4];
    let mut steps = [1.0f64; 4];
    let mut offsets = [0.0f64; 4];
    let mut data_units;
    let mut dim_units: [String; 4] = Default::default();
    let full_scale;
    let created;
    let modified;
    let data_offset;
    let data_len_declared;
    if version == 5 {
        created = u32_(w + 4);
        modified = u32_(w + 8);
        points = i64::from(i32_(w + 12));
        wave_type = i16_(w + 16);
        name = field_text(h.get(w + 28..w + 60).unwrap_or_default());
        for d in 0..4 {
            dims[d] = u64::try_from(i32_(w + 68 + 4 * d)).unwrap_or(0);
            steps[d] = f64_(w + 84 + 8 * d);
            offsets[d] = f64_(w + 116 + 8 * d);
            dim_units[d] = field_text(h.get(w + 152 + 4 * d..w + 156 + 4 * d).unwrap_or_default());
        }
        data_units = field_text(h.get(w + 148..w + 152).unwrap_or_default());
        full_scale = (i16_(w + 172) != 0).then(|| (f64_(w + 176), f64_(w + 184)));
        data_offset = start + BIN_HEADER5 + WAVE_HEADER5;
        data_len_declared = wfm_size - WAVE_HEADER5 as i64;
    } else {
        wave_type = i16_(w);
        name = field_text(h.get(w + 6..w + 26).unwrap_or_default());
        data_units = field_text(h.get(w + 34..w + 38).unwrap_or_default());
        dim_units[0] = field_text(h.get(w + 38..w + 42).unwrap_or_default());
        points = i64::from(i32_(w + 42));
        steps[0] = f64_(w + 48);
        offsets[0] = f64_(w + 56);
        full_scale = (i16_(w + 68) != 0).then(|| (f64_(w + 70), f64_(w + 78)));
        created = if version == 1 { 0 } else { u32_(w + 96) };
        modified = u32_(w + 102);
        dims[0] = u64::try_from(points).unwrap_or(0);
        data_offset = start + bin_header_len(version) + WAVE_HEADER2;
        data_len_declared = wfm_size - (WAVE_HEADER2 + PADDING2) as i64;
    }
    let kind = WaveKind::from_type(wave_type);
    if kind.is_none() {
        problems.push(Problem::error(
            "wave_type",
            format!("wave type {wave_type:#x} names no known sample type"),
            start,
        ));
    }
    let points = u64::try_from(points).unwrap_or_else(|_| {
        problems.push(Problem::error(
            "bad_points",
            format!("negative point count {points}"),
            start,
        ));
        0
    });
    // the data
    let data_len = match u64::try_from(data_len_declared) {
        Ok(n) => n,
        Err(_) => {
            problems.push(Problem::error(
                "bad_size",
                format!("the wave-section size {wfm_size} is smaller than its header"),
                start,
            ));
            0
        }
    };
    let mut pos = data_offset.saturating_add(data_len);
    if version != 5 {
        pos = pos.saturating_add(PADDING2);
    }
    if pos > limit {
        problems.push(Problem::error(
            "truncated",
            format!(
                "the wave data ends at {pos}, past the end of the wave's bytes ({limit}): the file is cut short"
            ),
            data_offset,
        ));
    }
    if let Some(k) = kind
        && k != WaveKind::Text
    {
        let need = points.saturating_mul(k.element_bytes());
        if need != data_len && !(data_len == 0 && formula_size > 0) {
            problems.push(Problem::error(
                "data_size",
                format!(
                    "{points} points of {} need {need} bytes; the wave section holds {data_len}",
                    k.label()
                ),
                data_offset,
            ));
        }
    }
    let product: u64 = dims
        .iter()
        .take_while(|d| **d > 0)
        .try_fold(1u64, |a, d| a.checked_mul(*d))
        .unwrap_or(u64::MAX);
    if version == 5 && points > 0 && product != points {
        problems.push(Problem::error(
            "dims",
            format!("the dimension sizes {dims:?} do not multiply to the point count {points}"),
            start,
        ));
    }
    // optional sections, in file order
    let take = |size: i64, what: &str, pos: &mut u64, problems: &mut Vec<Problem>| {
        let Ok(n) = u64::try_from(size) else {
            problems.push(Problem::error(
                "bad_size",
                format!("negative {what} size {size}"),
                *pos,
            ));
            return None;
        };
        if n == 0 {
            return None;
        }
        let at = *pos;
        *pos = pos.saturating_add(n);
        if *pos > limit {
            problems.push(Problem::error(
                "truncated",
                format!("the {what} ({n} bytes at {at}) runs past the end of the wave's bytes"),
                at,
            ));
            return None;
        }
        Some((at, n))
    };
    let text = |sec: Option<(u64, u64)>,
                    what: &str,
                    problems: &mut Vec<Problem>|
     -> Result<Option<String>> {
        let Some((at, n)) = sec else { return Ok(None) };
        if n > MAX_TEXT {
            problems.push(Problem::warning(
                "long_text",
                format!("the {what} holds {n} bytes; the first {MAX_TEXT} are read"),
                at,
            ));
        }
        let b = read_at(f, at, n.min(MAX_TEXT))?;
        Ok(Some(igor_text(&b)))
    };
    let mut formula = None;
    let mut note = String::new();
    let mut labels: [Vec<String>; 4] = Default::default();
    let mut string_indices = None;
    match version {
        1 => {}
        2 => {
            let s = take(note_size, "note", &mut pos, &mut problems);
            note = text(s, "note", &mut problems)?.unwrap_or_default();
        }
        3 => {
            let s = take(formula_size, "formula", &mut pos, &mut problems);
            formula = text(s, "formula", &mut problems)?;
            let s = take(note_size, "note", &mut pos, &mut problems);
            note = text(s, "note", &mut problems)?.unwrap_or_default();
        }
        _ => {
            let s = take(formula_size, "formula", &mut pos, &mut problems);
            formula = text(s, "formula", &mut problems)?;
            let s = take(note_size, "note", &mut pos, &mut problems);
            note = text(s, "note", &mut problems)?.unwrap_or_default();
            let s = take(eunits_size, "extended data units", &mut pos, &mut problems);
            if let Some(u) = text(s, "extended data units", &mut problems)? {
                data_units = u.trim_end_matches('\0').trim().to_string();
            }
            for d in 0..4 {
                let s = take(dim_eunits[d], "extended dimension units", &mut pos, &mut problems);
                if let Some(u) = text(s, "extended dimension units", &mut problems)? {
                    dim_units[d] = u.trim_end_matches('\0').trim().to_string();
                }
            }
            for d in 0..4 {
                let Some((at, n)) = take(dim_labels[d], "dimension labels", &mut pos, &mut problems)
                else {
                    continue;
                };
                if n % LABEL_FIELD as u64 != 0 {
                    problems.push(Problem::warning(
                        "labels",
                        format!(
                            "dimension {d} labels hold {n} bytes, not a multiple of {LABEL_FIELD}"
                        ),
                        at,
                    ));
                }
                let b = read_at(f, at, n.min(MAX_LABEL_BYTES))?;
                labels[d] = b.chunks_exact(LABEL_FIELD).map(field_text).collect();
            }
            string_indices = take(sindices_size, "string indices", &mut pos, &mut problems);
        }
    }
    Ok(Wave {
        version,
        endian,
        start,
        end: pos.min(limit),
        wave_type,
        kind,
        name,
        points,
        dims,
        steps,
        offsets,
        data_units,
        dim_units,
        full_scale,
        created,
        modified,
        data_offset,
        data_len,
        formula,
        note,
        labels,
        string_indices,
        checksum_ok,
        problems,
    })
}

impl Wave {
    /// Number of dimensions in use (1 for a version-1–3 wave; 0 for an empty wave).
    pub fn rank(&self) -> usize {
        self.dims.iter().take_while(|d| **d > 0).count()
    }

    /// True when the data is readable: known type, sizes consistent, inside the bytes.
    pub fn readable(&self) -> bool {
        self.kind.is_some()
            && !self
                .problems
                .iter()
                .any(|p| p.fatal && matches!(p.code, "truncated" | "data_size" | "dims" | "bad_size" | "wave_type" | "bad_points"))
    }

    /// The element labels of dimension `d` (without the label of the dimension itself), when any
    /// is set.
    pub fn element_labels(&self, d: usize) -> Option<&[String]> {
        let l = self.labels.get(d)?;
        let elems = l.get(1..)?;
        elems.iter().any(|s| !s.is_empty()).then_some(elems)
    }

    /// The label of dimension `d` itself, when set.
    pub fn dimension_label(&self, d: usize) -> Option<&str> {
        self.labels
            .get(d)?
            .first()
            .map(String::as_str)
            .filter(|s| !s.is_empty())
    }

    /// The governing formula was saved without the data.
    pub fn data_omitted(&self) -> bool {
        self.data_len == 0 && self.points > 0 && self.formula.is_some()
    }

    /// Read `count` elements starting at element `first` as stored bytes, converted to
    /// little-endian (each part of a complex value swapped separately).
    pub fn read_elements(&self, f: &SourceFile, first: u64, count: u64) -> Result<Vec<u8>> {
        let kind = self.kind.ok_or_else(|| {
            Error::unsupported(
                FORMAT_ID,
                format!("wave type {:#x}", self.wave_type),
                "The wave's type field names no sample type this reader knows; report the file.",
            )
        })?;
        let eb = kind.element_bytes();
        let at = first
            .checked_mul(eb)
            .and_then(|o| o.checked_add(self.data_offset))
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "element offset overflows"))?;
        let len = count
            .checked_mul(eb)
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "read size overflows"))?;
        if first.saturating_add(count) > self.points || at.saturating_add(len) > self.data_offset + self.data_len {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                at,
                format!(
                    "elements {first}..{} of wave `{}` lie outside its {} bytes of data",
                    first.saturating_add(count),
                    self.name,
                    self.data_len
                ),
            ));
        }
        let mut b = read_at(f, at, len)?;
        if self.endian == Endian::Big
            && let WaveKind::Numeric { number, .. } = kind
        {
            let w = number.bytes() as usize;
            if w > 1 {
                for c in b.chunks_exact_mut(w) {
                    c.reverse();
                }
            }
        }
        Ok(b)
    }
}

/// Little-endian stored values to f64: real parts, or (complex) real and imaginary parts
/// interleaved.
pub fn values_f64(kind: WaveKind, le: &[u8]) -> Vec<f64> {
    let WaveKind::Numeric {
        number, unsigned, ..
    } = kind
    else {
        return Vec::new();
    };
    let w = number.bytes() as usize;
    le.chunks_exact(w)
        .map(|c| match (number, unsigned) {
            (NumberType::Float32, _) => f64::from(f32::from_le_bytes([c[0], c[1], c[2], c[3]])),
            (NumberType::Float64, _) => f64::from_le_bytes(c.try_into().unwrap_or([0; 8])),
            (NumberType::Int8, false) => f64::from(c[0] as i8),
            (NumberType::Int8, true) => f64::from(c[0]),
            (NumberType::Int16, false) => f64::from(i16::from_le_bytes([c[0], c[1]])),
            (NumberType::Int16, true) => f64::from(u16::from_le_bytes([c[0], c[1]])),
            (NumberType::Int32, false) => f64::from(i32::from_le_bytes([c[0], c[1], c[2], c[3]])),
            (NumberType::Int32, true) => f64::from(u32::from_le_bytes([c[0], c[1], c[2], c[3]])),
            (NumberType::Int64, false) => i64::from_le_bytes(c.try_into().unwrap_or([0; 8])) as f64,
            (NumberType::Int64, true) => u64::from_le_bytes(c.try_into().unwrap_or([0; 8])) as f64,
        })
        .collect()
}

/// The elements of a text wave, at most `max`.
pub fn text_values(w: &Wave, f: &SourceFile, max: usize) -> Result<Vec<String>> {
    let Some((at, n)) = w.string_indices else {
        return Ok(Vec::new());
    };
    let count = (n / 4).min(w.points).min(max as u64);
    let idx = read_at(f, at, count * 4)?;
    let mut out = Vec::with_capacity(count as usize);
    let mut prev = 0u64;
    for k in 0..count as usize {
        let end = u64::try_from(w.endian.i32(&idx, 4 * k).unwrap_or(-1)).unwrap_or(u64::MAX);
        if end < prev || end > w.data_len {
            break;
        }
        let b = read_at(f, w.data_offset + prev, end - prev)?;
        out.push(igor_text(&b));
        prev = end;
    }
    Ok(out)
}
