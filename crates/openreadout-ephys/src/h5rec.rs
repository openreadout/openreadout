//! HDF5 helpers shared by the multi-electrode-array readers (`mcsh5`, `brw`, `maxwell`): table
//! datasets of compound records whose members may be variable-length strings (resolved from the
//! file's global heap, as the HDF5 file format specification lays it out), and windows of 2-D
//! sample matrices read chunk by chunk.

use std::collections::HashMap;
use std::path::Path;

use hdf5_pure::{Datatype, DatatypeByteOrder};
use openreadout_core::bytes::read_block;
use openreadout_core::source::{Fs, SourceFile};
use openreadout_core::{Error, PixelType, Result};
use openreadout_hdf5::h5util::{PlaneDataset, PlaneSource};

/// Records of one table dataset read at most.
pub const MAX_H5_RECORDS: usize = 1 << 20;
/// Rows of a sample matrix accepted at most.
pub const MAX_H5_MATRIX_ROWS: u64 = 1 << 16;
/// Bytes of one table dataset read at most.
pub const MAX_H5_TABLE_BYTES: u64 = 64 << 20;
/// Decoded bytes of one read at most per byte of the file (chunks are compressed at most this much).
pub const MAX_H5_EXPANSION: u64 = 1024;
/// Bytes of one global heap collection read at most.
pub const MAX_H5_HEAP_BYTES: u64 = 64 << 20;

/// One member value of a compound record.
#[derive(Debug, Clone, PartialEq)]
pub enum Field {
    /// An integer member.
    Int(i64),
    /// A floating-point member.
    Float(f64),
    /// A string member (fixed or variable length), trimmed of NULs and spaces.
    Text(String),
    /// Anything else (nested compounds, arrays, references).
    Other,
}

impl Field {
    /// The value as an integer, if it is a number.
    pub fn int(&self) -> Option<i64> {
        match self {
            Field::Int(v) => Some(*v),
            Field::Float(v) if v.fract() == 0.0 && v.is_finite() => Some(*v as i64),
            _ => None,
        }
    }

    /// The value as a float, if it is a number.
    pub fn float(&self) -> Option<f64> {
        match self {
            Field::Int(v) => Some(*v as f64),
            Field::Float(v) => Some(*v),
            _ => None,
        }
    }

    /// The value as text (numbers in decimal).
    pub fn text(&self) -> String {
        match self {
            Field::Text(s) => s.clone(),
            Field::Int(v) => v.to_string(),
            Field::Float(v) => v.to_string(),
            Field::Other => String::new(),
        }
    }
}

/// One record: member names and values in datatype order.
pub type Record = Vec<(String, Field)>;

/// The value of member `name` of a record.
pub fn field<'a>(r: &'a Record, name: &str) -> Option<&'a Field> {
    r.iter().find(|(n, _)| n == name).map(|(_, v)| v)
}

/// Reads global heap objects (variable-length data) of one file, caching the collections.
#[derive(Debug)]
pub struct Heap {
    f: SourceFile,
    len: u64,
    offset_size: usize,
    length_size: usize,
    base: u64,
    cache: HashMap<u64, Vec<(u16, Vec<u8>)>>,
    format: &'static str,
}

fn le(b: &[u8]) -> u64 {
    b.iter()
        .rev()
        .fold(0u64, |acc, &x| (acc << 8) | u64::from(x))
}

impl Heap {
    /// Open the heap reader of the HDF5 file at `path` (already parsed as `h5`).
    pub fn open(fs: &Fs, path: &Path, h5: &hdf5_pure::File, format: &'static str) -> Result<Self> {
        let f = fs.open(path).map_err(|e| Error::io(path, e))?;
        let len = f.size().map_err(|e| Error::io(path, e))?;
        let sb = h5.superblock();
        Ok(Heap {
            f,
            len,
            offset_size: usize::from(sb.offset_size).clamp(1, 8),
            length_size: usize::from(sb.length_size).clamp(1, 8),
            base: sb.base_address.get(),
            cache: HashMap::new(),
            format,
        })
    }

