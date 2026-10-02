//! Output stage: chroma upsampling, inverse colour transform and sample formatting of one
//! finished macroblock row (T.832 clause 9.9 and annex; jxrlib `strdec.c` `outputMBRow`,
//! `outputNChannel`, `interpolateUV`, `fixup_Y_ONLY_to_Others`).

use crate::header::{bd, cf};
use crate::w::W;
use crate::{Error, Result};

/// Position of pixel (x, y) of a macroblock in its buffer (jxrlib `idxCC`).
const IDX_CC: [[u8; 16]; 16] = [
    [
        0x00, 0x01, 0x05, 0x04, 0x40, 0x41, 0x45, 0x44, 0x80, 0x81, 0x85, 0x84, 0xc0, 0xc1, 0xc5,
        0xc4,
    ],
    [
        0x02, 0x03, 0x07, 0x06, 0x42, 0x43, 0x47, 0x46, 0x82, 0x83, 0x87, 0x86, 0xc2, 0xc3, 0xc7,
        0xc6,
    ],
    [
        0x0a, 0x0b, 0x0f, 0x0e, 0x4a, 0x4b, 0x4f, 0x4e, 0x8a, 0x8b, 0x8f, 0x8e, 0xca, 0xcb, 0xcf,
        0xce,
    ],
    [
        0x08, 0x09, 0x0d, 0x0c, 0x48, 0x49, 0x4d, 0x4c, 0x88, 0x89, 0x8d, 0x8c, 0xc8, 0xc9, 0xcd,
        0xcc,
    ],
    [
        0x10, 0x11, 0x15, 0x14, 0x50, 0x51, 0x55, 0x54, 0x90, 0x91, 0x95, 0x94, 0xd0, 0xd1, 0xd5,
        0xd4,
    ],
    [
        0x12, 0x13, 0x17, 0x16, 0x52, 0x53, 0x57, 0x56, 0x92, 0x93, 0x97, 0x96, 0xd2, 0xd3, 0xd7,
        0xd6,
    ],
    [
        0x1a, 0x1b, 0x1f, 0x1e, 0x5a, 0x5b, 0x5f, 0x5e, 0x9a, 0x9b, 0x9f, 0x9e, 0xda, 0xdb, 0xdf,
        0xde,
    ],
    [
        0x18, 0x19, 0x1d, 0x1c, 0x58, 0x59, 0x5d, 0x5c, 0x98, 0x99, 0x9d, 0x9c, 0xd8, 0xd9, 0xdd,
        0xdc,
    ],
    [
        0x20, 0x21, 0x25, 0x24, 0x60, 0x61, 0x65, 0x64, 0xa0, 0xa1, 0xa5, 0xa4, 0xe0, 0xe1, 0xe5,
        0xe4,
    ],
    [
        0x22, 0x23, 0x27, 0x26, 0x62, 0x63, 0x67, 0x66, 0xa2, 0xa3, 0xa7, 0xa6, 0xe2, 0xe3, 0xe7,
        0xe6,
    ],
    [
        0x2a, 0x2b, 0x2f, 0x2e, 0x6a, 0x6b, 0x6f, 0x6e, 0xaa, 0xab, 0xaf, 0xae, 0xea, 0xeb, 0xef,
        0xee,
    ],
    [
        0x28, 0x29, 0x2d, 0x2c, 0x68, 0x69, 0x6d, 0x6c, 0xa8, 0xa9, 0xad, 0xac, 0xe8, 0xe9, 0xed,
        0xec,
    ],
    [
        0x30, 0x31, 0x35, 0x34, 0x70, 0x71, 0x75, 0x74, 0xb0, 0xb1, 0xb5, 0xb4, 0xf0, 0xf1, 0xf5,
        0xf4,
    ],
    [
        0x32, 0x33, 0x37, 0x36, 0x72, 0x73, 0x77, 0x76, 0xb2, 0xb3, 0xb7, 0xb6, 0xf2, 0xf3, 0xf7,
        0xf6,
    ],
    [
        0x3a, 0x3b, 0x3f, 0x3e, 0x7a, 0x7b, 0x7f, 0x7e, 0xba, 0xbb, 0xbf, 0xbe, 0xfa, 0xfb, 0xff,
        0xfe,
    ],
    [
        0x38, 0x39, 0x3d, 0x3c, 0x78, 0x79, 0x7d, 0x7c, 0xb8, 0xb9, 0xbd, 0xbc, 0xf8, 0xf9, 0xfd,
        0xfc,
    ],
];

