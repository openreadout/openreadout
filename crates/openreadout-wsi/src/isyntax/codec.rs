//! The iSyntax code-block codec (compressor 16): one block of wavelet coefficients, stored as bit
//! planes that are Huffman coded with runs of zero bytes. A port of libisyntax's decoder
//! (BSD-2-Clause, see `NOTICE`); layout in `docs/formats/philips-isyntax.md` § Code blocks.

/// Bits of the direct Huffman lookup table; longer codes are matched one by one.
const FAST_BITS: u32 = 11;
/// Longest Huffman code accepted (the slow matcher compares 16-bit prefixes).
const MAX_CODE_BITS: u32 = 16;

/// Why a code block could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecError(pub String);

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn err(msg: impl Into<String>) -> CodecError {
    CodecError(msg.into())
}

/// Up to 64 bits of `data` starting at bit `pos`, least significant bit first; bytes past the
/// end read as zero.
fn peek(data: &[u8], pos: u64) -> u64 {
    let byte = usize::try_from(pos / 8).unwrap_or(usize::MAX);
    let mut b = [0u8; 8];
    if let Some(src) = data.get(byte..) {
        let n = src.len().min(8);
        b[..n].copy_from_slice(&src[..n]);
    }
    u64::from_le_bytes(b) >> (pos % 8)
}

