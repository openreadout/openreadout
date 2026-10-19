//! TDMS segments: the lead-in, the metadata (objects, raw-data indexes, properties) and where
//! each channel's values sit in the file. Written from National Instruments' public description
//! of the format ("TDMS File Format Internal Structure", `docs/formats/ni-tdms.md`).
//!
//! The parse reads every segment's lead-in and metadata once and records, per channel, the
//! stretches ("pieces") of the file that hold its values; no value is read here.

use std::collections::BTreeMap;

use openreadout_core::source::SourceFile;

/// Format id used in errors.
pub(crate) const FMT: &str = "ni-tdms";

/// ToC flags.
pub(crate) const TOC_META: u32 = 0x02;
pub(crate) const TOC_NEW_OBJ_LIST: u32 = 0x04;
pub(crate) const TOC_RAW: u32 = 0x08;
pub(crate) const TOC_INTERLEAVED: u32 = 0x20;
pub(crate) const TOC_BIG_ENDIAN: u32 = 0x40;
pub(crate) const TOC_DAQMX: u32 = 0x80;

/// Data type codes.
pub(crate) const T_STRING: u32 = 0x20;
pub(crate) const T_BOOL: u32 = 0x21;
pub(crate) const T_TIMESTAMP: u32 = 0x44;
pub(crate) const T_DAQMX: u32 = 0xFFFF_FFFF;

/// Seconds from 1904-01-01 (the TDMS epoch) to 1970-01-01.
pub(crate) const EPOCH_1904_TO_1970: f64 = 2_082_844_800.0;

/// Most metadata bytes read for one segment.
const MAX_META: u64 = 256 << 20;
/// Most pieces kept for all channels together.
const MAX_PIECES: usize = 1 << 22;

/// The size in bytes of one value of a fixed-size type, or `None` for strings and types the
/// reader does not decode.
pub(crate) fn type_size(t: u32) -> Option<u64> {
    Some(match t {
        1 | 5 | T_BOOL => 1,
        2 | 6 => 2,
        3 | 7 | 9 | 0x19 => 4,
        4 | 8 | 10 | 0x1A => 8,
        T_TIMESTAMP => 16,
        0x08_000C => 8,
        0x10_000D => 16,
        11 | 0x1B => 16,
        _ => return None,
    })
}

/// The name of a data type code, for `info` (`docs/formats/ni-tdms.md` § Data types).
pub(crate) fn type_name(t: u32) -> &'static str {
    match t {
        0 => "void",
        1 => "int8",
        2 => "int16",
        3 => "int32",
        4 => "int64",
        5 => "uint8",
        6 => "uint16",
        7 => "uint32",
        8 => "uint64",
        9 | 0x19 => "float32",
        10 | 0x1A => "float64",
        11 | 0x1B => "extended float",
        T_STRING => "string",
        T_BOOL => "bool",
        T_TIMESTAMP => "timestamp",
        0x4F => "fixed point",
        0x08_000C => "complex64",
        0x10_000D => "complex128",
        T_DAQMX => "DAQmx raw",
        _ => "unknown",
    }
}

/// A property value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Prop {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    /// Seconds since 1970-01-01 UTC.
    Time(f64),
    /// A type the reader keeps by name only.
    Other(&'static str),
}

impl Prop {
    pub(crate) fn as_f64(&self) -> Option<f64> {
        match self {
            Prop::Int(v) => Some(*v as f64),
            Prop::Float(v) | Prop::Time(v) => Some(*v),
            Prop::Bool(b) => Some(f64::from(u8::from(*b))),
            Prop::Str(s) => s.trim().parse().ok(),
            Prop::Other(_) => None,
        }
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Prop::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// One DAQmx format-changing scaler: where a channel's raw value sits in the raw buffers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Scaler {
    pub(crate) daqmx_type: u32,
    pub(crate) buffer: u32,
    pub(crate) byte_offset: u32,
    pub(crate) scale_id: u32,
}

/// A channel's raw-data index in a segment.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Index {
    pub(crate) dtype: u32,
    pub(crate) count: u64,
    /// Strings: bytes of the offsets and the text.
    pub(crate) total_bytes: u64,
    /// DAQmx raw data: the scaler and the widths of the raw buffers.
    pub(crate) daqmx: Option<(Scaler, Vec<u32>)>,
    /// The index used the digital-line scaler (not decoded).
    pub(crate) digital_line: bool,
}

