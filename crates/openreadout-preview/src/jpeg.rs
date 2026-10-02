//! A small baseline JPEG encoder (ITU-T T.81 sequential DCT, Huffman coded, 8-bit), written
//! from the public standard: JFIF APP0, the example quantization tables of Annex K.1 scaled by a
//! quality factor, the example Huffman tables of Annex K.3, no chroma subsampling. The DCT uses
//! f64 arithmetic with literal cosine constants (no libm calls), so output bytes are identical
//! on every platform.

use crate::canvas::Canvas;

/// Zig-zag scan: position `k` in the scan → index in natural (row-major) 8×8 order.
const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// Annex K.1 luminance quantization table, natural order.
const Q_LUMA: [u16; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56,
    14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113,
    92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];

/// Annex K.1 chrominance quantization table, natural order.
const Q_CHROMA: [u16; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99,
    47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];

// Annex K.3 Huffman tables: code counts per length 1..=16, then the symbols.
const DC_LUMA_BITS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const DC_CHROMA_BITS: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
const DC_VALS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const AC_LUMA_BITS: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d];
const AC_LUMA_VALS: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07,
    0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0,
    0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28,
    0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
    0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
    0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
    0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7,
    0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5,
    0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2,
    0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
    0xf9, 0xfa,
];
const AC_CHROMA_BITS: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77];
const AC_CHROMA_VALS: [u8; 162] = [
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71,
    0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0,
    0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34, 0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26,
    0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48,
    0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
    0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
    0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5,
    0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3,
    0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda,
    0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
    0xf9, 0xfa,
];

/// cos(kπ/16) for k = 0..=8, as literals (no libm, so identical on every platform).
const COS16: [f64; 9] = [
    1.0,
    0.980_785_280_403_230_4,
    0.923_879_532_511_286_7,
    0.831_469_612_302_545_2,
    std::f64::consts::FRAC_1_SQRT_2,
    0.555_570_233_019_602_2,
    0.382_683_432_365_089_84,
    0.195_090_322_016_128_25,
    0.0,
];

/// cos(mπ/16) for any integer m.
fn cos_pi16(m: usize) -> f64 {
    match m % 32 {
        m @ 0..=8 => COS16[m],
        m @ 9..=16 => -COS16[16 - m],
        m @ 17..=24 => -COS16[m - 16],
        m => COS16[32 - m],
    }
}

/// `basis[u][x] = C(u)/2 · cos((2x+1)uπ/16)`, the 1-D DCT-II matrix of T.81 A.3.3.
fn dct_basis() -> [[f64; 8]; 8] {
    let mut b = [[0.0; 8]; 8];
    for (u, row) in b.iter_mut().enumerate() {
        let cu = if u == 0 { COS16[4] } else { 1.0 };
        for (x, v) in row.iter_mut().enumerate() {
            *v = cu / 2.0 * cos_pi16((2 * x + 1) * u);
        }
    }
    b
}

/// Quantization table for `quality` (1–100): the Annex K table scaled by 5000/q below 50 and
/// by 200 − 2q from 50 up, clamped to 1..=255.
fn scaled(table: &[u16; 64], quality: u8) -> [u8; 64] {
    let q = u32::from(quality.clamp(1, 100));
    let scale = if q < 50 { 5000 / q } else { 200 - 2 * q };
    let mut out = [0u8; 64];
    for (o, &t) in out.iter_mut().zip(table) {
        *o = ((u32::from(t) * scale + 50) / 100).clamp(1, 255) as u8;
    }
    out
}

/// Canonical Huffman codes (T.81 Annex C): `(code, length)` per symbol.
fn huffman(bits: &[u8; 16], vals: &[u8]) -> [(u16, u8); 256] {
    let mut table = [(0u16, 0u8); 256];
    let mut code: u16 = 0;
    let mut k = 0;
    for (len, &n) in bits.iter().enumerate() {
        for _ in 0..n {
            table[vals[k] as usize] = (code, len as u8 + 1);
            code += 1;
            k += 1;
        }
        code <<= 1;
    }
    table
}

struct BitWriter {
    out: Vec<u8>,
    acc: u32,
    n: u32,
}

