//! Minimal reader of NumPy `.npy` files (the format NumPy documents: magic `\x93NUMPY`, version,
//! header length, a Python-literal dict with `descr`, `fortran_order` and `shape`). Only the
//! element types Open Ephys writes are accepted.

use std::path::Path;

use openreadout_core::bytes::until_nul;
use openreadout_core::source::Fs;
use openreadout_core::{Error, Result};

use super::OPEN_EPHYS_FORMAT_ID;

/// Largest `.npy` header accepted, bytes.
pub const MAX_NPY_HEADER: u64 = 64 * 1024;
/// Largest array read whole, bytes.
pub const MAX_NPY_BYTES: u64 = 1 << 31;

/// Element type of an `.npy` array.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NpyType {
    /// `|u1`.
    U8,
    /// `<i2`.
    I16,
    /// `<u2`.
    U16,
    /// `<i4`.
    I32,
    /// `<i8`.
    I64,
    /// `<u8`.
    U64,
    /// `<f4`.
    F32,
    /// `<f8`.
    F64,
    /// `|S<n>`: fixed-width byte strings.
    Bytes(u32),
}

impl NpyType {
    fn parse(descr: &str) -> Option<Self> {
        Some(match descr {
            "|u1" | "<u1" => Self::U8,
            "<i2" => Self::I16,
            "<u2" => Self::U16,
            "<i4" => Self::I32,
            "<i8" => Self::I64,
            "<u8" => Self::U64,
            "<f4" => Self::F32,
            "<f8" => Self::F64,
            s if s.starts_with("|S") => Self::Bytes(s[2..].parse().ok()?),
            _ => return None,
        })
    }
    /// Bytes per element.
    pub fn width(self) -> u64 {
        match self {
            Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::F32 => 4,
            Self::I64 | Self::U64 | Self::F64 => 8,
            Self::Bytes(n) => u64::from(n),
        }
    }
    /// True for integer types.
    pub fn is_integer(self) -> bool {
        !matches!(self, Self::F32 | Self::F64 | Self::Bytes(_))
    }
}

/// A parsed `.npy` header.
#[derive(Debug, Clone)]
pub struct NpyHeader {
    /// Element type.
    pub dtype: NpyType,
    /// Shape (C order).
    pub shape: Vec<u64>,
    /// Byte offset of the data.
    pub data_offset: u64,
}

impl NpyHeader {
    /// Elements in the array.
    pub fn len(&self) -> u64 {
        self.shape.iter().product()
    }
    /// True when the array has no elements.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn bad(path: &Path, what: &str) -> Error {
    Error::corrupt(OPEN_EPHYS_FORMAT_ID, format!("{}: {what}", path.display()))
}

/// The value of `'key':` in a Python dict literal (up to the next top-level comma or brace).
fn dict_value<'a>(h: &'a str, key: &str) -> Option<&'a str> {
    let k = h
        .find(&format!("'{key}'"))
        .or_else(|| h.find(&format!("\"{key}\"")))?;
    let rest = &h[k + key.len() + 2..];
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    if rest.starts_with('(') {
        let end = rest.find(')')?;
        return Some(&rest[..=end]);
    }
    let end = rest.find([',', '}']).unwrap_or(rest.len());
    Some(rest[..end].trim())
}

/// Parse the header of an `.npy` file.
pub fn read_header(fs: &Fs, path: &Path) -> Result<NpyHeader> {
    let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
    let len = f.size().map_err(|e| Error::io(path, e))?;
    let pre = openreadout_core::bytes::read_block(&mut f, path, 0, 12, len)?;
    if pre.slice(0, 6) != Some(&b"\x93NUMPY"[..]) {
        return Err(bad(path, "not a NumPy .npy file"));
    }
    let major = pre.u8_at(6).unwrap_or(0);
    let (hlen, start) = match major {
        1 => (u64::from(pre.u16_at(8).unwrap_or(0)), 10),
        2 | 3 => (u64::from(pre.u32_at(8).unwrap_or(0)), 12),
        _ => return Err(bad(path, "unknown .npy version")),
    };
    if hlen > MAX_NPY_HEADER {
        return Err(bad(path, ".npy header too large"));
    }
    let hb = openreadout_core::bytes::read_block(&mut f, path, start, hlen, len)?;
    if hb.len() as u64 != hlen {
        return Err(bad(path, ".npy header cut off"));
    }
    let h = String::from_utf8_lossy(&hb.bytes).to_string();
    let descr = dict_value(&h, "descr")
        .ok_or_else(|| bad(path, ".npy header has no descr"))?
        .trim_matches(['\'', '"']);
    let dtype = NpyType::parse(descr).ok_or_else(|| {
        Error::unsupported(
            OPEN_EPHYS_FORMAT_ID,
            format!(".npy element type {descr}"),
            format!(
                "{} stores {descr} values, which this reader does not decode.",
                path.display()
            ),
        )
    })?;
    if dict_value(&h, "fortran_order").is_some_and(|v| v.starts_with("True")) {
        return Err(Error::unsupported(
            OPEN_EPHYS_FORMAT_ID,
            "Fortran-ordered .npy arrays",
            format!("{} is stored in Fortran order.", path.display()),
        ));
    }
    let shape_s = dict_value(&h, "shape").ok_or_else(|| bad(path, ".npy header has no shape"))?;
    let shape: Vec<u64> = shape_s
        .trim_matches(['(', ')'])
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.trim_end_matches('L').parse::<u64>())
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| bad(path, ".npy shape is not a tuple of integers"))?;
    let hdr = NpyHeader {
        dtype,
        shape,
        data_offset: start + hlen,
    };
    let need = hdr
        .len()
        .checked_mul(dtype.width())
        .and_then(|n| n.checked_add(hdr.data_offset));
    if need.is_none_or(|n| n > len) {
        return Err(Error::corrupt(
            OPEN_EPHYS_FORMAT_ID,
            format!(
                "{}: the header declares {} values but the file has {len} bytes (truncated)",
                path.display(),
                hdr.len()
            ),
        ));
    }
    Ok(hdr)
}

