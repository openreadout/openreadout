//! Shared HDF5 helpers on top of `hdf5-pure`: detection, attribute text, a tree walk, and a
//! plane reader for 3-D (z, y, x) datasets that decodes chunks itself so that filters
//! `hdf5-pure` does not implement (the registered LZ4 filter 32004 that Imaris writes) work,
//! and so that one z plane only touches the chunks that hold it.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;

use hdf5_pure::{AttrValue, DType, Datatype, DatatypeByteOrder, Layout};
use openreadout_core::region::TileCache;
use openreadout_core::source::{ByteSource, Fs, SourceFile};
use openreadout_core::{Error, PixelType, Result};
use serde_json::{Value, json};

/// The 8-byte HDF5 signature.
pub const HDF5_SIGNATURE: [u8; 8] = [0x89, b'H', b'D', b'F', b'\r', b'\n', 0x1a, b'\n'];

/// Registered HDF5 filter ids we decode.
pub const FILTER_DEFLATE: u16 = 1;
pub const FILTER_SHUFFLE: u16 = 2;
pub const FILTER_FLETCHER32: u16 = 3;
pub const FILTER_LZ4: u16 = 32004;

/// Longest attribute text kept in JSON output.
const MAX_ATTR_TEXT: usize = 64 << 10;

/// HDF5 signature at byte 0 or at a user-block boundary (512, 1024, 2048, …) within `head`.
pub fn looks_like_hdf5(head: &[u8]) -> bool {
    let mut at = 0usize;
    loop {
        match head.get(at..at + 8) {
            Some(s) if s == HDF5_SIGNATURE => return true,
            Some(_) => {}
            None => return false,
        }
        at = if at == 0 { 512 } else { at * 2 };
    }
}

/// Open an HDF5 file (streaming, no memory map).
pub fn open_h5(path: &Path, format: &'static str) -> Result<hdf5_pure::File> {
    open_h5_in(&Fs::local(), path, format)
}

/// Open an HDF5 file in the namespace `fs`: local files as before (`hdf5-pure`'s own
/// streaming reader), anything else through [`H5Source`].
pub(crate) fn open_h5_in(fs: &Fs, path: &Path, format: &'static str) -> Result<hdf5_pure::File> {
    let opened = if fs.is_local() {
        hdf5_pure::File::open_streaming(path)
    } else {
        let src = fs.source(path).map_err(|e| Error::io(path, e))?;
        let src = H5Source::new(src).map_err(|e| Error::io(path, e))?;
        hdf5_pure::File::from_source(src)
    };
    opened.map_err(|e| match e {
        hdf5_pure::Error::Io(io) => Error::io(path, io),
        other => Error::corrupt(format, format!("HDF5: {other}")),
    })
}

/// A [`ByteSource`] served to `hdf5-pure` (in-memory buffers, host callbacks).
#[derive(Debug)]
pub(crate) struct H5Source {
    src: Arc<dyn ByteSource>,
    len: u64,
}

impl H5Source {
    /// Wrap `src`; its size is taken once (HDF5 files are not read while growing).
    pub(crate) fn new(src: Arc<dyn ByteSource>) -> std::io::Result<Self> {
        let len = src.size()?;
        Ok(Self { src, len })
    }
}

impl hdf5_pure::Source for H5Source {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(
        &self,
        offset: u64,
        buf: &mut [u8],
    ) -> std::result::Result<(), hdf5_pure::FormatError> {
        let end =
            offset
                .checked_add(buf.len() as u64)
                .ok_or(hdf5_pure::FormatError::OffsetOverflow {
                    offset,
                    length: buf.len() as u64,
                })?;
        if end > self.len {
            return Err(hdf5_pure::FormatError::UnexpectedEof {
                expected: usize::try_from(end).unwrap_or(usize::MAX),
                available: usize::try_from(self.len).unwrap_or(usize::MAX),
            });
        }
        self.src
            .read_exact_at(offset, buf)
            .map_err(|e| hdf5_pure::FormatError::Source(e.to_string()))
    }
}

