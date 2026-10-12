//! Tile reconstruction of an iSyntax pyramid: the 5/3 inverse wavelet transform of one tile with
//! margins from its eight neighbours, and the YCoCg to RGB conversion. A port of libisyntax's
//! reconstruction (BSD-2-Clause; its lifting steps come from OpenJPEG, BSD-2-Clause), see
//! `NOTICE` and `docs/formats/philips-isyntax.md` § Reconstruction.

/// Coefficients taken from each neighbour on each side of a quadrant.
pub const PAD: usize = 4;
/// First sample of the inverse transform that belongs to the tile.
pub const FIRST_VALID: usize = 7;

/// The coefficients of one colour component of a tile and its neighbourhood.
#[derive(Debug, Clone, Copy)]
pub struct Neighbour<'a> {
    /// LL block (`bw · bh`), `None` when its parent tile does not exist.
    pub ll: Option<&'a [i16]>,
    /// HL, LH and HH blocks, one after the other (`3 · bw · bh`).
    pub h: &'a [i16],
}

/// One 5/3 inverse lifting pass over `len = 2 · sn` samples whose first sample is odd
/// (`in_odd` = the first `sn` values, `in_even` = the next `sn`), as OpenJPEG's `cas = 1` case
/// does with 16-bit intermediates.
fn idwt_row(v: &mut [i16], tmp: &mut [i16]) {
    let len = v.len();
    let sn = len / 2;
    if len <= 2 {
        if len == 1 {
            v[0] /= 2;
        } else if len == 2 {
            let (odd, even) = (v[0], v[1]);
            let o1 = odd.wrapping_sub(((i32::from(even) + 1) >> 1) as i16);
            v[0] = even.wrapping_add(o1);
            v[1] = o1;
        }
        return;
    }
    let (odd, even) = v.split_at(sn);
    let mut s1 = even[1];
    let mut dc = odd[0].wrapping_sub(((i32::from(even[0]) + i32::from(s1) + 2) >> 2) as i16);
    tmp[0] = even[0].wrapping_add(dc);
    let mut i = 1usize;
    let mut j = 1usize;
    let stop = len - 2 - usize::from(len & 1 == 0);
    while i < stop {
        let s2 = even[j + 1];
        let dn = odd[j].wrapping_sub(((i32::from(s1) + i32::from(s2) + 2) >> 2) as i16);
        tmp[i] = dc;
        tmp[i + 1] = s1.wrapping_add(((i32::from(dn) + i32::from(dc)) >> 1) as i16);
        dc = dn;
        s1 = s2;
        i += 2;
        j += 1;
    }
    tmp[i] = dc;
    if len & 1 == 0 {
        let dn = odd[len / 2 - 1].wrapping_sub(((i32::from(s1) + 1) >> 1) as i16);
        tmp[len - 2] = s1.wrapping_add(((i32::from(dn) + i32::from(dc)) >> 1) as i16);
        tmp[len - 1] = dn;
    } else {
        tmp[len - 1] = s1.wrapping_add(dc);
    }
    v.copy_from_slice(&tmp[..len]);
}

/// The 2-D inverse transform of a `2·qw × 2·qh` buffer laid out as four quadrants
/// (LL | HL over LH | HH): every row, then every column.
pub fn idwt(buf: &mut [i16], qw: usize, qh: usize) {
    let (w, h) = (2 * qw, 2 * qh);
    let mut tmp = vec![0i16; w.max(h)];
    for row in buf.chunks_exact_mut(w).take(h) {
        idwt_row(row, &mut tmp);
    }
    let mut col = vec![0i16; h];
    for x in 0..w {
        for (y, c) in col.iter_mut().enumerate() {
            *c = buf[y * w + x];
        }
        idwt_row(&mut col, &mut tmp);
        for (y, c) in col.iter().enumerate() {
            buf[y * w + x] = *c;
        }
    }
}