    fn collection(&mut self, addr: u64, path: &Path) -> Result<&Vec<(u16, Vec<u8>)>> {
        if !self.cache.contains_key(&addr) {
            let at = self.base.saturating_add(addr);
            let head_len = 8 + self.length_size as u64;
            let head = read_block(&mut self.f, path, at, head_len, self.len)?;
            if head.bytes.len() < head_len as usize || &head.bytes[..4] != b"GCOL" {
                return Err(Error::corrupt_at(
                    self.format,
                    at,
                    "a variable-length string points to something that is not a global heap collection",
                ));
            }
            let size = le(&head.bytes[8..8 + self.length_size]);
            if size > MAX_H5_HEAP_BYTES || size < head_len {
                return Err(Error::corrupt_at(
                    self.format,
                    at,
                    format!("a global heap collection of {size} bytes"),
                ));
            }
            let all = read_block(&mut self.f, path, at, size, self.len)?.bytes;
            let mut objects = Vec::new();
            let mut pos = head_len as usize;
            let ohead = 8 + self.length_size;
            while pos + ohead <= all.len() {
                let index = u16::from_le_bytes([all[pos], all[pos + 1]]);
                if index == 0 {
                    break;
                }
                let n = usize::try_from(le(&all[pos + 8..pos + ohead])).unwrap_or(usize::MAX);
                let start = pos + ohead;
                let Some(end) = start.checked_add(n).filter(|&e| e <= all.len()) else {
                    break;
                };
                objects.push((index, all[start..end].to_vec()));
                pos = start + n.div_ceil(8) * 8;
            }
            self.cache.insert(addr, objects);
        }
        Ok(self.cache.get(&addr).expect("just inserted"))
    }

    /// The bytes of heap object `index` of the collection at `addr` (stored address).
    pub fn object(&mut self, addr: u64, index: u32, path: &Path) -> Result<Vec<u8>> {
        let format = self.format;
        let objects = self.collection(addr, path)?;
        objects
            .iter()
            .find(|(i, _)| u32::from(*i) == index)
            .map(|(_, b)| b.clone())
            .ok_or_else(|| {
                Error::corrupt(
                    format,
                    format!("global heap object {index} at {addr} is missing"),
                )
            })
    }

    /// Bytes of a variable-length element (sequence length, heap address, object index).
    pub fn vl_size(&self) -> usize {
        4 + self.offset_size + 4
    }
}

fn trim_text(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end])
        .trim_matches(|c: char| c.is_whitespace())
        .to_string()
}

fn member(dt: &Datatype, b: &[u8], heap: &mut Heap, path: &Path) -> Result<Field> {
    Ok(match dt {
        Datatype::FixedPoint {
            size,
            byte_order,
            layout,
        } => {
            let n = (*size as usize).min(8);
            let Some(x) = b.get(..n) else {
                return Ok(Field::Other);
            };
            let mut v = match byte_order {
                DatatypeByteOrder::BigEndian => {
                    x.iter().fold(0u64, |a, &c| (a << 8) | u64::from(c))
                }
                _ => le(x),
            };
            if layout.signed && n < 8 && n > 0 && v & (1 << (n * 8 - 1)) != 0 {
                v |= !0u64 << (n * 8);
            }
            if layout.signed {
                Field::Int(v as i64)
            } else {
                Field::Int(i64::try_from(v).unwrap_or(i64::MAX))
            }
        }
        Datatype::FloatingPoint {
            size, byte_order, ..
        } => {
            let be = matches!(byte_order, DatatypeByteOrder::BigEndian);
            match (*size, b.len()) {
                (4, 4..) => {
                    let a = [b[0], b[1], b[2], b[3]];
                    Field::Float(f64::from(if be {
                        f32::from_be_bytes(a)
                    } else {
                        f32::from_le_bytes(a)
                    }))
                }
                (8, 8..) => {
                    let a = [b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]];
                    Field::Float(if be {
                        f64::from_be_bytes(a)
                    } else {
                        f64::from_le_bytes(a)
                    })
                }
                _ => Field::Other,
            }
        }
        Datatype::String { size, .. } => {
            Field::Text(trim_text(b.get(..*size as usize).unwrap_or(b)))
        }
        Datatype::VariableLength {
            is_string: true, ..
        } => {
            let n = heap.vl_size();
            if b.len() < n {
                return Ok(Field::Other);
            }
            let count = le(&b[..4]);
            let addr = le(&b[4..n - 4]);
            let index = u32::from_le_bytes([b[n - 4], b[n - 3], b[n - 2], b[n - 1]]);
            if count == 0 || addr == 0 {
                Field::Text(String::new())
            } else {
                let bytes = heap.object(addr, index, path)?;
                let take = usize::try_from(count)
                    .unwrap_or(usize::MAX)
                    .min(bytes.len());
                Field::Text(trim_text(&bytes[..take]))
            }
        }
        _ => Field::Other,
    })
}