/// Position of pixel (x, y) of a 4:2:0 chroma macroblock (jxrlib `idxCC_420`).
const IDX_CC_420: [[u8; 8]; 8] = [
    [0x00, 0x01, 0x05, 0x04, 0x20, 0x21, 0x25, 0x24],
    [0x02, 0x03, 0x07, 0x06, 0x22, 0x23, 0x27, 0x26],
    [0x0a, 0x0b, 0x0f, 0x0e, 0x2a, 0x2b, 0x2f, 0x2e],
    [0x08, 0x09, 0x0d, 0x0c, 0x28, 0x29, 0x2d, 0x2c],
    [0x10, 0x11, 0x15, 0x14, 0x30, 0x31, 0x35, 0x34],
    [0x12, 0x13, 0x17, 0x16, 0x32, 0x33, 0x37, 0x36],
    [0x1a, 0x1b, 0x1f, 0x1e, 0x3a, 0x3b, 0x3f, 0x3e],
    [0x18, 0x19, 0x1d, 0x1c, 0x38, 0x39, 0x3d, 0x3c],
];

#[inline]
fn idx(x: usize, y: usize) -> usize {
    ((x >> 4) << 8) + IDX_CC[y][x & 15] as usize
}

/// Destination of the decoded pixels and everything the output stage needs.
#[derive(Debug)]
pub(crate) struct Output {
    /// Internal colour format of the codestream.
    pub(crate) cf_int: u8,
    /// Output colour format (jxrlib `WMII.cfColorFormat` after validation).
    pub(crate) cf_ext: u8,
    pub(crate) bd: u8,
    pub(crate) scaled: bool,
    pub(crate) shift: u8,
    pub(crate) exp_bias: i8,
    /// R,G,B order for 8-bit RGB output (false: B,G,R).
    pub(crate) rgb: bool,
    /// Channels coded in the codestream.
    pub(crate) channels: usize,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) left: usize,
    pub(crate) top: usize,
    pub(crate) mb_w: usize,
    /// Samples per pixel in `data` (including padding samples).
    pub(crate) px_samples: usize,
    pub(crate) sample_bytes: usize,
    pub(crate) uv_res_change: bool,
    pub(crate) res_u: Vec<i32>,
    pub(crate) res_v: Vec<i32>,
    pub(crate) data: Vec<u8>,
}

#[inline]
fn clip(v: i32, lo: i32, hi: i32) -> i32 {
    v.clamp(lo, hi)
}

/// jxrlib `backwardHalf`.
fn backward_half(h: i32) -> u16 {
    let s = h >> 31;
    (((h & 0x7fff) ^ s).wrapping_sub(s)) as u16
}

/// jxrlib `pixel2float`: internal fixed-point value to IEEE single-precision bits.
fn pixel_to_float(h: i32, c: i8, lm: u8) -> u32 {
    let lm = u32::from(lm);
    let lmshift = 1i32.wrapping_shl(lm);
    let s = h >> 31;
    let t = (h ^ s).wrapping_sub(s);
    let mut e = (t as u32).wrapping_shr(lm) as i32;
    let mut m = (t & lmshift.wrapping_sub(1)) | lmshift;
    if e == 0 {
        m ^= lmshift;
        e = 1;
    }
    e = e.wrapping_add(127 - i32::from(c));
    while m < lmshift && e > 1 && m > 0 {
        e -= 1;
        m = m.wrapping_shl(1);
    }
    if m < lmshift {
        e = 0;
    } else {
        m ^= lmshift;
    }
    m = m.wrapping_shl(23u32.wrapping_sub(lm));
    ((s as u32) & 0x8000_0000) | (e as u32).wrapping_shl(23) | m as u32
}

/// Write one sample of bit depth `bd` (jxrlib's clipping and conversions) at `dst[0..]`.
#[inline]
fn store(bd: u8, exp_bias: i8, len: u8, dst: &mut [u8], v: i32) {
    match bd {
        bd::B8 => dst[0] = clip(v, 0, 255) as u8,
        bd::B16 => dst[..2].copy_from_slice(&(clip(v, 0, 65535) as u16).to_le_bytes()),
        bd::B16S => dst[..2].copy_from_slice(&(clip(v, -32768, 32767) as i16).to_le_bytes()),
        bd::B16F => dst[..2].copy_from_slice(&backward_half(v).to_le_bytes()),
        bd::B32S => dst[..4].copy_from_slice(&v.to_le_bytes()),
        bd::B32F => dst[..4].copy_from_slice(&pixel_to_float(v, exp_bias, len).to_le_bytes()),
        _ => {}
    }
}

