//! Bounds-checked little-endian reading. Every read that would run past the buffer becomes a
//! `Corrupt` error carrying the absolute file offset, never a panic.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use openreadout_core::bytes::utf16le_z;
use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};

use crate::FORMAT_ID;

/// Longest UTF-16 string (in code units) we accept from a length-prefixed field.
pub const MAX_TEXT_UNITS: u32 = 1 << 20;

/// A read position inside a byte buffer that was loaded from absolute file offset `base`.
#[derive(Debug, Clone)]
pub struct Cursor<'a> {
    buf: &'a [u8],
    base: u64,
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(buf: &'a [u8], base: u64) -> Self {
        Cursor { buf, base, pos: 0 }
    }

    /// Absolute file offset of the next byte.
    pub fn offset(&self) -> u64 {
        self.base + self.pos as u64
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    pub fn seek_to(&mut self, pos: usize) -> Result<()> {
        if pos > self.buf.len() {
            return Err(self.short(pos.saturating_sub(self.pos)));
        }
        self.pos = pos;
        Ok(())
    }

    fn short(&self, want: usize) -> Error {
        Error::corrupt_at(
            FORMAT_ID,
            self.offset(),
            format!(
                "structure runs past the end of the data ({want} bytes needed, {} available)",
                self.remaining()
            ),
        )
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or_else(|| self.short(n))?;
        let s = self.buf.get(self.pos..end).ok_or_else(|| self.short(n))?;
        self.pos = end;
        Ok(s)
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n).map(|_| ())
    }

    fn arr<const N: usize>(&mut self) -> Result<[u8; N]> {
        let s = self.take(N)?;
        let mut a = [0u8; N];
        a.copy_from_slice(s);
        Ok(a)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.arr::<1>()?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.arr()?))
    }
    pub fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_le_bytes(self.arr()?))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.arr()?))
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.arr()?))
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.arr()?))
    }
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.arr()?))
    }
    pub fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(self.arr()?))
    }

    /// Fixed-width, zero-padded UTF-16LE text of `nbytes` bytes.
    pub fn utf16_fixed(&mut self, nbytes: usize) -> Result<String> {
        Ok(utf16le_z(self.take(nbytes)?))
    }

    /// u32 count of UTF-16 code units, then the units (no terminator).
    pub fn utf16_counted(&mut self) -> Result<String> {
        let at = self.offset();
        let n = self.u32()?;
        if n > MAX_TEXT_UNITS {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                at,
                format!("implausible text length {n}"),
            ));
        }
        let raw = self.take(n as usize * 2)?;
        Ok(utf16le_z(raw))
    }
}

/// Read `len` bytes at absolute `offset`; running past the end of the file is corruption.
pub fn read_at(
    file: &mut SourceFile,
    path: &Path,
    offset: u64,
    len: u64,
    what: &str,
) -> Result<Vec<u8>> {
    let file_len = file.metadata().map_err(|e| Error::io(path, e))?.len();
    let end = offset
        .checked_add(len)
        .ok_or_else(|| Error::corrupt_at(FORMAT_ID, offset, format!("{what}: length overflows")))?;
    if end > file_len {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            offset,
            format!(
                "{what} ends at byte {end} but the file has only {file_len} bytes (truncated?)"
            ),
        ));
    }
    let n = usize::try_from(len)
        .map_err(|_| Error::corrupt_at(FORMAT_ID, offset, format!("{what}: too large")))?;
    let mut buf = vec![0u8; n];
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| Error::io(path, e))?;
    file.read_exact(&mut buf).map_err(|e| Error::io(path, e))?;
    Ok(buf)
}

/// Adler-32 (RFC 1950) with the conventional start value 1; `seed` lets callers start from 0.
pub fn adler32(seed: u32, data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let mut a = seed & 0xFFFF;
    let mut b = seed >> 16;
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += u32::from(x);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adler_known_values() {
        // RFC 1950 example: "Wikipedia" -> 0x11E60398 with the standard start value 1.
        assert_eq!(adler32(1, b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(0, b""), 0);
    }

    #[test]
    fn cursor_errors_instead_of_panicking() {
        let data = [1u8, 0, 0];
        let mut c = Cursor::new(&data, 100);
        assert!(c.u32().is_err());
        assert_eq!(c.u16().unwrap(), 1);
        assert!(c.utf16_counted().is_err());
    }
}
