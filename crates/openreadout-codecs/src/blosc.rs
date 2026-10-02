//! Blosc 1 chunk decoding and LZ4 block decoding (pure Rust).
//!
//! Blosc chunk layout from the public c-blosc documentation (`README_CHUNK_FORMAT.rst`) and the
//! BSD-3-licensed c-blosc sources (`blosc.c` block splitting rules, `blosclz.c` decoder,
//! `shuffle.c` bit-unshuffle rules); the bit transpose from the MIT-licensed bitshuffle
//! library's description; Snappy from its public format description; LZ4 blocks through the
//! MIT-licensed `lz4_flex` crate. Provenance: `docs/provenance/ome-zarr.md`.
//!
//! Every length in the input is checked before use; malformed input is an error, never a panic.

use crate::{CodecError, Result};

/// Byte-shuffle filter applied (flags bit 0).
const FLAG_SHUFFLE: u8 = 0x01;
/// The payload is stored uncompressed after the header (flags bit 1).
const FLAG_MEMCPYED: u8 = 0x02;
/// Bit-shuffle filter applied (flags bit 2).
const FLAG_BITSHUFFLE: u8 = 0x04;
/// Blocks are not split into one stream per byte of the type (flags bit 4).
const FLAG_DONT_SPLIT: u8 = 0x10;
const HEADER: usize = 16;
const MAX_SPLITS: usize = 16;
const MIN_BUFFERSIZE: usize = 128;
/// Largest decoded chunk accepted (a guard against absurd headers).
const MAX_DECODED: usize = 1 << 31;

fn err(detail: impl Into<String>) -> CodecError {
    CodecError::Decode {
        codec: "blosc",
        detail: detail.into(),
    }
}

fn le32(b: &[u8], at: usize) -> Option<usize> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize)
}

/// The compressor recorded in a Blosc header (flags bits 5–7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BloscCompressor {
    /// Blosc's own LZ77 variant.
    BloscLz,
    /// LZ4 block format.
    Lz4,
    /// Snappy.
    Snappy,
    /// zlib (deflate with a zlib header).
    Zlib,
    /// Zstandard.
    Zstd,
    /// A compressor code this decoder does not know.
    Unknown(u8),
}

impl BloscCompressor {
    fn from_flags(flags: u8) -> Self {
        match flags >> 5 {
            0 => Self::BloscLz,
            1 => Self::Lz4,
            2 => Self::Snappy,
            3 => Self::Zlib,
            4 => Self::Zstd,
            n => Self::Unknown(n),
        }
    }
    /// Name as numcodecs spells it (`cname`).
    pub fn name(self) -> &'static str {
        match self {
            Self::BloscLz => "blosclz",
            Self::Lz4 => "lz4",
            Self::Snappy => "snappy",
            Self::Zlib => "zlib",
            Self::Zstd => "zstd",
            Self::Unknown(_) => "unknown",
        }
    }
}

/// Header fields of a Blosc 1 chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BloscHeader {
    /// Format version of the Blosc header.
    pub version: u8,
    /// Flag bits: shuffle mode (bits 0 and 2), memcpy'd (bit 1), compressor (bits 5 to 7).
    pub flags: u8,
    /// Size of one element in bytes, used by the shuffle filters.
    pub typesize: usize,
    /// Decoded size in bytes.
    pub nbytes: usize,
    /// Uncompressed size of one block in bytes.
    pub blocksize: usize,
    /// Compressed size including the header.
    pub cbytes: usize,
    /// The compressor that produced the blocks.
    pub compressor: BloscCompressor,
}

/// Parse the 16-byte Blosc header.
pub fn blosc_header(data: &[u8]) -> Result<BloscHeader> {
    if data.len() < HEADER {
        return Err(err(format!(
            "{} bytes is shorter than the 16-byte header",
            data.len()
        )));
    }
    let flags = data[2];
    Ok(BloscHeader {
        version: data[0],
        flags,
        typesize: data[3] as usize,
        nbytes: le32(data, 4).unwrap_or(0),
        blocksize: le32(data, 8).unwrap_or(0),
        cbytes: le32(data, 12).unwrap_or(0),
        compressor: BloscCompressor::from_flags(flags),
    })
}