impl Output {
    /// jxrlib `interpolateUV`: upsample one macroblock row of 4:2:0 or 4:2:2 chroma to 4:4:4.
    fn interpolate_uv(&mut self, buf: &[i32], prev: &[usize], cur: &[usize], bottom: bool) {
        let width = self.mb_w * 16;
        for (plane, dst) in [(1usize, &mut self.res_u), (2usize, &mut self.res_v)] {
            let src = buf;
            let s0 = prev[plane];
            if self.cf_int == cf::YUV_422 {
                for row in 0..16 {
                    let mut d_idx = 0;
                    let mut col = 0;
                    while col < width {
                        let s_idx = ((col >> 4) << 7) + IDX_CC[row][(col >> 1) & 7] as usize;
                        d_idx = idx(col, row);
                        dst[d_idx] = src[s0 + s_idx];
                        if col > 0 {
                            let l = idx(col - 2, row);
                            let c = idx(col - 1, row);
                            dst[c] = ((W(dst[l]) + W(dst[d_idx]) + 1) >> 1).0;
                        }
                        col += 2;
                    }
                    let last = idx(col - 1, row);
                    dst[last] = dst[d_idx];
                }
            } else {
                let full = self.cf_ext != cf::YUV_422;
                for col in (0..width).step_by(2) {
                    let (mb, pix) = if full {
                        ((col >> 4) << 8, col & 15)
                    } else {
                        ((col >> 4) << 7, (col >> 1) & 7)
                    };
                    let mut d_idx = 0;
                    for row in (0..16).step_by(2) {
                        let s_idx =
                            ((col >> 4) << 6) + IDX_CC_420[row >> 1][(col >> 1) & 7] as usize;
                        d_idx = mb + IDX_CC[row][pix] as usize;
                        dst[d_idx] = src[s0 + s_idx];
                        if row > 0 {
                            let t = mb + IDX_CC[row - 2][pix] as usize;
                            let c = mb + IDX_CC[row - 1][pix] as usize;
                            dst[c] = ((W(dst[t]) + W(dst[d_idx]) + 1) >> 1).0;
                        }
                    }
                    let s_last = mb + IDX_CC[15][pix] as usize;
                    if bottom {
                        dst[s_last] = dst[d_idx];
                    } else {
                        let b = ((col >> 4) << 6) + IDX_CC_420[0][(col >> 1) & 7] as usize;
                        dst[s_last] = ((W(src[cur[plane] + b]) + W(dst[d_idx]) + 1) >> 1).0;
                    }
                }
                if full {
                    for row in 0..16 {
                        let mut s_idx = 0;
                        let mut col = 1;
                        while col < width - 2 {
                            let l = idx(col - 1, row);
                            let d_idx = idx(col, row);
                            s_idx = idx(col + 1, row);
                            dst[d_idx] = ((W(dst[s_idx]) + W(dst[l]) + 1) >> 1).0;
                            col += 2;
                        }
                        let last = idx(width - 1, row);
                        dst[last] = dst[s_idx];
                    }
                }
            }
        }
    }
}