/// Values `first..first+n` of a numeric array as f64.
pub fn read_values(fs: &Fs, path: &Path, h: &NpyHeader, first: u64, n: u64) -> Result<Vec<f64>> {
    if matches!(h.dtype, NpyType::Bytes(_)) {
        return Err(bad(path, "text array read as numbers"));
    }
    let width = h.dtype.width();
    let n = n.min(h.len().saturating_sub(first));
    let bytes_n = n.saturating_mul(width);
    if bytes_n > MAX_NPY_BYTES {
        return Err(Error::unsupported(
            OPEN_EPHYS_FORMAT_ID,
            "arrays larger than 2 GiB",
            format!("{} is too large to read whole.", path.display()),
        ));
    }
    let mut file = fs.open(path).map_err(|e| Error::io(path, e))?;
    let len = file.size().map_err(|e| Error::io(path, e))?;
    let at = h.data_offset + first * width;
    let block = openreadout_core::bytes::read_block(&mut file, path, at, bytes_n, len)?;
    let mut out = Vec::with_capacity(n as usize);
    let raw = &block.bytes;
    for k in 0..n as usize {
        let s = &raw[k * width as usize..(k + 1) * width as usize];
        out.push(match h.dtype {
            NpyType::U8 => f64::from(s[0]),
            NpyType::I16 => f64::from(i16::from_le_bytes([s[0], s[1]])),
            NpyType::U16 => f64::from(u16::from_le_bytes([s[0], s[1]])),
            NpyType::I32 => f64::from(i32::from_le_bytes([s[0], s[1], s[2], s[3]])),
            NpyType::F32 => f64::from(f32::from_le_bytes([s[0], s[1], s[2], s[3]])),
            NpyType::I64 => {
                i64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]) as f64
            }
            NpyType::U64 => {
                u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]) as f64
            }
            NpyType::F64 => f64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]),
            NpyType::Bytes(_) => f64::NAN,
        });
    }
    Ok(out)
}

/// Every string of a `|S<n>` array (cut at the first NUL, decoded as UTF-8 with replacement).
pub fn read_strings(fs: &Fs, path: &Path, h: &NpyHeader) -> Result<Vec<String>> {
    let NpyType::Bytes(w) = h.dtype else {
        return Err(bad(path, "numeric array read as text"));
    };
    let total = h.len().saturating_mul(u64::from(w));
    if total > MAX_NPY_BYTES {
        return Err(Error::unsupported(
            OPEN_EPHYS_FORMAT_ID,
            "text arrays larger than 2 GiB",
            format!("{} is too large to read whole.", path.display()),
        ));
    }
    let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
    let len = f.size().map_err(|e| Error::io(path, e))?;
    let b = openreadout_core::bytes::read_block(&mut f, path, h.data_offset, total, len)?;
    Ok(b.bytes
        .chunks(w.max(1) as usize)
        .map(|c| String::from_utf8_lossy(until_nul(c)).to_string())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn npy(descr: &str, shape: &str, data: &[u8]) -> Vec<u8> {
        let mut h = format!("{{'descr': '{descr}', 'fortran_order': False, 'shape': {shape}, }}");
        while (10 + h.len() + 1) % 64 != 0 {
            h.push(' ');
        }
        h.push('\n');
        let mut b = b"\x93NUMPY\x01\x00".to_vec();
        b.extend((h.len() as u16).to_le_bytes());
        b.extend(h.as_bytes());
        b.extend(data);
        b
    }

    #[test]
    fn reads_numbers_and_strings() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.npy");
        let data: Vec<u8> = [5i64, -7].iter().flat_map(|v| v.to_le_bytes()).collect();
        std::fs::write(&p, npy("<i8", "(2,)", &data)).unwrap();
        let fs = Fs::local();
        let h = read_header(&fs, &p).unwrap();
        assert_eq!(h.shape, vec![2]);
        assert_eq!(read_values(&fs, &p, &h, 0, 10).unwrap(), vec![5.0, -7.0]);
        let q = dir.path().join("t.npy");
        std::fs::write(&q, npy("|S4", "(2,)", b"ab\0\0wxyz")).unwrap();
        let h = read_header(&fs, &q).unwrap();
        assert_eq!(read_strings(&fs, &q, &h).unwrap(), vec!["ab", "wxyz"]);
        // truncated data
        let r = dir.path().join("cut.npy");
        std::fs::write(&r, npy("<i8", "(3,)", &data)).unwrap();
        assert!(read_header(&fs, &r).is_err());
        assert!(read_header(&fs, &p.with_file_name("missing.npy")).is_err());
    }
}
