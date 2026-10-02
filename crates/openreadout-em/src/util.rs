//! Crate-private helpers shared by the EM readers: bounded file reads, byte-order
//! names, half-float widening and timestamp conversion.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use openreadout_core::source::{Fs, SourceFile};
use openreadout_core::{Error, Result};

pub(crate) use openreadout_core::bytes::Endian;

/// How a byte order is named in metadata and diagnostics.
pub(crate) fn byte_order_name(e: Endian) -> &'static str {
    match e {
        Endian::Little => "little-endian",
        Endian::Big => "big-endian",
    }
}

/// A byte source served to `hdf5-pure` (EMD files from memory or a host callback).
#[derive(Debug)]
pub(crate) struct H5Source {
    src: std::sync::Arc<dyn openreadout_core::source::ByteSource>,
    len: u64,
}

impl H5Source {
    pub(crate) fn new(
        src: std::sync::Arc<dyn openreadout_core::source::ByteSource>,
    ) -> std::io::Result<Self> {
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

/// An open file with its length; every read is bounds-checked against the length.
#[derive(Debug)]
pub(crate) struct Blob {
    pub(crate) path: PathBuf,
    file: SourceFile,
    pub(crate) len: u64,
}

impl Blob {
    pub(crate) fn open(fs: &Fs, path: &Path) -> Result<Self> {
        let file = fs.open(path).map_err(|e| Error::io(path, e))?;
        let len = file.metadata().map_err(|e| Error::io(path, e))?.len();
        Ok(Blob {
            path: path.to_path_buf(),
            file,
            len,
        })
    }

    /// Read exactly `len` bytes at `offset`; a range past the end of the file is a corrupt-file error.
    pub(crate) fn read_at(
        &mut self,
        format: &'static str,
        offset: u64,
        len: u64,
    ) -> Result<Vec<u8>> {
        let end = offset
            .checked_add(len)
            .ok_or_else(|| Error::corrupt_at(format, offset, "byte range overflows"))?;
        if end > self.len {
            return Err(Error::corrupt_at(
                format,
                offset,
                format!(
                    "needs bytes {offset}..{end} but the file has only {} bytes (truncated?)",
                    self.len
                ),
            ));
        }
        let n = usize::try_from(len)
            .map_err(|_| Error::corrupt_at(format, offset, "byte range too large"))?;
        let mut buf = vec![0u8; n];
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|e| Error::io(&self.path, e))?;
        self.file
            .read_exact(&mut buf)
            .map_err(|e| Error::io(&self.path, e))?;
        Ok(buf)
    }

    /// Read up to `len` bytes at `offset` (fewer at the end of the file).
    pub(crate) fn read_upto(&mut self, offset: u64, len: u64) -> Result<Vec<u8>> {
        if offset >= self.len {
            return Ok(Vec::new());
        }
        let n = len.min(self.len - offset);
        let mut buf = vec![0u8; usize::try_from(n).unwrap_or(usize::MAX)];
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|e| Error::io(&self.path, e))?;
        self.file
            .read_exact(&mut buf)
            .map_err(|e| Error::io(&self.path, e))?;
        Ok(buf)
    }
}

/// Reverse the bytes of every `width`-byte sample in place (big-endian → little-endian).
pub(crate) fn swap_samples(data: &mut [u8], width: usize) {
    if width > 1 {
        for c in data.chunks_exact_mut(width) {
            c.reverse();
        }
    }
}

/// Half-float widening lives in `openreadout-core` (shared with the LIF reader).
pub(crate) use openreadout_core::pixel::widen_half;

/// Bytes up to the first NUL, as text (UTF-8, falling back to Latin-1), trimmed.
pub(crate) fn text(b: &[u8]) -> String {
    let b = openreadout_core::bytes::until_nul(b);
    let s = match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => openreadout_core::bytes::latin1(b),
    };
    s.trim().to_string()
}

/// OLE Automation date (days since 1899-12-30, fraction = time of day) → ISO-8601 UTC.
pub(crate) fn ole_date_to_iso(days: f64) -> Option<String> {
    // 1899-12-30 is 25569 days before 1970-01-01.
    if !days.is_finite() || days <= 0.0 || days > 2_958_465.0 {
        return None;
    }
    let ms = ((days - 25_569.0) * 86_400_000.0).round() as i64;
    Some(openreadout_core::time::unix_to_iso8601(
        ms.div_euclid(1000),
        ms.rem_euclid(1000) as u32,
    ))
}

/// Microseconds since the Unix epoch → ISO-8601 UTC (years 1970–9999 only).
pub(crate) fn unix_micros_to_iso(us: i64) -> Option<String> {
    if us <= 0 || us > 253_402_300_799_000_000 {
        return None;
    }
    let ms = us / 1000;
    Some(openreadout_core::time::unix_to_iso8601(
        ms.div_euclid(1000),
        ms.rem_euclid(1000) as u32,
    ))
}

/// A finite f64 as JSON (non-finite values become null).
pub(crate) fn num(v: f64) -> serde_json::Value {
    serde_json::Number::from_f64(v).map_or(serde_json::Value::Null, serde_json::Value::Number)
}

/// Lower-case file extension (`""` when absent).
pub(crate) fn ext_lower(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use openreadout_core::pixel::half_to_f32;

    #[test]
    fn half_floats_widen_exactly() {
        assert_eq!(half_to_f32(0x3c00).to_bits(), 1.0f32.to_bits());
        assert_eq!(half_to_f32(0xc000).to_bits(), (-2.0f32).to_bits());
        assert_eq!(half_to_f32(0x0001).to_bits(), 2f32.powi(-24).to_bits());
        assert!(half_to_f32(0x7c00).is_infinite());
        assert!(half_to_f32(0x7e00).is_nan());
    }

    #[test]
    fn ole_dates() {
        // 2020-01-01T00:00:00Z = 43831 days after 1899-12-30
        assert_eq!(
            ole_date_to_iso(43_831.0).as_deref(),
            Some("2020-01-01T00:00:00.000Z")
        );
        assert_eq!(
            ole_date_to_iso(43_831.5).as_deref(),
            Some("2020-01-01T12:00:00.000Z")
        );
        assert!(ole_date_to_iso(0.0).is_none());
        assert!(ole_date_to_iso(f64::NAN).is_none());
    }

    #[test]
    fn samples_swap_bytes() {
        let mut d = vec![1, 2, 3, 4];
        swap_samples(&mut d, 2);
        assert_eq!(d, vec![2, 1, 4, 3]);
    }
}