/// Inverse transform of one colour component of a tile. `nb` holds the 3 × 3 neighbourhood in
/// raster order (index 4 is the tile itself, which must exist); `None` is a neighbour that does
/// not exist or lies outside the level. `luma` selects the fill for missing LL coefficients
/// (255 for Y, 0 for Co and Cg). Returns the `2·(bw+8) × 2·(bh+8)` buffer.
pub fn tile_component(
    nb: &[Option<Neighbour<'_>>; 9],
    bw: usize,
    bh: usize,
    luma: bool,
) -> Vec<i16> {
    let qw = bw + 2 * PAD;
    let qh = bh + 2 * PAD;
    let w = 2 * qw;
    let mut buf = vec![0i16; w * 2 * qh];
    if luma {
        // The LL quadrant starts white: margins of missing neighbours stay 255.
        for y in 0..qh {
            buf[y * w..y * w + qw].fill(255);
        }
    }
    let ll_fill = vec![if luma { 255i16 } else { 0 }; bw * bh];
    let quadrant_origin = [0, qw, qh * w, qh * w + qw];
    // For neighbour (dx, dy): which source rows/columns and where they go in the quadrant.
    let span = |d: i32, n: usize| -> (usize, usize, usize) {
        // (source start, count, destination start)
        match d {
            -1 => (n - PAD, PAD, 0),
            0 => (0, n, PAD),
            _ => (0, PAD, PAD + n),
        }
    };
    for (k, cell) in nb.iter().enumerate() {
        let Some(n) = cell else { continue };
        let dx = (k % 3) as i32 - 1;
        let dy = (k / 3) as i32 - 1;
        let (sx, cw, tx) = span(dx, bw);
        let (sy, ch, ty) = span(dy, bh);
        for (q, &origin) in quadrant_origin.iter().enumerate() {
            let src: &[i16] = if q == 0 {
                n.ll.unwrap_or(&ll_fill)
            } else {
                &n.h[(q - 1) * bw * bh..q * bw * bh]
            };
            for r in 0..ch {
                let s = (sy + r) * bw + sx;
                let d = origin + (ty + r) * w + tx;
                buf[d..d + cw].copy_from_slice(&src[s..s + cw]);
            }
        }
    }
    idwt(&mut buf, qw, qh);
    buf
}

/// The four LL blocks of the next finer level held by a transformed buffer: top left, top
/// right, bottom left, bottom right.
pub fn child_ll(buf: &[i16], bw: usize, bh: usize) -> [Vec<i16>; 4] {
    let w = 2 * (bw + 2 * PAD);
    let block = |ox: usize, oy: usize| {
        let mut out = Vec::with_capacity(bw * bh);
        for y in 0..bh {
            let s = (FIRST_VALID + oy + y) * w + FIRST_VALID + ox;
            out.extend_from_slice(&buf[s..s + bw]);
        }
        out
    };
    [block(0, 0), block(bw, 0), block(0, bh), block(bw, bh)]
}

/// RGB pixels of a tile (`2·bw × 2·bh`, interleaved) from the transformed Y, Co and Cg
/// buffers: Y is taken as an absolute value, then R, G, B are computed with arithmetic shifts
/// and clamped to 0..255.
#[allow(clippy::many_single_char_names)]
pub fn to_rgb(y: &[i16], co: &[i16], cg: &[i16], bw: usize, bh: usize) -> Vec<u8> {
    let w = 2 * (bw + 2 * PAD);
    let (tw, th) = (2 * bw, 2 * bh);
    let mut out = Vec::with_capacity(tw * th * 3);
    for r in 0..th {
        let base = (FIRST_VALID + r) * w + FIRST_VALID;
        for c in 0..tw {
            let i = base + c;
            // Signed magnitude read of the luma value, sign cleared.
            let yy = y[i] as u16;
            let ymag = if yy & 0x8000 != 0 {
                (0x8000u16.wrapping_sub(yy)) & 0x7FFF
            } else {
                yy
            } as i16;
            let tmp = ymag.wrapping_sub(cg[i] >> 1);
            let g = tmp.wrapping_add(cg[i]);
            let b = tmp.wrapping_sub(co[i] >> 1);
            let rr = b.wrapping_add(co[i]);
            out.push(rr.clamp(0, 255) as u8);
            out.push(g.clamp(0, 255) as u8);
            out.push(b.clamp(0, 255) as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_rows() {
        let mut tmp = vec![0; 4];
        let mut v = [10i16, 4];
        idwt_row(&mut v, &mut tmp);
        // odd = 10 - (4 + 1) / 2 = 8, even = 4 + 8 = 12
        assert_eq!(v, [12, 8]);
        let mut one = [5i16];
        idwt_row(&mut one, &mut tmp);
        assert_eq!(one, [2]);
    }

    #[test]
    fn white_neighbourhood_gives_white_pixels() {
        let (bw, bh) = (8, 8);
        let h = vec![0i16; 3 * bw * bh];
        let ll = vec![255i16; bw * bh];
        let zero = vec![0i16; bw * bh];
        let y_nb: [Option<Neighbour>; 9] = std::array::from_fn(|_| {
            Some(Neighbour {
                ll: Some(&ll),
                h: &h,
            })
        });
        let c_nb: [Option<Neighbour>; 9] = std::array::from_fn(|_| {
            Some(Neighbour {
                ll: Some(&zero),
                h: &h,
            })
        });
        let y = tile_component(&y_nb, bw, bh, true);
        let co = tile_component(&c_nb, bw, bh, false);
        let rgb = to_rgb(&y, &co, &co, bw, bh);
        assert_eq!(rgb.len(), 2 * bw * 2 * bh * 3);
        assert!(rgb.iter().all(|&v| v == 255), "{:?}", &rgb[..12]);
        let kids = child_ll(&y, bw, bh);
        assert!(kids.iter().all(|k| k.iter().all(|&v| v == 255)));
    }
}