/// Decode one Blosc 1 chunk (as written by c-blosc 1.x / numcodecs `Blosc`).
pub fn blosc_decode(data: &[u8]) -> Result<Vec<u8>> {
    let h = blosc_header(data)?;
    if h.nbytes > MAX_DECODED {
        return Err(err(format!("header declares {} decoded bytes", h.nbytes)));
    }
    if h.cbytes > data.len() {
        return Err(err(format!(
            "header declares {} compressed bytes, {} present (truncated)",
            h.cbytes,
            data.len()
        )));
    }
    let data = &data[..h.cbytes];
    if h.nbytes == 0 {
        return Ok(Vec::new());
    }
    if h.flags & FLAG_MEMCPYED != 0 {
        let body = data
            .get(HEADER..HEADER + h.nbytes)
            .ok_or_else(|| err("stored (memcpyed) payload is truncated"))?;
        return Ok(body.to_vec());
    }
    if h.blocksize == 0 || h.typesize == 0 {
        return Err(err("block size or type size is 0"));
    }
    if h.blocksize > MAX_DECODED {
        return Err(err(format!("header declares a {}-byte block", h.blocksize)));
    }
    let nblocks = h.nbytes.div_ceil(h.blocksize);
    let leftover = h.nbytes % h.blocksize;
    let mut out = vec![0u8; h.nbytes];
    let mut tmp = vec![0u8; h.blocksize.min(h.nbytes)];
    let shuffle = h.flags & FLAG_SHUFFLE != 0 && h.typesize > 1;
    let bitshuffle = h.flags & FLAG_BITSHUFFLE != 0 && h.blocksize >= h.typesize;
    for b in 0..nblocks {
        let start = le32(data, HEADER + 4 * b).ok_or_else(|| err("block offsets truncated"))?;
        let is_leftover = leftover != 0 && b == nblocks - 1;
        let bsize = if is_leftover { leftover } else { h.blocksize };
        let nsplits = if h.flags & FLAG_DONT_SPLIT == 0
            && h.typesize <= MAX_SPLITS
            && h.blocksize / h.typesize >= MIN_BUFFERSIZE
            && !is_leftover
        {
            h.typesize
        } else {
            1
        };
        let neblock = bsize / nsplits;
        let mut src = start;
        let mut filled = 0usize;
        for _ in 0..nsplits {
            let cbytes = le32(data, src).ok_or_else(|| err("split size truncated"))?;
            src += 4;
            let end = src
                .checked_add(cbytes)
                .ok_or_else(|| err("split size overflows"))?;
            let stream = data
                .get(src..end)
                .ok_or_else(|| err(format!("block {b}: split runs past the chunk")))?;
            let dst = tmp
                .get_mut(filled..filled + neblock)
                .ok_or_else(|| err("block larger than declared"))?;
            if cbytes == neblock {
                dst.copy_from_slice(stream);
            } else {
                let dec = decompress_stream(h.compressor, stream, neblock)?;
                if dec.len() != neblock {
                    return Err(err(format!(
                        "block {b}: split decoded to {} bytes, expected {neblock}",
                        dec.len()
                    )));
                }
                dst.copy_from_slice(&dec);
            }
            src = end;
            filled += neblock;
        }
        let dest = out
            .get_mut(b * h.blocksize..b * h.blocksize + bsize)
            .ok_or_else(|| err("block outside the decoded chunk"))?;
        if shuffle {
            unshuffle(h.typesize, &tmp[..bsize], dest);
        } else if bitshuffle {
            bitunshuffle(h.typesize, &tmp[..bsize], dest);
        } else {
            dest.copy_from_slice(&tmp[..bsize]);
        }
    }
    Ok(out)
}

/// Undo the Blosc byte shuffle: byte `j` of element `i` was stored at `j * n + i`; trailing
/// bytes that do not fill a whole element are stored as they are.
fn unshuffle(typesize: usize, src: &[u8], dest: &mut [u8]) {
    let n = src.len() / typesize;
    for i in 0..n {
        for j in 0..typesize {
            dest[i * typesize + j] = src[j * n + i];
        }
    }
    let done = n * typesize;
    dest[done..].copy_from_slice(&src[done..]);
}