/// Where some of a channel's values sit: `count` values from byte `at`, one every `stride` bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Piece {
    pub(crate) at: u64,
    pub(crate) count: u64,
    pub(crate) stride: u64,
    pub(crate) big_endian: bool,
    /// The type of the stored values (the DAQmx raw type for DAQmx data).
    pub(crate) dtype: u32,
}

/// An object of the file: the file itself (`/`), a group or a channel.
#[derive(Debug, Clone, Default)]
pub(crate) struct Object {
    pub(crate) path: String,
    pub(crate) group: Option<String>,
    pub(crate) channel: Option<String>,
    pub(crate) props: BTreeMap<String, Prop>,
    /// The data type of the channel's first index.
    pub(crate) dtype: Option<u32>,
    pub(crate) pieces: Vec<Piece>,
    pub(crate) count: u64,
    /// String values: (offsets start, value count, text start) per segment chunk.
    pub(crate) strings: Vec<(u64, u64, u64, bool)>,
    pub(crate) daqmx_scaler: Option<Scaler>,
    pub(crate) digital_line: bool,
    /// A data type that changed between segments, or another reason the values are not read.
    pub(crate) problem: Option<String>,
}

/// The whole file, parsed.
#[derive(Debug, Clone, Default)]
pub(crate) struct File {
    pub(crate) objects: Vec<Object>,
    pub(crate) segments: u64,
    pub(crate) versions: Vec<u32>,
    pub(crate) big_endian: bool,
    pub(crate) interleaved: bool,
    pub(crate) daqmx: bool,
    /// Problems that stopped or shortened the parse (truncation, a damaged segment).
    pub(crate) notes: Vec<String>,
    pub(crate) truncated: bool,
}

/// Split an object path (`/'group'/'channel'`, quotes doubled inside names) into its names.
pub(crate) fn split_path(p: &str) -> Vec<String> {
    let mut out = Vec::new();
    let b: Vec<char> = p.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '/' && b.get(i + 1) == Some(&'\'') {
            let mut name = String::new();
            i += 2;
            while i < b.len() {
                if b[i] == '\'' {
                    if b.get(i + 1) == Some(&'\'') {
                        name.push('\'');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                name.push(b[i]);
                i += 1;
            }
            out.push(name);
        } else {
            i += 1;
        }
    }
    out
}

/// A cursor over metadata bytes with the segment's byte order.
struct Meta<'a> {
    b: &'a [u8],
    at: usize,
    big: bool,
}