/// An attribute as text: strings (including Imaris' arrays of one-character strings, which
/// are joined), numbers as their decimal form. NULs are dropped and the text trimmed.
pub fn attr_text(v: &AttrValue) -> Option<String> {
    let s = match v {
        AttrValue::String(s)
        | AttrValue::AsciiString(s)
        | AttrValue::VarLenString(s)
        | AttrValue::VarLenAsciiString(s)
        | AttrValue::StringSized { value: s, .. }
        | AttrValue::AsciiStringSized { value: s, .. } => s.clone(),
        AttrValue::StringArray(a)
        | AttrValue::AsciiStringArray(a)
        | AttrValue::VarLenStringArray(a)
        | AttrValue::VarLenAsciiStringArray(a)
        | AttrValue::VarLenAsciiCharArray(a)
        | AttrValue::StringArraySized { values: a, .. }
        | AttrValue::AsciiStringArraySized { values: a, .. } => {
            if a.iter().all(|x| x.chars().count() <= 1) {
                a.concat()
            } else {
                a.join(", ")
            }
        }
        AttrValue::F64(x) => x.to_string(),
        AttrValue::F32(x) => x.to_string(),
        AttrValue::I64(x) => x.to_string(),
        AttrValue::I32(x) => x.to_string(),
        AttrValue::I16(x) => x.to_string(),
        AttrValue::I8(x) => x.to_string(),
        AttrValue::U64(x) => x.to_string(),
        AttrValue::U32(x) => x.to_string(),
        AttrValue::U16(x) => x.to_string(),
        AttrValue::U8(x) => x.to_string(),
        _ => return None,
    };
    let s: String = s.chars().filter(|&c| c != '\0').collect();
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// An attribute as JSON (numbers stay numbers, strings as [`attr_text`]).
pub fn attr_json(v: &AttrValue) -> Value {
    let cut = |s: String| {
        if s.len() > MAX_ATTR_TEXT {
            let mut end = MAX_ATTR_TEXT;
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            Value::String(format!("{}…", &s[..end]))
        } else {
            Value::String(s)
        }
    };
    let num_list = |v: Vec<Value>| {
        if v.len() > 1000 {
            json!({"values": v[..1000].to_vec(), "truncated": true, "count": v.len()})
        } else {
            Value::Array(v)
        }
    };
    match v {
        AttrValue::F64(x) => json!(x),
        AttrValue::F32(x) => json!(x),
        AttrValue::I64(x) => json!(x),
        AttrValue::I32(x) => json!(x),
        AttrValue::I16(x) => json!(x),
        AttrValue::I8(x) => json!(x),
        AttrValue::U64(x) => json!(x),
        AttrValue::U32(x) => json!(x),
        AttrValue::U16(x) => json!(x),
        AttrValue::U8(x) => json!(x),
        AttrValue::F64Array(a) => num_list(a.iter().map(|x| json!(x)).collect()),
        AttrValue::F32Array(a) => num_list(a.iter().map(|x| json!(x)).collect()),
        AttrValue::I64Array(a) => num_list(a.iter().map(|x| json!(x)).collect()),
        AttrValue::I32Array(a) => num_list(a.iter().map(|x| json!(x)).collect()),
        AttrValue::I16Array(a) => num_list(a.iter().map(|x| json!(x)).collect()),
        AttrValue::I8Array(a) => num_list(a.iter().map(|x| json!(x)).collect()),
        AttrValue::U64Array(a) => num_list(a.iter().map(|x| json!(x)).collect()),
        AttrValue::U32Array(a) => num_list(a.iter().map(|x| json!(x)).collect()),
        AttrValue::U16Array(a) => num_list(a.iter().map(|x| json!(x)).collect()),
        AttrValue::U8Array(a) => num_list(a.iter().map(|x| json!(x)).collect()),
        other => attr_text(other).map_or(Value::Null, cut),
    }
}

/// Attributes of a group or dataset as a sorted JSON object.
pub fn attrs_json<S: std::hash::BuildHasher>(
    attrs: &HashMap<String, AttrValue, S>,
) -> serde_json::Map<String, Value> {
    let sorted: BTreeMap<&String, &AttrValue> = attrs.iter().collect();
    sorted
        .into_iter()
        .map(|(k, v)| (k.clone(), attr_json(v)))
        .collect()
}

/// Attributes as text, keyed by name.
pub fn attrs_text<S: std::hash::BuildHasher>(
    attrs: &HashMap<String, AttrValue, S>,
) -> BTreeMap<String, String> {
    attrs
        .iter()
        .filter_map(|(k, v)| attr_text(v).map(|t| (k.clone(), t)))
        .collect()
}

/// Our pixel type for a numeric HDF5 type.
pub fn pixel_type(d: &DType) -> Option<PixelType> {
    Some(match d {
        DType::U8 => PixelType::Uint8,
        DType::U16 => PixelType::Uint16,
        DType::U32 => PixelType::Uint32,
        DType::I8 => PixelType::Int8,
        DType::I16 => PixelType::Int16,
        DType::I32 => PixelType::Int32,
        DType::F32 => PixelType::Float,
        DType::F64 => PixelType::Double,
        _ => return None,
    })
}

/// NumPy-style name of an HDF5 type (`uint16`, `float32`, `string`, `compound`, ...).
pub fn dtype_name(d: &DType) -> String {
    match d {
        DType::U8 => "uint8",
        DType::U16 => "uint16",
        DType::U32 => "uint32",
        DType::U64 => "uint64",
        DType::I8 => "int8",
        DType::I16 => "int16",
        DType::I32 => "int32",
        DType::I64 => "int64",
        DType::F32 => "float32",
        DType::F64 => "float64",
        DType::String | DType::VariableLengthString => "string",
        DType::Compound(_) => "compound",
        DType::Enum(_) => "enum",
        DType::Array(..) => "array",
        DType::ObjectReference => "reference",
        _ => "other",
    }
    .to_string()
}

/// Stored big-endian (samples are swapped on read).
pub fn big_endian(d: &Datatype) -> bool {
    matches!(
        d,
        Datatype::FixedPoint {
            byte_order: DatatypeByteOrder::BigEndian,
            ..
        } | Datatype::FloatingPoint {
            byte_order: DatatypeByteOrder::BigEndian,
            ..
        }
    )
}

/// Swap every `bps`-byte sample in place.
pub fn swap_samples(data: &mut [u8], bps: usize) {
    if bps > 1 {
        for s in data.chunks_exact_mut(bps) {
            s.reverse();
        }
    }
}

/// One node of an HDF5 tree walk.
#[derive(Debug, Clone, PartialEq)]
pub struct H5Node {
    /// `/`-separated path from the root (the root is `/`).
    pub path: String,
    pub is_group: bool,
    /// Dataset shape (empty for groups and scalars).
    pub shape: Vec<u64>,
    /// Dataset element type (`None` for groups).
    pub dtype: Option<String>,
    pub attributes: serde_json::Map<String, Value>,
}

/// Deepest group level walked. Hard links can make the group graph cyclic (a group linking to
/// itself or to an ancestor), so paths do not form a finite tree; real files nest a few levels.
const MAX_DEPTH: usize = 32;

/// Longest object path walked, in bytes.
const MAX_PATH: usize = 4096;

/// The node limit for walking the file at `path`: `limit`, but no more than the file could
/// hold as distinct objects (an object header takes at least 16 bytes) and at least 1024.
/// Beyond that the walk can only be revisiting hard-linked groups (a cycle), and every visit
/// re-reads headers: 200,000 of them took nearly a minute on an 18 KB file.
pub fn walk_limit(fs: &Fs, path: &Path, limit: usize) -> usize {
    let len = fs.metadata(path).map_or(0, |m| m.len());
    limit.min(usize::try_from(len / 16).unwrap_or(usize::MAX).max(1024))
}

/// Walk the tree breadth-first from `/`, at most `limit` nodes, `MAX_DEPTH` group levels and
/// paths of `MAX_PATH` bytes (the second value says whether the walk stopped early). Each
/// group and dataset is opened from its parent's handle, not by path from the root.
pub fn walk(file: &hdf5_pure::File, limit: usize) -> (Vec<H5Node>, bool) {
    let mut out = Vec::new();
    let mut truncated = false;
    let mut queue = std::collections::VecDeque::from([(String::from("/"), file.root(), 0usize)]);
    while let Some((g, group, depth)) = queue.pop_front() {
        if out.len() >= limit {
            return (out, true);
        }
        out.push(H5Node {
            path: g.clone(),
            is_group: true,
            shape: Vec::new(),
            dtype: None,
            attributes: group.attrs().map(|a| attrs_json(&a)).unwrap_or_default(),
        });
        let prefix = if g == "/" { String::new() } else { g.clone() };
        let mut ds = group.datasets().unwrap_or_default();
        ds.sort();
        for d in ds {
            if out.len() >= limit {
                return (out, true);
            }
            let p = format!("{prefix}/{d}");
            if p.len() > MAX_PATH {
                truncated = true;
                continue;
            }
            let Ok(dset) = group.dataset(&d) else {
                continue;
            };
            out.push(H5Node {
                path: p,
                is_group: false,
                shape: dset.shape().unwrap_or_default(),
                dtype: dset.dtype().ok().map(|t| dtype_name(&t)),
                attributes: dset.attrs().map(|a| attrs_json(&a)).unwrap_or_default(),
            });
        }
        let mut gs = group.groups().unwrap_or_default();
        gs.sort();
        for c in gs {
            let p = format!("{prefix}/{c}");
            if depth + 1 > MAX_DEPTH || p.len() > MAX_PATH || queue.len() >= limit {
                truncated = true;
                continue;
            }
            if let Ok(child) = group.group(&c) {
                queue.push_back((p, child, depth + 1));
            }
        }
    }
    (out, truncated)
}

fn read_at(f: &mut SourceFile, path: &Path, offset: u64, len: u64) -> Result<Vec<u8>> {
    let n = usize::try_from(len).map_err(|_| Error::Other("read too large".into()))?;
    let mut buf = vec![0u8; n];
    f.seek(SeekFrom::Start(offset))
        .map_err(|e| Error::io(path, e))?;
    f.read_exact(&mut buf).map_err(|e| Error::io(path, e))?;
    Ok(buf)
}

/// How a 3-D dataset's bytes are stored.
#[derive(Debug, Clone)]
enum Storage {
    Contiguous {
        address: u64,
    },
    Chunked {
        chunk: [u64; 3],
        /// (chunk offset, file address, stored size, filter mask) of every allocated chunk.
        chunks: Vec<([u64; 3], u64, u64, u32)>,
        /// (filter id, client data) in write order.
        filters: Vec<(u16, Vec<u32>)>,
    },
    /// Compact or unallocated: read through `hdf5-pure`.
    Library,
}

/// A 3-D (z, y, x) dataset read one z plane at a time.
#[derive(Debug, Clone)]
pub struct PlaneDataset {
    pub path: String,
    /// Stored extent (Imaris pads it to whole chunks).
    pub shape: [u64; 3],
    pub pixel_type: PixelType,
    pub big_endian: bool,
    storage: Storage,
}

impl PlaneDataset {
    /// Describe `path` (a rank-3 numeric dataset).
    pub fn open(file: &hdf5_pure::File, path: &str, format: &'static str) -> Result<Self> {
        let err = |e: hdf5_pure::Error| Error::corrupt(format, format!("{path}: {e}"));
        let ds = file.dataset(path).map_err(err)?;
        let shape = ds.shape().map_err(err)?;
        let shape: [u64; 3] = match shape.as_slice() {
            [z, y, x] => [*z, *y, *x],
            [y, x] => [1, *y, *x],
            other => {
                return Err(Error::corrupt(
                    format,
                    format!("{path}: shape {other:?}, expected (z, y, x)"),
                ));
            }
        };
        let dt = ds.dtype().map_err(err)?;
        let pixel_type = pixel_type(&dt).ok_or_else(|| {
            Error::unsupported(
                format,
                format!("{path}: sample type {dt}"),
                "Only 8/16/32-bit integer and 32/64-bit float images are read.",
            )
        })?;
        let big_endian = ds.datatype().is_ok_and(|d| big_endian(&d));
        let storage = match ds.layout().map_err(err)? {
            Layout::Contiguous {
                address: Some(address),
                ..
            } => Storage::Contiguous { address },
            Layout::Chunked { chunk_shape, .. }
                if chunk_shape.len() == ds.shape().map_or(0, |s| s.len()) =>
            {
                match ds.chunks() {
                    Ok(list) => {
                        let chunk = match chunk_shape.as_slice() {
                            [z, y, x] => [*z, *y, *x],
                            [y, x] => [1, *y, *x],
                            _ => [1, 1, 1],
                        };
                        let rank2 = chunk_shape.len() == 2;
                        Storage::Chunked {
                            chunk,
                            chunks: list
                                .into_iter()
                                .map(|c| {
                                    let o = if rank2 {
                                        [
                                            0,
                                            c.offset.first().copied().unwrap_or(0),
                                            c.offset.get(1).copied().unwrap_or(0),
                                        ]
                                    } else {
                                        [
                                            c.offset.first().copied().unwrap_or(0),
                                            c.offset.get(1).copied().unwrap_or(0),
                                            c.offset.get(2).copied().unwrap_or(0),
                                        ]
                                    };
                                    (o, c.address, c.storage_size, c.filter_mask)
                                })
                                .collect(),
                            filters: ds
                                .filter_pipeline()
                                .into_iter()
                                .map(|f| (f.id, f.client_data))
                                .collect(),
                        }
                    }
                    Err(_) => Storage::Library,
                }
            }
            _ => Storage::Library,
        };
        Ok(PlaneDataset {
            path: path.to_string(),
            shape,
            pixel_type,
            big_endian,
            storage,
        })
    }

    /// Filter names (for `info --view structure`).
    pub fn filter_names(&self) -> Vec<String> {
        match &self.storage {
            Storage::Chunked { filters, .. } => filters
                .iter()
                .map(|(id, _)| match *id {
                    FILTER_DEFLATE => "deflate".into(),
                    FILTER_SHUFFLE => "shuffle".into(),
                    FILTER_FLETCHER32 => "fletcher32".into(),
                    FILTER_LZ4 => "lz4".into(),
                    other => format!("filter {other}"),
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Chunk edge lengths, when chunked.
    pub fn chunk_shape(&self) -> Option<[u64; 3]> {
        match &self.storage {
            Storage::Chunked { chunk, .. } => Some(*chunk),
            _ => None,
        }
    }

    /// Number of allocated chunks, when chunked.
    pub fn chunk_count(&self) -> Option<usize> {
        match &self.storage {
            Storage::Chunked { chunks, .. } => Some(chunks.len()),
            _ => None,
        }
    }

    /// Plane `z`, cropped to `width` × `height` (the stored extent may be padded), as
    /// little-endian samples.
    pub fn read_plane(
        &self,
        src: PlaneSource<'_>,
        z: u64,
        width: u64,
        height: u64,
    ) -> Result<Vec<u8>> {
        self.read_plane_region(src, z, (0, 0, width, height), (width, height), None)
    }

    /// The rectangle `(x, y, w, h)` of plane `z` of a `full` = (width, height) image (the stored
    /// extent may be padded beyond it), as little-endian samples. Chunked storage decodes only
    /// the chunks the rectangle overlaps; with a `cache`, decoded chunks (which usually span
    /// several z planes) are kept for the next read.
    pub fn read_plane_region(
        &self,
        src: PlaneSource<'_>,
        z: u64,
        (rx, ry, rw, rh): (u64, u64, u64, u64),
        (width, height): (u64, u64),
        cache: Option<&mut TileCache<u64, Vec<u8>>>,
    ) -> Result<Vec<u8>> {
        let format = src.format;
        let [sz, sy, sx] = self.shape;
        if z >= sz || width > sx || height > sy {
            return Err(Error::corrupt(
                format,
                format!(
                    "{}: plane z={z} {width}x{height} outside the stored extent {sz}x{sy}x{sx}",
                    self.path
                ),
            ));
        }
        if rw == 0 || rh == 0 || rx + rw > width || ry + rh > height {
            return Err(Error::Usage(format!(
                "region {rx},{ry},{rw},{rh} is outside the {width} x {height} plane"
            )));
        }
        let bps = self.pixel_type.bytes_per_sample() as u64;
        let row = usize::try_from(rw * bps).map_err(|_| Error::Other("row too large".into()))?;
        let mut out = vec![0u8; row * rh as usize];
        let region = (rx, ry, rw, rh);
        match &self.storage {
            Storage::Contiguous { address } => {
                let path = src.path;
                let mut f = src.fs.open(path).map_err(|e| Error::io(path, e))?;
                let flen = f.metadata().map_err(|e| Error::io(path, e))?.len();
                let base = address + z * sy * sx * bps;
                let last = base + ((ry + rh - 1) * sx + rx + rw) * bps;
                if last > flen {
                    return Err(Error::corrupt_at(
                        format,
                        base,
                        format!(
                            "{}: plane z={z} lies past the end of the file (truncated)",
                            self.path
                        ),
                    ));
                }
                if rx == 0 && rw == sx {
                    out = read_at(&mut f, path, base + ry * sx * bps, rh * sx * bps)?;
                } else {
                    for y in 0..rh {
                        let r = read_at(&mut f, path, base + ((ry + y) * sx + rx) * bps, rw * bps)?;
                        out[y as usize * row..(y as usize + 1) * row].copy_from_slice(&r);
                    }
                }
            }
            Storage::Chunked { .. } => self.read_chunked(src, z, region, cache, &mut out)?,
            Storage::Library => {
                let ds = src
                    .h5
                    .dataset(&self.path)
                    .map_err(|e| Error::corrupt(format, format!("{}: {e}", self.path)))?;
                let raw = ds
                    .read_raw_rows(z, 1)
                    .map_err(|e| Error::corrupt(format, format!("{}: {e}", self.path)))?;
                let srow = (sx * bps) as usize;
                if raw.len() < srow * (ry + rh) as usize {
                    return Err(Error::corrupt(
                        format,
                        format!("{}: plane z={z} is short", self.path),
                    ));
                }
                let skip = (rx * bps) as usize;
                for y in 0..rh as usize {
                    let s0 = (ry as usize + y) * srow + skip;
                    out[y * row..(y + 1) * row].copy_from_slice(&raw[s0..s0 + row]);
                }
            }
        }
        if self.big_endian {
            swap_samples(&mut out, bps as usize);
        }
        Ok(out)
    }

    /// The chunked-storage part of [`PlaneDataset::read_plane_region`]: decode the chunks that
    /// overlap `(rx, ry, rw, rh)` of plane `z` and copy their samples into `out`.
    fn read_chunked(
        &self,
        src: PlaneSource<'_>,
        z: u64,
        (rx, ry, rw, rh): (u64, u64, u64, u64),
        mut cache: Option<&mut TileCache<u64, Vec<u8>>>,
        out: &mut [u8],
    ) -> Result<()> {
        let Storage::Chunked {
            chunk,
            chunks,
            filters,
        } = &self.storage
        else {
            return Ok(());
        };
        let (format, path) = (src.format, src.path);
        let bps = self.pixel_type.bytes_per_sample() as u64;
        let [cz, cy, cx] = *chunk;
        let chunk_bytes = usize::try_from(cz * cy * cx * bps)
            .map_err(|_| Error::Other("chunk too large".into()))?;
        let mut f = src.fs.open(path).map_err(|e| Error::io(path, e))?;
        let flen = f.metadata().map_err(|e| Error::io(path, e))?.len();
        for (off, addr, size, mask) in chunks {
            if !(off[0] <= z && z < off[0] + cz)
                || off[1] >= ry + rh
                || off[1] + cy <= ry
                || off[2] >= rx + rw
                || off[2] + cx <= rx
            {
                continue;
            }
            let cached = cache.as_mut().and_then(|c| c.get(addr));
            let dec = if let Some(d) = cached {
                d
            } else {
                if addr.checked_add(*size).is_none_or(|e| e > flen) {
                    return Err(Error::corrupt_at(
                        format,
                        *addr,
                        format!(
                            "{}: chunk {off:?} lies past the end of the file (truncated)",
                            self.path
                        ),
                    ));
                }
                let raw = read_at(&mut f, path, *addr, *size)?;
                let dec = unfilter(raw, filters, *mask, bps as usize, format, &self.path)?;
                if dec.len() < chunk_bytes {
                    return Err(Error::corrupt(
                        format,
                        format!(
                            "{}: chunk {off:?} decoded to {} bytes, expected {chunk_bytes}",
                            self.path,
                            dec.len()
                        ),
                    ));
                }
                let d = Arc::new(dec);
                if let Some(c) = cache.as_mut() {
                    c.put(*addr, d.clone(), chunk_bytes);
                }
                d
            };
            let zi = z - off[0];
            let (x0, x1) = (off[2].max(rx), (off[2] + cx).min(rx + rw));
            let (y0, y1) = (off[1].max(ry), (off[1] + cy).min(ry + rh));
            let n = ((x1 - x0) * bps) as usize;
            for y in y0..y1 {
                let src = ((zi * cy + (y - off[1])) * cx + (x0 - off[2])) * bps;
                let dst = ((y - ry) * rw + (x0 - rx)) * bps;
                let (src, dst) = (src as usize, dst as usize);
                out[dst..dst + n].copy_from_slice(&dec[src..src + n]);
            }
        }
        Ok(())
    }
}

/// Where a plane read finds its bytes: the parsed file, the namespace and path to read stored
/// samples from, and the format id its errors name.
#[derive(Debug, Clone, Copy)]
pub struct PlaneSource<'a> {
    pub h5: &'a hdf5_pure::File,
    pub fs: &'a Fs,
    pub path: &'a Path,
    pub format: &'static str,
}

/// Undo a chunk's filters (reverse pipeline order), skipping those masked off for this chunk.
fn unfilter(
    mut data: Vec<u8>,
    filters: &[(u16, Vec<u32>)],
    mask: u32,
    bps: usize,
    format: &'static str,
    what: &str,
) -> Result<Vec<u8>> {
    for (i, (id, cd)) in filters.iter().enumerate().rev() {
        if i < 32 && mask & (1 << i) != 0 {
            continue;
        }
        let codec = |e: openreadout_codecs::CodecError| {
            Error::corrupt(format, format!("{what}: chunk filter {id}: {e}"))
        };
        data = match *id {
            FILTER_DEFLATE => openreadout_codecs::zlib_decode(&data, 0).map_err(codec)?,
            FILTER_LZ4 => openreadout_codecs::hdf5_lz4_decode(&data).map_err(codec)?,
            FILTER_SHUFFLE => {
                let ts = cd.first().map_or(bps, |v| *v as usize).max(1);
                unshuffle(&data, ts)
            }
            FILTER_FLETCHER32 => {
                let n = data.len().saturating_sub(4);
                data.truncate(n);
                data
            }
            other => {
                return Err(Error::unsupported(
                    format,
                    format!("HDF5 filter {other} ({what})"),
                    "Chunks compressed with deflate, shuffle, fletcher32 and LZ4 (32004) are decoded; re-save the file with one of those.",
                ));
            }
        };
    }
    Ok(data)
}

/// HDF5 shuffle: byte `j` of element `i` was stored at `j * n + i`.
fn unshuffle(src: &[u8], ts: usize) -> Vec<u8> {
    let n = src.len() / ts;
    let mut out = vec![0u8; src.len()];
    for i in 0..n {
        for j in 0..ts {
            out[i * ts + j] = src[j * n + i];
        }
    }
    let done = n * ts;
    out[done..].copy_from_slice(&src[done..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_attr_text_and_unshuffle() {
        let mut b = vec![0u8; 1100];
        assert!(!looks_like_hdf5(&b));
        b[512..520].copy_from_slice(&HDF5_SIGNATURE);
        assert!(looks_like_hdf5(&b));
        let chars = AttrValue::StringArray(vec!["1".into(), "0".into(), "2".into(), "4".into()]);
        assert_eq!(attr_text(&chars).as_deref(), Some("1024"));
        assert_eq!(attr_text(&AttrValue::String("\0".into())), None);
        assert_eq!(attr_json(&AttrValue::U32(7)), json!(7));
        assert_eq!(unshuffle(&[1, 3, 2, 4, 9], 2), vec![1, 2, 3, 4, 9]);
        let e = unfilter(vec![1, 2], &[(4, vec![])], 0, 1, "t", "x").unwrap_err();
        assert_eq!(e.exit_code(), 6);
        // masked-off filters are skipped
        assert_eq!(
            unfilter(vec![1, 2], &[(FILTER_DEFLATE, vec![])], 1, 1, "t", "x").unwrap(),
            vec![1, 2]
        );
    }
}