/// Undo the bit shuffle of one block (c-blosc `bitunshuffle`): when the block holds a multiple
/// of 8 whole elements, bit `k` of byte `j` of element `i` was stored at bit `i % 8` of byte
/// `(j * 8 + k) * n / 8 + i / 8` (`n` elements; one row of `n` bits per bit of each byte);
/// bytes after the whole elements are stored as they are. Other blocks are stored unshuffled.
fn bitunshuffle(typesize: usize, src: &[u8], dest: &mut [u8]) {
    let n = src.len() / typesize;
    if !n.is_multiple_of(8) {
        dest.copy_from_slice(src);
        return;
    }
    let row = n / 8;
    let done = n * typesize;
    dest[..done].fill(0);
    for j in 0..typesize {
        for k in 0..8 {
            let bits = &src[(j * 8 + k) * row..(j * 8 + k + 1) * row];
            for i in 0..n {
                let bit = (bits[i / 8] >> (i % 8)) & 1;
                dest[i * typesize + j] |= bit << k;
            }
        }
    }
    dest[done..].copy_from_slice(&src[done..]);
}

/// A Snappy raw block (public format description, `format_description.txt`): a varint decoded
/// length, then literals and back references (1, 2 or 4-byte offsets). Must decode to exactly
/// `expected` bytes.
pub fn snappy_decode(src: &[u8], expected: usize) -> Result<Vec<u8>> {
    let bad = |d: &str| CodecError::Decode {
        codec: "snappy",
        detail: d.into(),
    };
    let mut ip = 0usize;
    let mut len = 0usize;
    let mut shift = 0u32;
    loop {
        let b = *src.get(ip).ok_or_else(|| bad("length truncated"))?;
        ip += 1;
        len |= usize::from(b & 0x7f)
            .checked_shl(shift)
            .ok_or_else(|| bad("length overflows"))?;
        if b & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift > 28 {
            return Err(bad("length overflows"));
        }
    }
    if len != expected || len > MAX_DECODED {
        return Err(bad(&format!("declares {len} bytes, expected {expected}")));
    }
    let mut out: Vec<u8> = Vec::with_capacity(len);
    let le = |at: usize, n: usize| -> Option<usize> {
        let s = src.get(at..at.checked_add(n)?)?;
        Some(s.iter().rev().fold(0usize, |a, &b| a << 8 | usize::from(b)))
    };
    while ip < src.len() {
        let tag = src[ip];
        ip += 1;
        let (n, offset) = match tag & 3 {
            0 => {
                let mut n = usize::from(tag >> 2);
                if n >= 60 {
                    let extra = n - 59;
                    n = le(ip, extra).ok_or_else(|| bad("literal length truncated"))?;
                    ip += extra;
                }
                n += 1;
                let lit = src
                    .get(ip..ip.checked_add(n).ok_or_else(|| bad("literal overflows"))?)
                    .ok_or_else(|| bad("literal runs past the input"))?;
                if out.len() + n > len {
                    return Err(bad("literal runs past the declared length"));
                }
                out.extend_from_slice(lit);
                ip += n;
                continue;
            }
            1 => {
                let o = *src.get(ip).ok_or_else(|| bad("copy truncated"))?;
                ip += 1;
                (
                    4 + usize::from((tag >> 2) & 7),
                    usize::from(tag >> 5) << 8 | usize::from(o),
                )
            }
            2 => {
                let o = le(ip, 2).ok_or_else(|| bad("copy truncated"))?;
                ip += 2;
                (1 + usize::from(tag >> 2), o)
            }
            _ => {
                let o = le(ip, 4).ok_or_else(|| bad("copy truncated"))?;
                ip += 4;
                (1 + usize::from(tag >> 2), o)
            }
        };
        if offset == 0 || offset > out.len() || out.len() + n > len {
            return Err(bad("back reference outside the output"));
        }
        let from = out.len() - offset;
        for k in 0..n {
            let v = out[from + k];
            out.push(v);
        }
    }
    if out.len() != len {
        return Err(bad(&format!("decoded {} of {len} bytes", out.len())));
    }
    Ok(out)
}

fn decompress_stream(c: BloscCompressor, src: &[u8], expected: usize) -> Result<Vec<u8>> {
    match c {
        BloscCompressor::BloscLz => blosclz_decode(src, expected),
        BloscCompressor::Lz4 => lz4_block_decode(src, expected),
        BloscCompressor::Snappy => snappy_decode(src, expected),
        BloscCompressor::Zlib => crate::zlib_decode(src, expected),
        BloscCompressor::Zstd => crate::zstd_decode(src, expected),
        BloscCompressor::Unknown(_) => Err(err(format!(
            "the {} compressor inside Blosc is not decoded",
            c.name()
        ))),
    }
}

