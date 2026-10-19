//! Byte and bit cursors over decoded CRAM buffers, and the variable-length integers CRAM uses:
//! ITF8 and LTF8 (CRAM specification § 2.3), uint7 (CRAM codecs § 1.4). Every read is
//! bounds-checked; running out of bytes is an error, never a panic.

/// Format id used in errors.
pub(crate) const FMT: &str = "cram";

/// An error while decoding: a message that becomes `corrupt_file`.
pub(crate) type DecodeResult<T> = std::result::Result<T, String>;

/// A cursor over a byte buffer.
#[derive(Debug, Clone)]
pub(crate) struct Cursor<'a> {
    buf: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        Cursor { buf, at: 0 }
    }

    pub(crate) fn pos(&self) -> usize {
        self.at
    }

    pub(crate) fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.at)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// The bytes not read yet.
    pub(crate) fn rest(&self) -> &'a [u8] {
        self.buf.get(self.at..).unwrap_or(&[])
    }

    pub(crate) fn u8(&mut self) -> DecodeResult<u8> {
        let b = *self
            .buf
            .get(self.at)
            .ok_or_else(|| "data ends early".to_string())?;
        self.at += 1;
        Ok(b)
    }

    pub(crate) fn take(&mut self, n: usize) -> DecodeResult<&'a [u8]> {
        let end = self
            .at
            .checked_add(n)
            .filter(|e| *e <= self.buf.len())
            .ok_or_else(|| format!("{n} bytes wanted, {} left", self.remaining()))?;
        let s = &self.buf[self.at..end];
        self.at = end;
        Ok(s)
    }

    pub(crate) fn skip(&mut self, n: usize) -> DecodeResult<()> {
        self.take(n).map(|_| ())
    }

    pub(crate) fn u16_le(&mut self) -> DecodeResult<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub(crate) fn u32_le(&mut self) -> DecodeResult<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub(crate) fn i32_le(&mut self) -> DecodeResult<i32> {
        self.u32_le().map(|v| v as i32)
    }

    /// ITF8: the count of leading one bits of the first byte says how many bytes follow.
    pub(crate) fn itf8(&mut self) -> DecodeResult<i32> {
        let b0 = u32::from(self.u8()?);
        let v = if b0 < 0x80 {
            b0
        } else if b0 < 0xc0 {
            ((b0 & 0x3f) << 8) | u32::from(self.u8()?)
        } else if b0 < 0xe0 {
            let b = self.take(2)?;
            ((b0 & 0x1f) << 16) | (u32::from(b[0]) << 8) | u32::from(b[1])
        } else if b0 < 0xf0 {
            let b = self.take(3)?;
            ((b0 & 0x0f) << 24) | (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2])
        } else {
            let b = self.take(4)?;
            ((b0 & 0x0f) << 28)
                | (u32::from(b[0]) << 20)
                | (u32::from(b[1]) << 12)
                | (u32::from(b[2]) << 4)
                | (u32::from(b[3]) & 0x0f)
        };
        Ok(v as i32)
    }

    /// LTF8: as ITF8, up to 9 bytes for 64 bits.
    pub(crate) fn ltf8(&mut self) -> DecodeResult<i64> {
        let b0 = self.u8()?;
        let extra = b0.leading_ones() as usize;
        if extra == 0 {
            return Ok(i64::from(b0));
        }
        let bytes = self.take(extra)?;
        let mut v: u64 = if extra >= 8 {
            0
        } else {
            u64::from(b0) & ((1u64 << (7 - extra)) - 1)
        };
        for &b in bytes {
            v = (v << 8) | u64::from(b);
        }
        Ok(v as i64)
    }

    /// uint7: seven bits per byte, most significant group first, top bit set on all but the last.
    pub(crate) fn uint7(&mut self) -> DecodeResult<u64> {
        let mut v: u64 = 0;
        for _ in 0..10 {
            let c = self.u8()?;
            v = (v << 7) | u64::from(c & 0x7f);
            if c < 0x80 {
                return Ok(v);
            }
        }
        Err("uint7 value longer than 10 bytes".into())
    }

    /// A uint7 that must fit in memory-size terms (`usize`) and below `max`.
    pub(crate) fn uint7_len(&mut self, max: usize, what: &str) -> DecodeResult<usize> {
        let v = self.uint7()?;
        usize::try_from(v)
            .ok()
            .filter(|v| *v <= max)
            .ok_or_else(|| format!("{what} of {v} bytes is too large"))
    }

    /// An ITF8 that must be a length (0 ..= max).
    pub(crate) fn itf8_len(&mut self, max: usize, what: &str) -> DecodeResult<usize> {
        let v = self.itf8()?;
        usize::try_from(v)
            .ok()
            .filter(|v| *v <= max)
            .ok_or_else(|| format!("{what} {v} out of range"))
    }
}

/// A most-significant-bit-first bit reader over the core data block.
#[derive(Debug, Clone)]
pub(crate) struct BitReader<'a> {
    buf: &'a [u8],
    byte: usize,
    bit: u8,
}

impl<'a> BitReader<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        BitReader {
            buf,
            byte: 0,
            bit: 0,
        }
    }

    pub(crate) fn bit(&mut self) -> DecodeResult<u32> {
        let b = *self
            .buf
            .get(self.byte)
            .ok_or_else(|| "core data block ends early".to_string())?;
        let v = (b >> (7 - self.bit)) & 1;
        self.bit += 1;
        if self.bit == 8 {
            self.bit = 0;
            self.byte += 1;
        }
        Ok(u32::from(v))
    }

    /// `n` bits (at most 32), most significant first.
    pub(crate) fn bits(&mut self, n: u32) -> DecodeResult<u32> {
        if n > 32 {
            return Err(format!("{n} bits in one value"));
        }
        let mut v: u64 = 0;
        for _ in 0..n {
            v = (v << 1) | u64::from(self.bit()?);
        }
        Ok(v as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variable_integers() {
        assert_eq!(Cursor::new(&[0x7f]).itf8(), Ok(127));
        assert_eq!(Cursor::new(&[0x80, 0xc8]).itf8(), Ok(200));
        assert_eq!(Cursor::new(&[0xe0, 0x45, 0x4f, 0x46]).itf8(), Ok(4_542_278));
        assert_eq!(Cursor::new(&[0xff, 0xff, 0xff, 0xff, 0x0f]).itf8(), Ok(-1));
        assert_eq!(Cursor::new(&[0x05]).ltf8(), Ok(5));
        assert_eq!(Cursor::new(&[0x80, 0xc8]).ltf8(), Ok(200));
        assert_eq!(Cursor::new(&[0x81, 0x00]).uint7(), Ok(128));
        assert!(Cursor::new(&[0x80]).itf8().is_err());
        let mut b = BitReader::new(&[0xb0, 0x70]);
        assert_eq!(b.bits(1), Ok(1));
        assert_eq!(b.bits(1), Ok(0));
        assert_eq!(b.bits(2), Ok(3));
        assert_eq!(b.bits(8), Ok(7));
    }
}
