//! MSB-first bit reader over one packet of the codestream (jxrlib `BitIOInfo`, `SimpleBitIO`).
//!
//! jxrlib reads packets through a circular buffer; that is an implementation detail of a linear
//! MSB-first stream, which is what this is. Reads past the end of the data return zero bits and
//! are counted, so a truncated stream decodes to a clean error instead of reading other memory.

/// Bits a reader may run past the end of its data before the decoder gives up. A valid stream
/// peeks at most 32 bits ahead of what it consumes, so real overruns are always larger.
const OVERRUN_SLACK_BITS: u64 = 64;

#[derive(Debug, Clone)]
pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    /// Bit position from the start of `data`.
    pos: u64,
}

impl<'a> BitReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0 }
    }

    /// 64 bits starting at the byte holding bit `pos`, big-endian, zero past the end.
    #[inline]
    fn window(&self) -> u64 {
        let byte = usize::try_from(self.pos >> 3).unwrap_or(usize::MAX);
        if let Some(b) = byte.checked_add(8).and_then(|end| self.data.get(byte..end)) {
            let mut a = [0u8; 8];
            a.copy_from_slice(b);
            return u64::from_be_bytes(a);
        }
        let mut v = 0u64;
        for i in 0..8 {
            let b = byte
                .checked_add(i)
                .and_then(|j| self.data.get(j))
                .copied()
                .unwrap_or(0);
            v = (v << 8) | u64::from(b);
        }
        v
    }

    /// The next `n` bits (0..=32) without consuming them.
    #[inline]
    pub(crate) fn peek(&self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let sh = (self.pos & 7) as u32;
        ((self.window() << sh) >> (64 - n)) as u32
    }

    #[inline]
    pub(crate) fn flush(&mut self, n: u32) {
        self.pos = self.pos.saturating_add(u64::from(n));
    }

    /// `n` bits (0..=32), MSB first.
    #[inline]
    pub(crate) fn get(&mut self, n: u32) -> u32 {
        let v = self.peek(n);
        self.flush(n);
        v
    }

    #[inline]
    pub(crate) fn get_bool(&mut self) -> bool {
        self.get(1) != 0
    }

    /// One bit as a sign mask: 0 or -1 (jxrlib `_getSign`).
    #[inline]
    pub(crate) fn get_sign(&mut self) -> i32 {
        -(self.get(1) as i32)
    }

    /// jxrlib `_getBit16s`: `n` magnitude bits followed by a sign bit that is only present when
    /// the magnitude is nonzero.
    #[inline]
    pub(crate) fn get_signed(&mut self, n: u32) -> i32 {
        let r = self.peek(n + 1) as i32;
        let v = ((r >> 1) ^ (-(r & 1))).wrapping_add(r & 1);
        self.flush(n + u32::from(v != 0));
        v
    }

    /// Skip to the next byte boundary (jxrlib `flushToByte`).
    pub(crate) fn align(&mut self) {
        self.pos = (self.pos + 7) & !7;
    }

    /// Byte offset of the next unread byte (after `align`).
    pub(crate) fn byte_pos(&self) -> u64 {
        self.pos.div_ceil(8)
    }

    /// True once the reader has consumed well past the end of its data.
    pub(crate) fn overrun(&self) -> bool {
        self.pos > (self.data.len() as u64) * 8 + OVERRUN_SLACK_BITS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msb_first_and_padding() {
        let mut r = BitReader::new(&[0b1010_0000, 0xFF]);
        assert_eq!(r.get(1), 1);
        assert_eq!(r.get(3), 0b010);
        assert_eq!(r.get(8), 0b0000_1111);
        assert_eq!(r.get(8), 0b1111_0000);
        assert!(!r.overrun());
        r.flush(200);
        assert!(r.overrun());
        assert_eq!(r.get(32), 0);
    }

    #[test]
    fn signed_values() {
        // magnitude 3 (2 bits) then sign 1 -> -3; magnitude 0 consumes no sign bit.
        let mut r = BitReader::new(&[0b1110_0100]);
        assert_eq!(r.get_signed(2), -3);
        assert_eq!(r.get_signed(2), 0);
        assert_eq!(r.get(3), 0b100);
    }
}