/// BloscLZ (the default Blosc compressor), following `blosclz_decompress` in c-blosc (BSD-3).
pub fn blosclz_decode(src: &[u8], maxout: usize) -> Result<Vec<u8>> {
    const MAX_DISTANCE: usize = 8191;
    let bad = || err("malformed BloscLZ stream");
    let mut out: Vec<u8> = Vec::with_capacity(maxout);
    if src.is_empty() {
        return Ok(out);
    }
    let mut ip = 0usize;
    let mut ctrl = usize::from(src[ip] & 31);
    ip += 1;
    loop {
        if ctrl >= 32 {
            let mut len = (ctrl >> 5) - 1;
            let ofs = (ctrl & 31) << 8;
            if len == 6 {
                loop {
                    if ip + 1 >= src.len() {
                        return Err(bad());
                    }
                    let code = src[ip];
                    ip += 1;
                    len += usize::from(code);
                    if code != 255 {
                        break;
                    }
                }
            } else if ip + 1 >= src.len() {
                return Err(bad());
            }
            let code = usize::from(src[ip]);
            ip += 1;
            len += 3;
            // distance back from the output position, minus one (see `ref--` in C)
            let mut back = ofs + code;
            if code == 255 && ofs == (31 << 8) {
                if ip + 1 >= src.len() {
                    return Err(bad());
                }
                let far = (usize::from(src[ip]) << 8) + usize::from(src[ip + 1]);
                ip += 2;
                back = far + MAX_DISTANCE;
            }
            back += 1;
            if out.len() + len > maxout || back > out.len() {
                return Err(bad());
            }
            let from = out.len() - back;
            for k in 0..len {
                let v = out[from + k];
                out.push(v);
            }
            if ip >= src.len() {
                break;
            }
            ctrl = usize::from(src[ip]);
            ip += 1;
        } else {
            let n = ctrl + 1;
            if out.len() + n > maxout || ip + n > src.len() {
                return Err(bad());
            }
            out.extend_from_slice(&src[ip..ip + n]);
            ip += n;
            if ip >= src.len() {
                break;
            }
            ctrl = usize::from(src[ip]);
            ip += 1;
        }
    }
    Ok(out)
}

/// One raw LZ4 block (no frame, no size prefix) that decodes to exactly `expected` bytes.
pub fn lz4_block_decode(src: &[u8], expected: usize) -> Result<Vec<u8>> {
    #[cfg(feature = "lz4")]
    {
        lz4_flex::block::decompress(src, expected).map_err(|e| CodecError::Decode {
            codec: "lz4",
            detail: e.to_string(),
        })
    }
    #[cfg(not(feature = "lz4"))]
    {
        let _ = (src, expected);
        Err(CodecError::NotCompiled {
            codec: "lz4",
            feature: "lz4",
        })
    }
}

/// numcodecs `LZ4`: a little-endian u32 decoded size, then one LZ4 block.
pub fn lz4_sized_decode(src: &[u8]) -> Result<Vec<u8>> {
    let n = le32(src, 0).ok_or_else(|| CodecError::Decode {
        codec: "lz4",
        detail: "missing 4-byte size prefix".into(),
    })?;
    if n > MAX_DECODED {
        return Err(CodecError::Decode {
            codec: "lz4",
            detail: format!("size prefix declares {n} bytes"),
        });
    }
    lz4_block_decode(&src[4..], n)
}