/// jxrlib `outputMBRow` for macroblock row `mb_row`, whose pixels are in the `prev` regions.
pub(crate) fn output_row(
    out: &mut Output,
    buf: &[i32],
    prev: &[usize],
    cur: &[usize],
    mb_row: usize,
    bottom: bool,
) -> Result<()> {
    if out.uv_res_change {
        out.interpolate_uv(buf, prev, cur, bottom);
    }
    let shift: i32 = if out.scaled { 3 } else { 0 };
    let scaled_bias = if out.scaled { 3 } else { 0 };
    let len = i32::from(out.shift);
    let bias = match out.bd {
        bd::B8 => (128 << shift) + scaled_bias,
        bd::B16 => ((W(1 << 15) >> len) << shift).0 + if shift == 0 { 0 } else { 1 << (shift - 1) },
        bd::B16S | bd::B16F | bd::B32S | bd::B32F => scaled_bias,
        _ => return Err(Error::unsupported("output bit depth")),
    };
    let cf_eff = if out.cf_int == cf::Y_ONLY {
        cf::Y_ONLY
    } else {
        out.cf_ext
    };
    let y0 = mb_row * 16;
    let (b_idx, r_idx) = if out.rgb || out.bd != bd::B8 {
        (2, 0)
    } else {
        (0, 2)
    };
    let (sb, px) = (out.sample_bytes, out.px_samples * out.sample_bytes);
    let row_bytes = out.width * px;
    let (obd, exp_bias, olen) = (out.bd, out.exp_bias, out.shift);
    let n_ch = if out.cf_ext == cf::Y_ONLY {
        1
    } else {
        out.channels
    };
    let scale_len = matches!(obd, bd::B8 | bd::B16 | bd::B16S | bd::B32S);
    let shl_len = matches!(obd, bd::B16 | bd::B16S | bd::B32S);
    if !matches!(cf_eff, cf::RGB | cf::Y_ONLY | cf::YUV_444 | cf::NCOMPONENT) {
        return Err(Error::unsupported("output colour format"));
    }
    let Output {
        data,
        res_u,
        res_v,
        uv_res_change,
        top,
        height,
        left,
        width,
        ..
    } = out;
    for yy in 0..16 {
        let y = y0 + yy;
        if y < *top || y >= *top + *height {
            continue;
        }
        let oy = y - *top;
        let Some(drow) = data.get_mut(oy * row_bytes..(oy + 1) * row_bytes) else {
            continue;
        };
        for (ox, dpx) in drow.chunks_exact_mut(px).enumerate().take(*width) {
            let i = idx(ox + *left, yy);
            if cf_eff == cf::RGB {
                let yv = W(buf[prev[0] + i]);
                let (u, v) = if *uv_res_change {
                    (W(res_u[i]), W(res_v[i]))
                } else {
                    (W(buf[prev[1] + i]), W(buf[prev[2] + i]))
                };
                let mut g = yv + bias;
                let mut r = -u;
                let mut b = v;
                // _ICC(r, g, b)
                g -= r >> 1;
                r -= ((b + 1) >> 1) - g;
                b += r;
                let (r, g, b) = if scale_len {
                    let f = |c: W| {
                        if obd == bd::B8 {
                            c >> shift
                        } else {
                            (c >> shift) << len
                        }
                    };
                    (f(r), f(g), f(b))
                } else {
                    (r >> shift, g >> shift, b >> shift)
                };
                store(obd, exp_bias, olen, &mut dpx[r_idx * sb..], r.0);
                store(obd, exp_bias, olen, &mut dpx[sb..], g.0);
                store(obd, exp_bias, olen, &mut dpx[b_idx * sb..], b.0);
            } else {
                for ch in 0..n_ch {
                    let v = if *uv_res_change && (ch == 1 || ch == 2) {
                        if ch == 1 { res_u[i] } else { res_v[i] }
                    } else {
                        buf[prev[ch] + i]
                    };
                    let mut p = (W(v) + bias) >> shift;
                    if shl_len {
                        p = p << len;
                    }
                    store(obd, exp_bias, olen, &mut dpx[ch * sb..], p.0);
                }
            }
        }
    }
    Ok(())
}

/// jxrlib `fixup_Y_ONLY_to_Others`: grey decoded into an RGB pixel format.
pub(crate) fn fixup_gray_to_rgb(out: &mut Output) {
    if out.cf_ext != cf::RGB || out.cf_int != cf::Y_ONLY || out.px_samples < 3 {
        return;
    }
    let sb = out.sample_bytes;
    for px in out.data.chunks_exact_mut(out.px_samples * sb) {
        let (first, rest) = px.split_at_mut(sb);
        rest[..sb].copy_from_slice(first);
        rest[sb..2 * sb].copy_from_slice(first);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_and_half_conversions() {
        // Exponent field 1 with bias 1 and a 10-bit mantissa of 0: 2^(1 + 127 - 1 - 127) = 1.0.
        assert_eq!(pixel_to_float(1 << 10, 1, 10), 1.0f32.to_bits());
        assert_eq!(pixel_to_float(-(1 << 10), 1, 10), (-1.0f32).to_bits());
        assert_eq!(pixel_to_float(3 << 9, 1, 10), 1.5f32.to_bits());
        assert_eq!(pixel_to_float(0, 0, 10), 0);
        assert_eq!(backward_half(0x3c00), 0x3c00);
        assert_eq!(backward_half(-0x3c00), 0xbc00);
    }
}