fn le_u16(data: &[u8], at: usize) -> Result<u16, CodecError> {
    data.get(at..at + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or_else(|| err("code block too short for its header"))
}

/// A Huffman table read from the block: symbol codes, a direct lookup for short codes and a
/// list for the others.
struct Huffman {
    code: [u32; 256],
    size: [u32; 256],
    /// Short-code lookup: the symbol, or `None` when the code is longer than `FAST_BITS`.
    fast: Vec<Option<u8>>,
    /// Codes longer than `FAST_BITS`: (code, size, symbol).
    slow: Vec<(u32, u32, u8)>,
}

impl Huffman {
    /// Read the tree, stored as a depth-first walk: a 0 bit descends, a 1 bit is a leaf
    /// followed by its 8-bit symbol; after a leaf the walk climbs to the last left branch and
    /// takes the right one.
    fn read(data: &[u8], bits_read: &mut u64, total_bits: u64) -> Result<Huffman, CodecError> {
        let mut h = Huffman {
            code: [0; 256],
            size: [0; 256],
            fast: vec![None; 1 << FAST_BITS],
            slow: Vec::new(),
        };
        let mut code: u32 = 0;
        let mut code_size: u32 = 0;
        let mut leaves = 0u32;
        loop {
            if *bits_read >= total_bits {
                return Err(err("the Huffman table runs past the end of the code block"));
            }
            leaves += 1;
            if leaves > 256 {
                return Err(err("the Huffman table has more than 256 symbols"));
            }
            let mut blob = peek(data, *bits_read);
            let mut advance = 1u64;
            while blob & 1 == 0 {
                advance += 1;
                blob >>= 1;
                code_size += 1;
                if code_size > MAX_CODE_BITS {
                    return Err(err("a Huffman code is longer than 16 bits"));
                }
            }
            blob >>= 1;
            let symbol = (blob & 0xFF) as u8;
            h.code[usize::from(symbol)] = code;
            h.size[usize::from(symbol)] = code_size;
            if code_size <= FAST_BITS {
                let dup = FAST_BITS - code_size;
                for i in 0..(1u32 << dup) {
                    h.fast[((i << code_size) | code) as usize] = Some(symbol);
                }
            } else {
                h.slow.push((code, code_size, symbol));
            }
            *bits_read += advance + 8;
            if code_size == 0 {
                break; // a tree that is a single leaf
            }
            let mut high = 1u32 << (code_size - 1);
            while code & high != 0 {
                code_size -= 1;
                if code_size == 0 {
                    break;
                }
                code &= high - 1;
                high >>= 1;
            }
            code |= high;
            if code_size == 0 {
                break;
            }
        }
        Ok(h)
    }

    /// The symbol whose code starts `blob`, and its code length (at least 1).
    fn decode(&self, blob: u64) -> Option<(u8, u32)> {
        let fast = (blob & ((1 << FAST_BITS) - 1)) as usize;
        if let Some(sym) = self.fast[fast] {
            return Some((sym, self.size[usize::from(sym)].max(1)));
        }
        self.slow
            .iter()
            .find(|(code, size, _)| (blob & ((1u64 << size) - 1)) as u32 == *code)
            .map(|&(_, size, sym)| (sym, size.max(1)))
    }
}

/// Decode one code block of `block_w × block_h` coefficients: three planes (HL, LH, HH) for a
/// high-pass block (`ll == false`), one for an LL block. Returns two's-complement coefficients,
/// plane after plane, each in raster order.
pub fn decode_block(
    data: &[u8],
    block_w: usize,
    block_h: usize,
    ll: bool,
) -> Result<Vec<i16>, CodecError> {
    let coeff_count = if ll { 1 } else { 3 };
    let area = block_w * block_h;
    if area == 0 || !area.is_multiple_of(16) || !block_w.is_multiple_of(4) {
        return Err(err(format!(
            "code block size {block_w} x {block_h} is not a multiple of 4 x 4"
        )));
    }
    let mut out = vec![0i16; coeff_count * area];
    // Blocks of 8 bytes or less are empty (all coefficients zero).
    if data.len() <= 8 {
        return Ok(out);
    }
    let total_bits = (data.len() as u64) * 8;
    let serialized = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
    if serialized > 2 * coeff_count * area * 2 {
        return Err(err(format!(
            "the block declares {serialized} bytes of bit planes, more than {} coefficients can hold",
            coeff_count * area
        )));
    }
    let zerorun_symbol = data[4];
    let counter_bits = u32::from(data[5]);
    if counter_bits > 24 {
        return Err(err(format!(
            "zero-run counter of {counter_bits} bits (at most 24 expected)"
        )));
    }
    let mut bits_read: u64 = 48;
    let h = Huffman::read(data, &mut bits_read, total_bits)?;

    // The message: 8-bit symbols, a zero-run symbol followed by a counter expands to that many
    // zero bytes (a counter of 0 is the symbol itself); consecutive zero-run symbols extend the
    // counter by `counter_bits` each.
    let mut buf = vec![0u8; serialized];
    let zr_code = h.code[usize::from(zerorun_symbol)];
    let zr_size = h.size[usize::from(zerorun_symbol)].max(1);
    let zr_mask = (1u64 << zr_size) - 1;
    let counter_mask = (1u64 << counter_bits) - 1;
    let mut n = 0usize;
    while n < serialized && bits_read < total_bits {
        let blob = peek(data, bits_read);
        let (sym, size) = h
            .decode(blob)
            .ok_or_else(|| err("a Huffman code matches no symbol"))?;
        let blob = blob >> size;
        bits_read += u64::from(size);
        if sym != zerorun_symbol {
            buf[n] = sym;
            n += 1;
            continue;
        }
        let mut zeros = blob & counter_mask;
        bits_read += u64::from(counter_bits);
        if zeros == 0 {
            buf[n] = sym;
            n += 1;
            continue;
        }
        if n as u64 + zeros >= serialized as u64 || bits_read >= total_bits {
            n = n.saturating_add(usize::try_from(zeros).unwrap_or(usize::MAX));
            break;
        }
        let mut width = counter_bits;
        loop {
            let next = peek(data, bits_read);
            if next & zr_mask != u64::from(zr_code) {
                break;
            }
            width += counter_bits;
            if width > 63 {
                return Err(err("a zero run counter is longer than 63 bits"));
            }
            zeros = (zeros << counter_bits) | ((next >> zr_size) & counter_mask);
            bits_read += u64::from(zr_size + counter_bits);
            if n as u64 + zeros >= serialized as u64 || bits_read >= total_bits {
                break;
            }
        }
        // The buffer is zero already: skip the run.
        n = n.saturating_add(usize::try_from(zeros).unwrap_or(usize::MAX));
    }
    let decoded = n.min(serialized);

    // Which bit planes are stored: all 16 of each coefficient, or, when the data is shorter,
    // the ones set in the masks stored in its last 2 (LL) or 6 bytes.
    let plane_bytes = area / 8;
    let mut masks = [0xFFFFu16; 3];
    let mut planes: usize = 16 * coeff_count;
    if decoded < planes * plane_bytes {
        let at = decoded
            .checked_sub(2 * coeff_count)
            .ok_or_else(|| err("bit-plane masks missing at the end of the block"))?;
        for (k, m) in masks.iter_mut().take(coeff_count).enumerate() {
            *m = le_u16(&buf, at + 2 * k)?;
        }
        planes = masks
            .iter()
            .take(coeff_count)
            .map(|m| m.count_ones() as usize)
            .sum();
        if planes * plane_bytes > at {
            return Err(err(format!(
                "the masks name {planes} bit planes, the block holds {}",
                at / plane_bytes.max(1)
            )));
        }
    }

    // Bit planes in order: coefficient by coefficient, each from its lowest set mask bit up.
    // Mask bit 0 is the sign (bit 15 of the sample), bit b > 0 is magnitude bit b - 1.
    let mut raw = vec![0u16; coeff_count * area];
    let mut remaining = masks;
    let mut coeff = 0usize;
    for p in 0..planes {
        let bit = loop {
            if coeff >= coeff_count {
                return Err(err("more bit planes than the masks allow"));
            }
            if remaining[coeff] != 0 {
                let b = remaining[coeff].trailing_zeros();
                remaining[coeff] &= !(1 << b);
                break b;
            }
            coeff += 1;
        };
        let shift = if bit == 0 { 15 } else { bit - 1 };
        let plane = &buf[p * plane_bytes..(p + 1) * plane_bytes];
        let dst = &mut raw[coeff * area..(coeff + 1) * area];
        for (j, &b) in plane.iter().enumerate() {
            if b == 0 {
                continue;
            }
            for k in 0..8 {
                dst[8 * j + k] |= u16::from((b >> k) & 1) << shift;
            }
        }
    }

    // Samples are stored in 4 × 4 areas (raster order of areas, raster order inside); signed
    // magnitude becomes two's complement.
    let areas_x = block_w / 4;
    for c in 0..coeff_count {
        if masks[c] == 0 {
            continue;
        }
        let src = &raw[c * area..(c + 1) * area];
        let dst = &mut out[c * area..(c + 1) * area];
        for (a, chunk) in src.chunks_exact(16).enumerate() {
            let ax = (a % areas_x) * 4;
            let ay = (a / areas_x) * 4;
            for r in 0..4 {
                for q in 0..4 {
                    let v = chunk[r * 4 + q];
                    let mag = (v & 0x7FFF) as i16;
                    dst[(ay + r) * block_w + ax + q] = if v & 0x8000 != 0 { -mag } else { mag };
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes bits least significant first.
    struct Bits(Vec<u8>, u64);
    impl Bits {
        fn put(&mut self, v: u64, n: u32) {
            for i in 0..n {
                let pos = self.1;
                if pos / 8 >= self.0.len() as u64 {
                    self.0.push(0);
                }
                self.0[(pos / 8) as usize] |= (((v >> i) & 1) as u8) << (pos % 8);
                self.1 += 1;
            }
        }
    }

    /// A block of one LL plane set: the sign plane is all zero, magnitude bit 0 is set for the
    /// first sample only. Tree with three leaves: 0x00, 0x01, 0x03.
    #[test]
    fn decodes_a_hand_made_block() {
        let (bw, bh) = (8usize, 4usize);
        let plane_bytes = bw * bh / 8; // 4
        // Two bit planes (mask 0b11: sign and magnitude bit 0), then the 2-byte mask.
        let mut message = vec![0u8; 2 * plane_bytes];
        message[plane_bytes] = 0x01;
        message.extend_from_slice(&0b11u16.to_le_bytes());
        let mut tree = Bits(Vec::new(), 0);
        for (bit, sym) in [(0, None), (0, None), (1, Some(0x00)), (1, Some(0x01)), (1, Some(0x03))] {
            tree.put(bit, 1);
            if let Some(s) = sym {
                tree.put(s, 8);
            }
        }
        let mut pos = 0;
        let h = Huffman::read(&tree.0, &mut pos, tree.1).unwrap();
        assert_eq!(pos, tree.1);
        let mut b = Bits(Vec::new(), 0);
        b.put(message.len() as u64, 32);
        b.put(0x07, 8); // zero-run symbol (not in the tree, never used)
        b.put(4, 8); // counter bits
        for i in 0..tree.1 {
            b.put(peek(&tree.0, i) & 1, 1);
        }
        for &byte in &message {
            let s = usize::from(byte);
            b.put(u64::from(h.code[s]), h.size[s].max(1));
        }
        let out = decode_block(&b.0, bw, bh, true).unwrap();
        let mut want = vec![0i16; bw * bh];
        want[0] = 1;
        assert_eq!(out, want);
    }

    #[test]
    fn short_blocks_are_empty_and_bad_ones_are_refused() {
        assert_eq!(decode_block(&[0; 8], 8, 4, false).unwrap(), vec![0; 96]);
        // Declared length larger than the coefficients can hold.
        let mut d = vec![0u8; 16];
        d[..4].copy_from_slice(&1_000_000u32.to_le_bytes());
        assert!(decode_block(&d, 8, 4, true).is_err());
        // A tree that never ends.
        let mut d = vec![0u8; 16];
        d[5] = 4;
        assert!(decode_block(&d, 8, 4, true).is_err());
        assert!(decode_block(&[0; 20], 6, 4, true).is_err());
    }
}