impl Meta<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self
            .at
            .checked_add(n)
            .filter(|e| *e <= self.b.len())
            .ok_or_else(|| "the metadata ends early".to_string())?;
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }

    fn u32(&mut self) -> Result<u32, String> {
        let s: [u8; 4] = self.take(4)?.try_into().map_err(|_| "short read")?;
        Ok(if self.big {
            u32::from_be_bytes(s)
        } else {
            u32::from_le_bytes(s)
        })
    }

    fn u64(&mut self) -> Result<u64, String> {
        let s: [u8; 8] = self.take(8)?.try_into().map_err(|_| "short read")?;
        Ok(if self.big {
            u64::from_be_bytes(s)
        } else {
            u64::from_le_bytes(s)
        })
    }

    fn string(&mut self) -> Result<String, String> {
        let n = self.u32()? as usize;
        let s = self.take(n)?;
        Ok(String::from_utf8_lossy(s)
            .trim_end_matches('\0')
            .to_string())
    }

    fn value(&mut self, t: u32) -> Result<Prop, String> {
        let big = self.big;
        let fixed = |m: &mut Self, n: usize| -> Result<Vec<u8>, String> {
            let mut v = m.take(n)?.to_vec();
            if big {
                v.reverse();
            }
            Ok(v)
        };
        Ok(match t {
            1 => Prop::Int(i64::from(fixed(self, 1)?[0] as i8)),
            2 => Prop::Int(i64::from(i16::from_le_bytes(
                fixed(self, 2)?.try_into().map_err(|_| "x")?,
            ))),
            3 => Prop::Int(i64::from(i32::from_le_bytes(
                fixed(self, 4)?.try_into().map_err(|_| "x")?,
            ))),
            4 => Prop::Int(i64::from_le_bytes(
                fixed(self, 8)?.try_into().map_err(|_| "x")?,
            )),
            5 => Prop::Int(i64::from(fixed(self, 1)?[0])),
            6 => Prop::Int(i64::from(u16::from_le_bytes(
                fixed(self, 2)?.try_into().map_err(|_| "x")?,
            ))),
            7 => Prop::Int(i64::from(u32::from_le_bytes(
                fixed(self, 4)?.try_into().map_err(|_| "x")?,
            ))),
            8 => {
                let v = u64::from_le_bytes(fixed(self, 8)?.try_into().map_err(|_| "x")?);
                i64::try_from(v).map_or(Prop::Float(v as f64), Prop::Int)
            }
            9 | 0x19 => Prop::Float(f64::from(f32::from_le_bytes(
                fixed(self, 4)?.try_into().map_err(|_| "x")?,
            ))),
            10 | 0x1A => Prop::Float(f64::from_le_bytes(
                fixed(self, 8)?.try_into().map_err(|_| "x")?,
            )),
            T_STRING => Prop::Str(self.string()?),
            T_BOOL => Prop::Bool(self.take(1)?[0] != 0),
            T_TIMESTAMP => {
                let b = self.take(16)?.to_vec();
                Prop::Time(timestamp(&b, big))
            }
            11 | 0x1B => {
                self.take(16)?;
                Prop::Other("extended float")
            }
            0x08_000C => {
                self.take(8)?;
                Prop::Other("complex64")
            }
            0x10_000D => {
                self.take(16)?;
                Prop::Other("complex128")
            }
            other => return Err(format!("property of data type 0x{other:x}")),
        })
    }
}

/// A TDMS timestamp (seconds since 1904 and a 2^-64 fraction) as seconds since 1970.
pub(crate) fn timestamp(b: &[u8], big: bool) -> f64 {
    let (frac, secs) = if big {
        (
            u64::from_be_bytes(b[8..16].try_into().unwrap_or([0; 8])),
            i64::from_be_bytes(b[0..8].try_into().unwrap_or([0; 8])),
        )
    } else {
        (
            u64::from_le_bytes(b[0..8].try_into().unwrap_or([0; 8])),
            i64::from_le_bytes(b[8..16].try_into().unwrap_or([0; 8])),
        )
    };
    (secs as f64 - EPOCH_1904_TO_1970) + frac as f64 / 18_446_744_073_709_551_616.0
}

/// What a segment's metadata says about one object.
#[derive(Debug, Clone)]
enum RawIndex {
    None,
    Same,
    New(Index),
}