/// Every record of a compound dataset (at most `MAX_H5_RECORDS`).
pub fn records(ds: &hdf5_pure::Dataset, heap: &mut Heap, path: &Path) -> Result<Vec<Record>> {
    let format = heap.format;
    let err = |e: hdf5_pure::Error| Error::corrupt(format, format!("{}: {e}", path.display()));
    let dt = ds.datatype().map_err(err)?;
    let Datatype::Compound { size, members } = &dt else {
        return Err(Error::corrupt(
            format,
            "a table dataset is not a compound type",
        ));
    };
    let size = *size as usize;
    let n: u64 = ds.shape().map_err(err)?.iter().product();
    if usize::try_from(n).is_ok_and(|n| n > MAX_H5_RECORDS) {
        return Err(Error::unsupported(
            format,
            format!("a table of {n} records"),
            "The file may be damaged; the limit is MAX_H5_RECORDS.",
        ));
    }
    if n == 0 || size == 0 {
        return Ok(Vec::new());
    }
    if n.saturating_mul(size as u64) > MAX_H5_TABLE_BYTES {
        return Err(Error::unsupported(
            format,
            format!("a table of {n} records of {size} bytes"),
            "The file may be damaged; the limit is MAX_H5_TABLE_BYTES.",
        ));
    }
    let raw = ds.read_raw().map_err(err)?;
    let mut out = Vec::new();
    for rec in raw.chunks_exact(size) {
        let mut r = Vec::with_capacity(members.len());
        for m in members {
            let at = usize::try_from(m.byte_offset).unwrap_or(usize::MAX);
            let v = match rec.get(at..) {
                Some(b) => member(&m.datatype, b, heap, path)?,
                None => Field::Other,
            };
            r.push((m.name.clone(), v));
        }
        out.push(r);
    }
    Ok(out)
}

/// Refuse a whole-dataset read the file cannot back: more than `MAX_H5_EXPANSION` × the file
/// length, or more than `MAX_H5_TABLE_BYTES` × 4.
pub fn check_backed(
    ds: &hdf5_pure::Dataset,
    file_len: u64,
    format: &'static str,
    what: &str,
) -> Result<()> {
    let n: u64 = ds
        .shape()
        .map_err(|e| Error::corrupt(format, format!("{what}: {e}")))?
        .iter()
        .fold(1u64, |a, &d| a.saturating_mul(d));
    let bytes = n.saturating_mul(ds.element_size().unwrap_or(1));
    if bytes > MAX_H5_EXPANSION.saturating_mul(file_len) || bytes > 4 * MAX_H5_TABLE_BYTES {
        return Err(Error::unsupported(
            format,
            format!("{what}: {bytes} bytes in a {file_len}-byte file"),
            "The file may be damaged; the limits are MAX_H5_EXPANSION and MAX_H5_TABLE_BYTES.",
        ));
    }
    Ok(())
}

/// Little-endian samples of `pt` as f64.
pub fn decode_samples(bytes: &[u8], pt: PixelType, out: &mut Vec<f64>) {
    match pt {
        PixelType::Uint8 => out.extend(bytes.iter().map(|&v| f64::from(v))),
        PixelType::Int8 => out.extend(bytes.iter().map(|&v| f64::from(v as i8))),
        PixelType::Uint16 => out.extend(
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|x| f64::from(u16::from_le_bytes(*x))),
        ),
        PixelType::Int16 => out.extend(
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|x| f64::from(i16::from_le_bytes(*x))),
        ),
        PixelType::Uint32 => out.extend(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|x| f64::from(u32::from_le_bytes(*x))),
        ),
        PixelType::Int32 => out.extend(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|x| f64::from(i32::from_le_bytes(*x))),
        ),
        PixelType::Float => out.extend(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|x| f64::from(f32::from_le_bytes(*x))),
        ),
        PixelType::Double => out.extend(
            bytes
                .as_chunks::<8>()
                .0
                .iter()
                .map(|x| f64::from_le_bytes(*x)),
        ),
        _ => {}
    }
}

/// NumPy-style name of a sample type.
pub fn dtype_of(pt: PixelType) -> &'static str {
    match pt {
        PixelType::Uint8 => "uint8",
        PixelType::Int8 => "int8",
        PixelType::Uint16 => "uint16",
        PixelType::Int16 => "int16",
        PixelType::Uint32 => "uint32",
        PixelType::Int32 => "int32",
        PixelType::Float => "float32",
        PixelType::Double => "float64",
        _ => "other",
    }
}