/// The HDF5 LZ4 filter (registered filter id 32004): a big-endian u64 decoded size and a
/// big-endian u32 block size, then per block a big-endian u32 compressed size and the LZ4
/// block (stored as is when the compressed size equals the block's decoded size).
pub fn hdf5_lz4_decode(src: &[u8]) -> Result<Vec<u8>> {
    let e = |d: &str| CodecError::Decode {
        codec: "hdf5-lz4",
        detail: d.into(),
    };
    let be = |at: usize, n: usize| -> Option<u64> {
        let s = src.get(at..at.checked_add(n)?)?;
        Some(s.iter().fold(0u64, |a, &b| a << 8 | u64::from(b)))
    };
    let total = usize::try_from(be(0, 8).ok_or_else(|| e("header truncated"))?)
        .map_err(|_| e("declared size too large"))?;
    let block = be(8, 4).ok_or_else(|| e("header truncated"))? as usize;
    if total > MAX_DECODED {
        return Err(e("declared size too large"));
    }
    if block == 0 && total > 0 {
        return Err(e("block size 0"));
    }
    let mut out = Vec::with_capacity(total);
    let mut at = 12usize;
    while out.len() < total {
        let want = block.min(total - out.len());
        let c = be(at, 4).ok_or_else(|| e("block header truncated"))? as usize;
        at += 4;
        let end = at.checked_add(c).ok_or_else(|| e("block size overflows"))?;
        let s = src
            .get(at..end)
            .ok_or_else(|| e("block runs past the chunk"))?;
        if c == want {
            out.extend_from_slice(s);
        } else {
            out.extend_from_slice(&lz4_block_decode(s, want)?);
        }
        at = end;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memcpyed_and_shuffled_stored_blocks() {
        // stored payload
        let mut c = vec![
            2u8,
            1,
            FLAG_MEMCPYED,
            2,
            4,
            0,
            0,
            0,
            4,
            0,
            0,
            0,
            20,
            0,
            0,
            0,
        ];
        c.extend_from_slice(&[1, 2, 3, 4]);
        assert_eq!(blosc_decode(&c).unwrap(), vec![1, 2, 3, 4]);
        // one block of 4 u16 values, shuffled, split size == neblock (stored raw)
        let mut c = vec![
            2u8,
            1,
            FLAG_SHUFFLE | FLAG_DONT_SPLIT,
            2,
            8,
            0,
            0,
            0,
            8,
            0,
            0,
            0,
        ];
        let body_start = 16 + 4;
        c.extend_from_slice(&((body_start + 4 + 8) as u32).to_le_bytes());
        c.extend_from_slice(&(body_start as u32).to_le_bytes());
        c.extend_from_slice(&8u32.to_le_bytes());
        // values 0x0201, 0x0403, 0x0605, 0x0807 shuffled: low bytes then high bytes
        c.extend_from_slice(&[1, 3, 5, 7, 2, 4, 6, 8]);
        assert_eq!(blosc_decode(&c).unwrap(), vec![1, 2, 3, 4, 5, 6, 7, 8]);
        // truncated
        assert!(blosc_decode(&c[..c.len() - 3]).is_err());
        assert!(blosc_decode(&c[..10]).is_err());
    }

    #[test]
    fn bitunshuffle_transposes_bits() {
        // 8 u16 elements 0x0001, 0x0002, ..., 0x0008: bit rows of 8 bits (1 byte) each
        let vals: Vec<u16> = (1..=8).collect();
        let mut shuffled = vec![0u8; 16];
        for (i, v) in vals.iter().enumerate() {
            for j in 0..2 {
                let byte = v.to_le_bytes()[j];
                for k in 0..8 {
                    if byte >> k & 1 == 1 {
                        shuffled[j * 8 + k] |= 1 << i;
                    }
                }
            }
        }
        let mut out = vec![0u8; 16];
        bitunshuffle(2, &shuffled, &mut out);
        let back: Vec<u16> = out
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(back, vals);
        // not a multiple of 8 elements: stored as it is
        let mut out = vec![0u8; 6];
        bitunshuffle(2, &[1, 2, 3, 4, 5, 6], &mut out);
        assert_eq!(out, [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn snappy_literals_and_copies() {
        // "abcabcabcd": literal "abc", copy len 6 offset 3 (1-byte offset form), literal "d"
        let src = [10u8, 0x08, b'a', b'b', b'c', 0x09, 3, 0x00, b'd'];
        assert_eq!(snappy_decode(&src, 10).unwrap(), b"abcabcabcd");
        assert!(snappy_decode(&src, 9).is_err());
        assert!(snappy_decode(&src[..6], 10).is_err());
        assert!(snappy_decode(&[4, 0x05, 9, 0], 4).is_err());
    }

    #[test]
    fn blosclz_literal_and_match() {
        // literal run of 3 ("abc"), a match of length 3 at distance 3, a literal "d"
        let src = [2u8, b'a', b'b', b'c', 0x20, 2, 0, b'd'];
        assert_eq!(blosclz_decode(&src, 7).unwrap(), b"abcabcd");
        assert!(blosclz_decode(&[0, b'x', 0x20, 5, 0, b'y'], 8).is_err());
    }

    #[test]
    fn hdf5_lz4_stored_block() {
        let mut s = Vec::new();
        s.extend_from_slice(&4u64.to_be_bytes());
        s.extend_from_slice(&4u32.to_be_bytes());
        s.extend_from_slice(&4u32.to_be_bytes());
        s.extend_from_slice(&[9, 8, 7, 6]);
        assert_eq!(hdf5_lz4_decode(&s).unwrap(), vec![9, 8, 7, 6]);
        assert!(hdf5_lz4_decode(&s[..14]).is_err());
    }
}