fn read_index(m: &mut Meta, len: u32) -> Result<RawIndex, String> {
    match len {
        0xFFFF_FFFF => Ok(RawIndex::None),
        0 => Ok(RawIndex::Same),
        // the bytes 69 12 00 00 (format-changing scaler) and 69 13 00 00 (digital-line scaler)
        0x1269 | 0x1369 | 0x6912_0000 | 0x6913_0000 => {
            let dtype = m.u32()?;
            let _dim = m.u32()?;
            let count = m.u64()?;
            let n = m.u32()? as usize;
            if n > 4096 {
                return Err(format!("{n} DAQmx scalers"));
            }
            let mut scalers = Vec::with_capacity(n);
            for _ in 0..n {
                let daqmx_type = m.u32()?;
                let buffer = m.u32()?;
                let byte_offset = m.u32()?;
                let _sample_format = m.u32()?;
                let scale_id = m.u32()?;
                scalers.push(Scaler {
                    daqmx_type,
                    buffer,
                    byte_offset,
                    scale_id,
                });
            }
            let nw = m.u32()? as usize;
            if nw > 4096 {
                return Err(format!("{nw} DAQmx raw buffer widths"));
            }
            let mut widths = Vec::with_capacity(nw);
            for _ in 0..nw {
                widths.push(m.u32()?);
            }
            let first = scalers.first().copied();
            Ok(RawIndex::New(Index {
                dtype,
                count,
                total_bytes: 0,
                daqmx: first.map(|s| (s, widths)),
                digital_line: len == 0x1369 || len == 0x6913_0000,
            }))
        }
        _ => {
            let dtype = m.u32()?;
            let dim = m.u32()?;
            if dim != 1 {
                return Err(format!("raw data index with array dimension {dim}"));
            }
            let count = m.u64()?;
            let total_bytes = if dtype == T_STRING { m.u64()? } else { 0 };
            Ok(RawIndex::New(Index {
                dtype,
                count,
                total_bytes,
                daqmx: None,
                digital_line: false,
            }))
        }
    }
}

/// Bytes one chunk of `ix` takes in a contiguous or interleaved segment.
fn chunk_bytes(ix: &Index) -> Option<u64> {
    if ix.dtype == T_STRING {
        Some(ix.total_bytes)
    } else {
        type_size(ix.dtype)?.checked_mul(ix.count)
    }
}

