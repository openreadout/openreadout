//! Bounds-checked access to byte buffers read from a file, shared by the binary format readers.
//! Every accessor returns `None` past the end of the buffer, so a short or truncated file never
//! panics.
//!
//! - Scalars at a byte offset: [`le_u32`], [`be_f64`] and so on, or [`Endian::u32`] when the
//!   byte order is only known at run time.
//! - Text: [`latin1`], [`latin1_field`], [`until_nul`], [`utf16le`], [`utf16le_z`] (and the
//!   run-time-order forms [`utf16`] and [`utf16_z`]).
//! - [`Block`]: a buffer addressed by absolute file offsets, filled by [`read_block`].

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::error::{Error, Result};
use crate::source::Fs;

/// The `N` bytes at `at`, if the buffer holds them.
pub fn array<const N: usize>(b: &[u8], at: usize) -> Option<[u8; N]> {
    b.get(at..at.checked_add(N)?)?.try_into().ok()
}

macro_rules! scalar_readers {
    ($($t:ident: $le:ident, $be:ident;)*) => {
        $(
            #[doc = concat!("Little-endian `", stringify!($t), "` at byte offset `at`.")]
            pub fn $le(b: &[u8], at: usize) -> Option<$t> {
                array(b, at).map(<$t>::from_le_bytes)
            }
            #[doc = concat!("Big-endian `", stringify!($t), "` at byte offset `at`.")]
            pub fn $be(b: &[u8], at: usize) -> Option<$t> {
                array(b, at).map(<$t>::from_be_bytes)
            }
        )*

        impl Endian {
            $(
                #[doc = concat!("`", stringify!($t), "` at byte offset `at` in this byte order.")]
                pub fn $t(self, b: &[u8], at: usize) -> Option<$t> {
                    match self {
                        Endian::Little => $le(b, at),
                        Endian::Big => $be(b, at),
                    }
                }
            )*
        }
    };
}

/// Byte order of multi-byte values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endian {
    /// Least significant byte first.
    Little,
    /// Most significant byte first.
    Big,
}

scalar_readers! {
    u16: le_u16, be_u16;
    i16: le_i16, be_i16;
    u32: le_u32, be_u32;
    i32: le_i32, be_i32;
    u64: le_u64, be_u64;
    i64: le_i64, be_i64;
    f32: le_f32, be_f32;
    f64: le_f64, be_f64;
}

/// The bytes before the first NUL (all of `b` when there is none).
pub fn until_nul(b: &[u8]) -> &[u8] {
    b.iter().position(|&c| c == 0).map_or(b, |end| &b[..end])
}

/// Every byte as the Latin-1 character of the same value (no cut, no trim).
pub fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| char::from(c)).collect()
}

/// Windows-1252 code points for bytes 0x80..=0x9F (0 = undefined, kept as the C1 control).
const CP1252_HIGH: [u16; 32] = [
    0x20AC, 0, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0, 0x017D, 0, 0, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC,
    0x2122, 0x0161, 0x203A, 0x0153, 0, 0x017E, 0x0178,
];

/// Windows-1252 text (no cut, no trim); the five undefined bytes stay C1 controls.
pub fn windows1252(b: &[u8]) -> String {
    b.iter()
        .map(
            |&c| match CP1252_HIGH.get(usize::from(c.wrapping_sub(0x80))) {
                Some(&cp) if cp != 0 => char::from_u32(u32::from(cp)).unwrap_or(char::from(c)),
                _ => char::from(c),
            },
        )
        .collect()
}

/// The UTF-16 code units of `b` in byte order `e`; a trailing odd byte is ignored.
pub fn utf16_units(b: &[u8], e: Endian) -> impl Iterator<Item = u16> + '_ {
    b.as_chunks::<2>().0.iter().map(move |c| match e {
        Endian::Little => u16::from_le_bytes(*c),
        Endian::Big => u16::from_be_bytes(*c),
    })
}