/// A 2-D sample matrix (rows × columns) read by column windows.
#[derive(Debug, Clone)]
pub struct Matrix {
    /// Dataset path.
    pub path: String,
    /// Rows (channels).
    pub rows: u64,
    /// Columns (samples).
    pub cols: u64,
    ds: PlaneDataset,
}

impl Matrix {
    /// Describe the rank-2 numeric dataset at `path` of a `file_len`-byte file. A matrix the
    /// file cannot back (more than `MAX_H5_EXPANSION` × its length; for storage that is neither
    /// chunked nor an allocated contiguous run, more than `MAX_H5_TABLE_BYTES`) is refused.
    pub fn open(
        h5: &hdf5_pure::File,
        path: &str,
        format: &'static str,
        file_len: u64,
    ) -> Result<Self> {
        let ds = PlaneDataset::open(h5, path, format)?;
        let [z, rows, cols] = ds.shape;
        let total = rows
            .saturating_mul(cols)
            .saturating_mul(ds.pixel_type.bytes_per_sample() as u64);
        let stored = h5
            .dataset(path)
            .ok()
            .and_then(|d| d.layout().ok())
            .is_some_and(|l| {
                matches!(
                    l,
                    hdf5_pure::Layout::Chunked { .. }
                        | hdf5_pure::Layout::Contiguous {
                            address: Some(_),
                            ..
                        }
                )
            });
        if total > MAX_H5_EXPANSION.saturating_mul(file_len)
            || (!stored && total > MAX_H5_TABLE_BYTES)
        {
            return Err(Error::unsupported(
                format,
                format!("{path}: a sample matrix of {total} bytes in a {file_len}-byte file"),
                "The file may be damaged; the limits are MAX_H5_EXPANSION and MAX_H5_TABLE_BYTES.",
            ));
        }
        let chunk = ds.chunk_shape().map(|[a, b, c]| {
            a.saturating_mul(b)
                .saturating_mul(c)
                .saturating_mul(ds.pixel_type.bytes_per_sample() as u64)
        });
        if chunk.is_some_and(|c| c > MAX_H5_TABLE_BYTES) {
            return Err(Error::unsupported(
                format,
                format!("{path}: chunks of {} bytes", chunk.unwrap_or(0)),
                "The file may be damaged; chunks over MAX_H5_TABLE_BYTES are not read.",
            ));
        }
        if rows > MAX_H5_MATRIX_ROWS {
            return Err(Error::unsupported(
                format,
                format!("{path}: a sample matrix of {rows} rows"),
                "The file may be damaged; the limit is MAX_H5_MATRIX_ROWS.",
            ));
        }
        if z != 1 {
            return Err(Error::corrupt(
                format,
                format!("{path}: a sample matrix of three dimensions"),
            ));
        }
        Ok(Matrix {
            path: path.to_string(),
            rows,
            cols,
            ds,
        })
    }

    /// Stored sample type.
    pub fn pixel_type(&self) -> PixelType {
        self.ds.pixel_type
    }

    /// Filter names of the chunks.
    pub fn filters(&self) -> Vec<String> {
        self.ds.filter_names()
    }

    /// Columns `col0 .. col0 + ncols` of every row, row by row, as f64.
    pub fn read(
        &self,
        h5: &hdf5_pure::File,
        fs: &Fs,
        file: &Path,
        format: &'static str,
        col0: u64,
        ncols: u64,
    ) -> Result<Vec<Vec<f64>>> {
        if ncols == 0 || self.rows == 0 {
            return Ok(vec![Vec::new(); usize::try_from(self.rows).unwrap_or(0)]);
        }
        let src = PlaneSource {
            h5,
            fs,
            path: file,
            format,
        };
        let bytes = self.ds.read_plane_region(
            src,
            0,
            (col0, 0, ncols, self.rows),
            (self.cols, self.rows),
            None,
        )?;
        let bps = self.ds.pixel_type.bytes_per_sample();
        let row = usize::try_from(ncols).unwrap_or(0) * bps;
        let mut out = Vec::with_capacity(usize::try_from(self.rows).unwrap_or(0));
        for r in bytes.chunks_exact(row.max(1)) {
            let mut v = Vec::with_capacity(r.len() / bps.max(1));
            decode_samples(r, self.ds.pixel_type, &mut v);
            out.push(v);
        }
        Ok(out)
    }
}