/// Parse every segment of a `len`-byte file.
pub(crate) fn parse(f: &SourceFile, len: u64) -> Result<File, String> {
    let mut file = File::default();
    let mut ids: BTreeMap<String, usize> = BTreeMap::new();
    let mut last_index: BTreeMap<usize, Index> = BTreeMap::new();
    // the active object list: object and whether it has data in the segment
    let mut list: Vec<(usize, bool)> = Vec::new();
    let mut pos = 0u64;
    let mut pieces_total = 0usize;
    while pos + 28 <= len {
        let mut lead = [0u8; 28];
        f.read_exact_at(pos, &mut lead)
            .map_err(|e| format!("lead-in at byte {pos}: {e}"))?;
        if &lead[..4] != b"TDSm" {
            if pos == 0 {
                return Err("no TDMS segment tag (TDSm)".into());
            }
            file.notes.push(format!(
                "the segment at byte {pos} has no TDSm tag: the rest of the file is not read"
            ));
            file.truncated = true;
            break;
        }
        let toc = u32::from_le_bytes([lead[4], lead[5], lead[6], lead[7]]);
        let big = toc & TOC_BIG_ENDIAN != 0;
        let rd32 = |b: &[u8]| {
            let a: [u8; 4] = b.try_into().unwrap_or([0; 4]);
            if big {
                u32::from_be_bytes(a)
            } else {
                u32::from_le_bytes(a)
            }
        };
        let rd64 = |b: &[u8]| {
            let a: [u8; 8] = b.try_into().unwrap_or([0; 8]);
            if big {
                u64::from_be_bytes(a)
            } else {
                u64::from_le_bytes(a)
            }
        };
        let version = rd32(&lead[8..12]);
        if version != 4712 && version != 4713 {
            return Err(format!(
                "TDMS version {version} at byte {pos} (4712 and 4713 are defined)"
            ));
        }
        if !file.versions.contains(&version) {
            file.versions.push(version);
        }
        let next_off = rd64(&lead[12..20]);
        let raw_off = rd64(&lead[20..28]);
        let meta_start = pos + 28;
        let mut seg_end = if next_off == u64::MAX {
            file.notes.push(format!(
                "the segment at byte {pos} was not finished (interrupted write): its data runs to the end of the file"
            ));
            len
        } else {
            meta_start.saturating_add(next_off)
        };
        if seg_end > len {
            file.notes.push(format!(
                "the segment at byte {pos} runs past the end of the file (truncated): {} of {} bytes are there",
                len - meta_start,
                next_off
            ));
            file.truncated = true;
            seg_end = len;
        }
        let raw_start = meta_start.saturating_add(raw_off);
        if raw_start > seg_end {
            file.notes.push(format!(
                "the metadata of the segment at byte {pos} runs past its end: the rest of the file is not read"
            ));
            file.truncated = true;
            break;
        }
        file.segments += 1;
        file.big_endian |= big;
        file.interleaved |= toc & TOC_INTERLEAVED != 0;
        file.daqmx |= toc & TOC_DAQMX != 0;
        if toc & TOC_META != 0 {
            if raw_off > MAX_META {
                return Err(format!("{raw_off} bytes of metadata in one segment"));
            }
            let mut mb = vec![0u8; raw_off as usize];
            f.read_exact_at(meta_start, &mut mb)
                .map_err(|e| format!("metadata at byte {meta_start}: {e}"))?;
            let mut m = Meta { b: &mb, at: 0, big };
            if toc & TOC_NEW_OBJ_LIST != 0 {
                list.clear();
            }
            let n = m.u32()?;
            for _ in 0..n {
                let path = m.string()?;
                let ilen = m.u32()?;
                let ri = read_index(&mut m, ilen)?;
                let id = *ids.entry(path.clone()).or_insert_with(|| {
                    let names = split_path(&path);
                    file.objects.push(Object {
                        path: path.clone(),
                        group: names.first().cloned(),
                        channel: names.get(1).cloned(),
                        ..Object::default()
                    });
                    file.objects.len() - 1
                });
                let has_data = match ri {
                    RawIndex::None => false,
                    RawIndex::Same => last_index.contains_key(&id),
                    RawIndex::New(ix) => {
                        let o = &mut file.objects[id];
                        match o.dtype {
                            None => o.dtype = Some(ix.dtype),
                            Some(t) if t != ix.dtype => {
                                o.problem.get_or_insert(format!(
                                    "the data type changes from {} to {} between segments",
                                    type_name(t),
                                    type_name(ix.dtype)
                                ));
                            }
                            _ => {}
                        }
                        o.digital_line |= ix.digital_line;
                        if let Some((s, _)) = &ix.daqmx {
                            o.daqmx_scaler.get_or_insert(*s);
                        }
                        last_index.insert(id, ix);
                        true
                    }
                };
                let np = m.u32()?;
                for _ in 0..np {
                    let name = m.string()?;
                    let t = m.u32()?;
                    let v = m.value(t).map_err(|e| {
                        format!(
                            "segment at byte {pos}, object {}, property {name}: {e}",
                            file.objects[id].path
                        )
                    })?;
                    file.objects[id].props.insert(name, v);
                }
                match list.iter_mut().find(|e| e.0 == id) {
                    Some(e) => e.1 = has_data,
                    None => list.push((id, has_data)),
                }
            }
        }
        if toc & TOC_RAW != 0 && raw_start < seg_end {
            pieces_total += place(
                &mut file,
                &list,
                &last_index,
                raw_start,
                seg_end,
                toc,
                big,
                pos,
            )?;
            if pieces_total > MAX_PIECES {
                return Err(format!(
                    "more than {MAX_PIECES} separate stretches of channel data"
                ));
            }
        }
        if seg_end <= pos {
            break;
        }
        pos = seg_end;
    }
    if pos < len && pos + 28 > len && !file.truncated && len - pos > 0 {
        file.notes.push(format!(
            "{} bytes after the last segment are not a segment",
            len - pos
        ));
    }
    Ok(file)
}