/// UTF-16 text in byte order `e`, every code unit kept (NULs included); unpaired surrogates
/// become U+FFFD.
pub fn utf16(b: &[u8], e: Endian) -> String {
    char::decode_utf16(utf16_units(b, e))
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// UTF-16 text in byte order `e` up to the first NUL code unit; unpaired surrogates become
/// U+FFFD.
pub fn utf16_z(b: &[u8], e: Endian) -> String {
    char::decode_utf16(utf16_units(b, e).take_while(|&u| u != 0))
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// [`utf16`] for little-endian text.
pub fn utf16le(b: &[u8]) -> String {
    utf16(b, Endian::Little)
}

/// [`utf16_z`] for little-endian text.
pub fn utf16le_z(b: &[u8]) -> String {
    utf16_z(b, Endian::Little)
}

/// Position of the first occurrence of `needle` in `hay` (`None` for an empty needle).
pub fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// A byte buffer read from `origin` in a file, addressed by absolute file offsets.
#[derive(Debug, Clone, Default)]
pub struct Block {
    /// File offset of `bytes[0]`.
    pub origin: u64,
    /// The bytes read.
    pub bytes: Vec<u8>,
}

macro_rules! le_getter {
    ($name:ident, $t:ty) => {
        #[doc = concat!("Little-endian `", stringify!($t), "` at absolute offset `at`.")]
        pub fn $name(&self, at: u64) -> Option<$t> {
            self.slice(at, size_of::<$t>())
                .and_then(|b| array(b, 0))
                .map(<$t>::from_le_bytes)
        }
    };
}

impl Block {
    /// `n` bytes at absolute offset `at`, if held.
    pub fn slice(&self, at: u64, n: usize) -> Option<&[u8]> {
        let rel = usize::try_from(at.checked_sub(self.origin)?).ok()?;
        self.bytes.get(rel..rel.checked_add(n)?)
    }
    le_getter!(u8_at, u8);
    le_getter!(i8_at, i8);
    le_getter!(u16_at, u16);
    le_getter!(i16_at, i16);
    le_getter!(u32_at, u32);
    le_getter!(i32_at, i32);
    le_getter!(u64_at, u64);
    le_getter!(i64_at, i64);
    le_getter!(f32_at, f32);
    le_getter!(f64_at, f64);
    /// A fixed-width 8-bit text field: Latin-1, cut at the first NUL, trimmed.
    pub fn text_at(&self, at: u64, n: usize) -> Option<String> {
        self.slice(at, n).map(latin1_field)
    }
    /// One past the last byte held.
    pub fn end(&self) -> u64 {
        self.origin + self.bytes.len() as u64
    }
    /// Number of bytes held.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }
    /// True when no bytes are held.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

/// Decode an 8-bit text field: Latin-1, stop at the first NUL, trim whitespace.
pub fn latin1_field(b: &[u8]) -> String {
    latin1(until_nul(b)).trim().to_string()
}

/// Read a whole (small) file such as a sidecar or companion document from `fs`, refusing one
/// larger than `limit` bytes instead of loading it: the read itself stops after `limit + 1`
/// bytes, so a file that grows, or a path that is not what its name suggests, cannot exhaust
/// memory. `format` and `what` name the file in the error (exit 6, with a hint).
pub fn read_file_capped_in(
    fs: &Fs,
    path: &Path,
    limit: u64,
    format: &'static str,
    what: &str,
) -> Result<Vec<u8>> {
    let f = fs.open(path).map_err(|e| Error::io(path, e))?;
    let mut bytes = Vec::new();
    f.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| Error::io(path, e))?;
    if bytes.len() as u64 > limit {
        return Err(Error::unsupported(
            format,
            format!("{what} larger than {limit} bytes"),
            format!(
                "{} is too large to be a {what}; check that it is the right file.",
                path.display()
            ),
        ));
    }
    Ok(bytes)
}

/// Read up to `len` bytes at `offset` (fewer at the end of the file), without knowing the file
/// length.
pub fn read_range<R: Read + Seek + ?Sized>(
    f: &mut R,
    path: &Path,
    offset: u64,
    len: u64,
) -> Result<Vec<u8>> {
    f.seek(SeekFrom::Start(offset))
        .map_err(|e| Error::io(path, e))?;
    let cap = usize::try_from(len).map_err(|_| Error::Other("read too large".into()))?;
    let mut buf = Vec::with_capacity(cap.min(1 << 26));
    f.take(len)
        .read_to_end(&mut buf)
        .map_err(|e| Error::io(path, e))?;
    Ok(buf)
}

/// Read up to `len` bytes at `offset`; fewer at the end of the file, none past it. `f` is a
/// `std::fs::File` or a [`crate::source::SourceFile`].
pub fn read_block<R: Read + Seek + ?Sized>(
    f: &mut R,
    path: &Path,
    offset: u64,
    len: u64,
    file_len: u64,
) -> Result<Block> {
    if offset >= file_len {
        return Ok(Block {
            origin: offset,
            bytes: Vec::new(),
        });
    }
    let n = len.min(file_len - offset);
    let n = usize::try_from(n).map_err(|_| Error::Other("block too large for memory".into()))?;
    let mut bytes = vec![0u8; n];
    f.seek(SeekFrom::Start(offset))
        .map_err(|e| Error::io(path, e))?;
    f.read_exact(&mut bytes).map_err(|e| Error::io(path, e))?;
    Ok(Block {
        origin: offset,
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds() {
        let b = Block {
            origin: 4,
            bytes: vec![1, 0, 0, 0, 0xB5, b'V', 0, 9],
        };
        assert_eq!(b.u32_at(4), Some(1));
        assert_eq!(b.u32_at(3), None);
        assert_eq!(b.u32_at(9), None);
        assert_eq!(b.text_at(8, 4).as_deref(), Some("µV"));
        assert_eq!(b.i8_at(11), Some(9));
        assert_eq!(b.end(), 12);
    }

    #[test]
    fn scalars_and_text() {
        let b = [0x01, 0x02, 0x03, 0x04, b'h', 0, b'i', 0, 0, 0, b'x', 0];
        assert_eq!(le_u32(&b, 0), Some(0x0403_0201));
        assert_eq!(be_u32(&b, 0), Some(0x0102_0304));
        assert_eq!(Endian::Big.u16(&b, 0), Some(0x0102));
        assert_eq!(le_u32(&b, 9), None);
        assert_eq!(le_u32(&b, usize::MAX), None);
        assert_eq!(utf16le_z(&b[4..]), "hi");
        assert_eq!(utf16le(&b[4..10]), "hi\0");
        assert_eq!(until_nul(b"ab\0c"), b"ab");
        assert_eq!(latin1(&[0xB5, b'V']), "\u{b5}V");
        assert_eq!(
            windows1252(&[0x80, 0x81, 0x9F, 0xE9, b'a']),
            "\u{20ac}\u{81}\u{178}\u{e9}a"
        );
        assert_eq!(find(b"abcabc", b"ca"), Some(2));
        assert_eq!(find(b"abc", b""), None);
    }

    #[test]
    fn capped_read_refuses_large_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.meta");
        std::fs::write(&p, [7u8; 100]).unwrap();
        assert_eq!(
            read_file_capped_in(&Fs::local(), &p, 100, "test", "sidecar")
                .unwrap()
                .len(),
            100
        );
        let e = read_file_capped_in(&Fs::local(), &p, 99, "test", "sidecar").unwrap_err();
        assert!(
            e.to_string().contains("sidecar larger than 99 bytes"),
            "{e}"
        );
    }
}