impl BitWriter {
    fn put(&mut self, code: u32, len: u32) {
        for i in (0..len).rev() {
            self.acc = (self.acc << 1) | ((code >> i) & 1);
            self.n += 1;
            if self.n == 8 {
                let b = self.acc as u8;
                self.out.push(b);
                if b == 0xFF {
                    self.out.push(0); // byte stuffing
                }
                self.acc = 0;
                self.n = 0;
            }
        }
    }
    fn flush(&mut self) {
        while self.n != 0 {
            self.put(1, 1); // pad with 1-bits
        }
    }
}

/// Number of bits needed for `v` (the JPEG "category"), and its value bits.
fn category(v: i32) -> (u32, u32) {
    let a = v.unsigned_abs();
    let size = 32 - a.leading_zeros();
    let bits = if v < 0 {
        (v - 1) as u32 & ((1u32 << size) - 1)
    } else {
        v as u32
    };
    (size, bits)
}

fn marker(out: &mut Vec<u8>, m: u8, payload: &[u8]) {
    out.extend_from_slice(&[0xFF, m]);
    out.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(payload);
}

/// Encode an RGB canvas (stored as one greyscale component when every pixel is grey).
#[allow(clippy::needless_range_loop)] // `comp` selects the colour transform, table and predictor
pub fn encode(c: &Canvas, quality: u8) -> Vec<u8> {
    let gray = c.is_gray();
    let ncomp = if gray { 1 } else { 3 };
    let ql = scaled(&Q_LUMA, quality);
    let qc = scaled(&Q_CHROMA, quality);
    let mut out = vec![0xFF, 0xD8];
    marker(
        &mut out,
        0xE0,
        &[b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0],
    );
    for (id, q) in [(0u8, &ql), (1u8, &qc)]
        .into_iter()
        .take(if gray { 1 } else { 2 })
    {
        let mut p = vec![id];
        p.extend(ZIGZAG.iter().map(|&n| q[n]));
        marker(&mut out, 0xDB, &p);
    }
    let (w, h) = (c.width as u16, c.height as u16);
    let mut sof = vec![8];
    sof.extend_from_slice(&h.to_be_bytes());
    sof.extend_from_slice(&w.to_be_bytes());
    sof.push(ncomp as u8);
    for i in 0..ncomp {
        sof.extend_from_slice(&[i as u8 + 1, 0x11, u8::from(i > 0)]);
    }
    marker(&mut out, 0xC0, &sof);
    let tables: [(u8, &[u8; 16], &[u8]); 4] = [
        (0x00, &DC_LUMA_BITS, &DC_VALS),
        (0x10, &AC_LUMA_BITS, &AC_LUMA_VALS),
        (0x01, &DC_CHROMA_BITS, &DC_VALS),
        (0x11, &AC_CHROMA_BITS, &AC_CHROMA_VALS),
    ];
    for (class, bits, vals) in tables.iter().take(if gray { 2 } else { 4 }) {
        let mut p = vec![*class];
        p.extend_from_slice(*bits);
        p.extend_from_slice(vals);
        marker(&mut out, 0xC4, &p);
    }
    let mut sos = vec![ncomp as u8];
    for i in 0..ncomp {
        sos.extend_from_slice(&[i as u8 + 1, if i == 0 { 0x00 } else { 0x11 }]);
    }
    sos.extend_from_slice(&[0, 63, 0]);
    marker(&mut out, 0xDA, &sos);

    let dc = [
        huffman(&DC_LUMA_BITS, &DC_VALS),
        huffman(&DC_CHROMA_BITS, &DC_VALS),
    ];
    let ac = [
        huffman(&AC_LUMA_BITS, &AC_LUMA_VALS),
        huffman(&AC_CHROMA_BITS, &AC_CHROMA_VALS),
    ];
    let basis = dct_basis();
    let mut bw = BitWriter { out, acc: 0, n: 0 };
    let mut pred = [0i32; 3];
    let (wu, hu) = (c.width as usize, c.height as usize);
    for by in (0..hu).step_by(8) {
        for bx in (0..wu).step_by(8) {
            for comp in 0..ncomp {
                let mut block = [0f64; 64];
                for y in 0..8 {
                    for x in 0..8 {
                        let (sx, sy) = ((bx + x).min(wu - 1), (by + y).min(hu - 1));
                        let i = (sy * wu + sx) * 3;
                        let (r, g, b) = (
                            f64::from(c.rgb[i]),
                            f64::from(c.rgb[i + 1]),
                            f64::from(c.rgb[i + 2]),
                        );
                        // JFIF YCbCr (full range)
                        block[y * 8 + x] = match comp {
                            0 => 0.299 * r + 0.587 * g + 0.114 * b,
                            1 => {
                                -0.168_735_891_647_856_6 * r - 0.331_264_108_352_143_4 * g
                                    + 0.5 * b
                                    + 128.0
                            }
                            _ => {
                                0.5 * r - 0.418_687_589_158_345_2 * g - 0.081_312_410_841_654_8 * b
                                    + 128.0
                            }
                        } - 128.0;
                    }
                }
                // separable 2-D DCT: rows, then columns
                let mut tmp = [0f64; 64];
                for y in 0..8 {
                    for u in 0..8 {
                        let mut s = 0.0;
                        for x in 0..8 {
                            s += basis[u][x] * block[y * 8 + x];
                        }
                        tmp[y * 8 + u] = s;
                    }
                }
                let q = if comp == 0 { &ql } else { &qc };
                let mut coef = [0i32; 64];
                for u in 0..8 {
                    for v in 0..8 {
                        let mut s = 0.0;
                        for y in 0..8 {
                            s += basis[v][y] * tmp[y * 8 + u];
                        }
                        coef[v * 8 + u] = (s / f64::from(q[v * 8 + u])).round() as i32;
                    }
                }
                let t = usize::from(comp > 0);
                let diff = coef[0] - pred[comp];
                pred[comp] = coef[0];
                let (size, bits) = category(diff);
                let (code, len) = dc[t][size as usize];
                bw.put(u32::from(code), u32::from(len));
                bw.put(bits, size);
                let mut run = 0;
                for &n in &ZIGZAG[1..] {
                    let v = coef[n];
                    if v == 0 {
                        run += 1;
                        continue;
                    }
                    while run > 15 {
                        let (code, len) = ac[t][0xF0];
                        bw.put(u32::from(code), u32::from(len));
                        run -= 16;
                    }
                    let (size, bits) = category(v);
                    let (code, len) = ac[t][(run << 4) | size as usize];
                    bw.put(u32::from(code), u32::from(len));
                    bw.put(bits, size);
                    run = 0;
                }
                if run > 0 {
                    let (code, len) = ac[t][0x00];
                    bw.put(u32::from(code), u32::from(len));
                }
            }
        }
    }
    bw.flush();
    let mut out = bw.out;
    out.extend_from_slice(&[0xFF, 0xD9]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_complete() {
        assert_eq!(
            AC_LUMA_BITS.iter().map(|&b| usize::from(b)).sum::<usize>(),
            162
        );
        assert_eq!(
            AC_CHROMA_BITS
                .iter()
                .map(|&b| usize::from(b))
                .sum::<usize>(),
            162
        );
        assert_eq!(
            DC_LUMA_BITS.iter().map(|&b| usize::from(b)).sum::<usize>(),
            12
        );
        assert_eq!(
            DC_CHROMA_BITS
                .iter()
                .map(|&b| usize::from(b))
                .sum::<usize>(),
            12
        );
        let mut seen = [false; 64];
        for &z in &ZIGZAG {
            seen[z] = true;
        }
        assert!(seen.iter().all(|&s| s));
    }

    #[test]
    fn category_matches_t81() {
        assert_eq!(category(0), (0, 0));
        assert_eq!(category(1), (1, 1));
        assert_eq!(category(-1), (1, 0));
        assert_eq!(category(-3), (2, 0));
        assert_eq!(category(5), (3, 5));
        assert_eq!(scaled(&Q_LUMA, 50)[0], 16);
        assert_eq!(scaled(&Q_LUMA, 100)[0], 1);
    }
}
