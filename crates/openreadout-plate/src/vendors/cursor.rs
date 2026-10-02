//! A bounds-checked reader over a byte slice for the binary document parsers. Every read
//! returns `None` past the end; nothing panics on malformed input.

use openreadout_core::bytes;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Cursor<'a> {
    pub(crate) data: &'a [u8],
    pub(crate) pos: usize,
}

macro_rules! num {
    ($be:ident, $le:ident, $t:ty, $n:expr) => {
        #[allow(dead_code)]
        pub(crate) fn $be(&mut self) -> Option<$t> {
            let b = self.take($n)?;
            Some(<$t>::from_be_bytes(b.try_into().ok()?))
        }
        #[allow(dead_code)]
        pub(crate) fn $le(&mut self) -> Option<$t> {
            let b = self.take($n)?;
            Some(<$t>::from_le_bytes(b.try_into().ok()?))
        }
    };
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(data: &'a [u8], pos: usize) -> Self {
        Cursor { data, pos }
    }
    pub(crate) fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }
    pub(crate) fn skip(&mut self, n: usize) -> Option<()> {
        self.take(n).map(|_| ())
    }
    pub(crate) fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    /// The next bytes equal `want` (and are consumed).
    pub(crate) fn expect(&mut self, want: &[u8]) -> Option<()> {
        (self.take(want.len())? == want).then_some(())
    }
    /// The next bytes equal `want` (not consumed).
    pub(crate) fn at(&self, want: &[u8]) -> bool {
        self.data
            .get(self.pos..)
            .is_some_and(|rest| rest.starts_with(want))
    }
    /// A NUL-terminated Latin-1 string of at most `max` bytes.
    pub(crate) fn cstr(&mut self, max: usize) -> Option<String> {
        let rest = self.data.get(self.pos..)?;
        let n = rest.iter().take(max + 1).position(|&b| b == 0)?;
        let s = bytes::latin1(&rest[..n]);
        self.pos += n + 1;
        Some(s)
    }
    num!(u16_be, u16_le, u16, 2);
    num!(u32_be, u32_le, u32, 4);
    num!(i32_be, i32_le, i32, 4);
    num!(u64_be, u64_le, u64, 8);
    num!(f32_be, f32_le, f32, 4);
    num!(f64_be, f64_le, f64, 8);
}

/// Offset of the first occurrence of `needle` in `hay[from..to]`.
pub(crate) fn find(hay: &[u8], needle: &[u8], from: usize, to: usize) -> Option<usize> {
    bytes::find(hay.get(from..to.min(hay.len()))?, needle).map(|p| p + from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_bounds() {
        let d = [0x00, 0x01, 0x41, 0x42, 0x00, 0xff];
        let mut c = Cursor::new(&d, 0);
        assert_eq!(c.u16_be(), Some(1));
        assert_eq!(c.cstr(8).as_deref(), Some("AB"));
        assert_eq!(c.u16_be(), None);
        assert_eq!(c.u8(), Some(0xff));
        assert_eq!(c.u8(), None);
        assert_eq!(find(&d, b"AB", 0, 6), Some(2));
        assert_eq!(find(&d, b"AB", 3, 6), None);
    }
}