/// Record where each channel's values of one segment sit; returns the pieces added.
#[allow(clippy::too_many_arguments)]
fn place(
    file: &mut File,
    list: &[(usize, bool)],
    last_index: &BTreeMap<usize, Index>,
    raw_start: u64,
    seg_end: u64,
    toc: u32,
    big: bool,
    seg_pos: u64,
) -> Result<usize, String> {
    let data: Vec<(usize, &Index)> = list
        .iter()
        .filter(|e| e.1)
        .filter_map(|e| last_index.get(&e.0).map(|ix| (e.0, ix)))
        .filter(|(_, ix)| ix.count > 0)
        .collect();
    if data.is_empty() {
        return Ok(0);
    }
    let total = seg_end - raw_start;
    let mut added = 0;
    let mut push = |file: &mut File, id: usize, p: Piece| {
        let o = &mut file.objects[id];
        o.count = o.count.saturating_add(p.count);
        if let Some(last) = o.pieces.last_mut()
            && last.stride == p.stride
            && last.big_endian == p.big_endian
            && last.dtype == p.dtype
            && last.at.checked_add(last.count.saturating_mul(last.stride)) == Some(p.at)
        {
            last.count += p.count;
            return;
        }
        o.pieces.push(p);
        added += 1;
    };
    // DAQmx raw data: values sit in raw buffers, one value of every channel per row
    if data.iter().any(|(_, ix)| ix.daqmx.is_some()) {
        let n = data[0].1.count;
        let widths = data
            .iter()
            .find_map(|(_, ix)| ix.daqmx.as_ref().map(|d| d.1.clone()))
            .unwrap_or_default();
        let row: u64 = widths.iter().map(|w| u64::from(*w)).sum();
        let chunk = row.checked_mul(n).filter(|c| *c > 0).ok_or_else(|| {
            format!("segment at byte {seg_pos}: DAQmx data with no raw buffer width")
        })?;
        let chunks = total / chunk;
        if chunks > MAX_PIECES as u64 {
            return Err(format!(
                "segment at byte {seg_pos}: {chunks} chunks of DAQmx data"
            ));
        }
        let rows_left = (total % chunk) / row.max(1);
        for k in 0..=chunks {
            let rows = if k < chunks { n } else { rows_left };
            if rows == 0 {
                continue;
            }
            let base = raw_start + k * chunk;
            for (id, ix) in &data {
                let Some((s, w)) = &ix.daqmx else {
                    file.objects[*id].problem.get_or_insert(
                        "a channel without DAQmx raw data in a DAQmx segment".into(),
                    );
                    continue;
                };
                if ix.count != n {
                    file.objects[*id].problem.get_or_insert(
                        "DAQmx channels of one segment hold different value counts".into(),
                    );
                    continue;
                }
                let b = s.buffer as usize;
                let before: u64 = w.iter().take(b).map(|x| u64::from(*x)).sum();
                let width = u64::from(*w.get(b).ok_or_else(|| {
                    format!("segment at byte {seg_pos}: DAQmx scaler names raw buffer {b}, which does not exist")
                })?);
                let at = before
                    .checked_mul(n)
                    .and_then(|x| x.checked_add(base))
                    .and_then(|x| x.checked_add(u64::from(s.byte_offset)))
                    .ok_or_else(|| format!("segment at byte {seg_pos}: DAQmx offsets overflow"))?;
                push(
                    file,
                    *id,
                    Piece {
                        at,
                        count: rows,
                        stride: width,
                        big_endian: big,
                        dtype: s.daqmx_type | 0x8000_0000,
                    },
                );
            }
        }
        return Ok(added);
    }
    let mut sizes = Vec::with_capacity(data.len());
    for (k, (_, ix)) in data.iter().enumerate() {
        if let Some(b) = chunk_bytes(ix) {
            sizes.push(b);
            continue;
        }
        // a value size the reader does not know: this channel and the ones after it in the
        // segment cannot be located
        let why = format!(
            "{} values are not read, so the data after them in the segment at byte {seg_pos} cannot be located",
            type_name(ix.dtype)
        );
        for (later, _) in &data[k..] {
            file.objects[*later].problem.get_or_insert(why.clone());
        }
        let data = &data[..k];
        if data.is_empty() {
            return Ok(added);
        }
        break;
    }
    let data = &data[..sizes.len()];
    let chunk: u64 = sizes.iter().sum();
    if chunk == 0 {
        return Ok(0);
    }
    let chunks = total / chunk;
    let rem = total % chunk;
    if chunks.saturating_mul(data.len() as u64) > MAX_PIECES as u64 {
        return Err(format!(
            "segment at byte {seg_pos}: {chunks} chunks of values"
        ));
    }
    if toc & TOC_INTERLEAVED != 0 {
        if data.iter().any(|(_, ix)| ix.dtype == T_STRING) {
            return Err(format!(
                "segment at byte {seg_pos}: interleaved string data is not defined"
            ));
        }
        let n = data[0].1.count;
        if data.iter().any(|(_, ix)| ix.count != n) || sizes.len() != data.len() {
            return Err(format!(
                "segment at byte {seg_pos}: interleaved channels with different value counts"
            ));
        }
        let row: u64 = data
            .iter()
            .map(|(_, ix)| type_size(ix.dtype).unwrap_or(0))
            .sum();
        let rows = chunks * n + rem / row.max(1);
        let mut col = 0u64;
        for (id, ix) in data {
            let sz = type_size(ix.dtype).unwrap_or(0);
            push(
                file,
                *id,
                Piece {
                    at: raw_start + col,
                    count: rows,
                    stride: row,
                    big_endian: big,
                    dtype: ix.dtype,
                },
            );
            col += sz;
        }
        return Ok(added);
    }
    for k in 0..=chunks {
        let mut at = raw_start + k * chunk;
        let mut left = if k < chunks { chunk } else { rem };
        if left == 0 {
            continue;
        }
        for ((id, ix), &bytes) in data.iter().zip(&sizes) {
            if left == 0 {
                break;
            }
            if ix.dtype == T_STRING {
                if bytes > left {
                    break;
                }
                let n = ix.count;
                let o = &mut file.objects[*id];
                match n.checked_mul(4).filter(|b| *b <= bytes) {
                    Some(offsets) => {
                        o.strings.push((at, n, at + offsets, big));
                        o.count = o.count.saturating_add(n);
                    }
                    None => {
                        o.problem.get_or_insert(
                            "a string index whose byte size is smaller than its offsets".into(),
                        );
                    }
                }
            } else {
                let sz = type_size(ix.dtype).unwrap_or(1);
                let n = ix.count.min(left / sz);
                push(
                    file,
                    *id,
                    Piece {
                        at,
                        count: n,
                        stride: sz,
                        big_endian: big,
                        dtype: ix.dtype,
                    },
                );
            }
            let used = bytes.min(left);
            at += used;
            left -= used;
        }
    }
    if rem > 0 {
        file.notes.push(format!(
            "the segment at byte {seg_pos} ends inside a chunk of values: the values that are there are read"
        ));
    }
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(split_path("/"), Vec::<String>::new());
        assert_eq!(split_path("/'Group'"), vec!["Group"]);
        assert_eq!(
            split_path("/'Gr''p'/'Ch/1'"),
            vec!["Gr'p".to_string(), "Ch/1".to_string()]
        );
    }

    #[test]
    fn timestamps() {
        // 1904 + 2,082,844,800 s is the Unix epoch; half a second in the fraction
        let mut b = Vec::new();
        b.extend_from_slice(&(1u64 << 63).to_le_bytes());
        b.extend_from_slice(&2_082_844_800i64.to_le_bytes());
        assert!((timestamp(&b, false) - 0.5).abs() < 1e-12);
    }
}
