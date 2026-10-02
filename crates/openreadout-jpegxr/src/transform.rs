//! Inverse lapped transform: two stages of 4x4 core transforms (PCT) and overlap filters (POT)
//! across block and macroblock edges (T.832 clause 9.8; jxrlib `strInvTransform.c`,
//! `strTransform.c`).
//!
//! jxrlib runs the transform one macroblock behind the entropy decoder over two macroblock-row
//! buffers, reaching into the macroblocks to the left and above through negative offsets. This is
//! a faithful port of that schedule, which is what makes the output bit-exact: `p0` is the
//! position of the current column in the previous row's buffer, `p1` in the current row's, both
//! indices into one buffer laid out as jxrlib lays out its memory (see `decoder.rs`).
//!
//! Buffer layout of one macroblock (256 coefficients or pixels): sixteen 4x4 blocks in
//! column-major order (block (bx, by) at `16 * (4 * bx + by)`), and inside a block the order of
//! jxrlib's `idxCC` table. Chroma macroblocks of 4:2:0 hold 4 blocks, of 4:2:2 8 blocks.

use crate::header::cf;
use crate::w::W;

#[inline]
fn ld(d: &[i32], i: isize) -> W {
    W(d[i as usize])
}
#[inline]
fn st(d: &mut [i32], i: isize, v: W) {
    d[i as usize] = v.0;
}

/// jxrlib `strDCT2x2dn`.
fn dct2x2dn(d: &mut [i32], pa: isize, pb: isize, pc: isize, pd: isize) {
    let (mut a, mut b, cc, mut dd) = (ld(d, pa), ld(d, pb), ld(d, pc), ld(d, pd));
    a += dd;
    b -= cc;
    let t = (a - b) >> 1;
    let c = t - dd;
    dd = t - cc;
    a -= dd;
    b += c;
    st(d, pa, a);
    st(d, pb, b);
    st(d, pc, c);
    st(d, pd, dd);
}

/// jxrlib `strDCT2x2up`.
fn dct2x2up(d: &mut [i32], pa: isize, pb: isize, pc: isize, pd: isize) {
    let (mut a, mut b, cc, mut dd) = (ld(d, pa), ld(d, pb), ld(d, pc), ld(d, pd));
    a += dd;
    b -= cc;
    let t = (a - b + 1) >> 1;
    let c = t - dd;
    dd = t - cc;
    a -= dd;
    b += c;
    st(d, pa, a);
    st(d, pb, b);
    st(d, pc, c);
    st(d, pd, dd);
}

/// jxrlib `strDCT2x2dnDec` (2x2 with post-scaling, scaled arithmetic).
fn dct2x2dn_dec(d: &mut [i32], pa: isize, pb: isize, pc: isize, pd: isize) {
    dct2x2dn(d, pa, pb, pc, pd);
    for p in [pa, pb, pc, pd] {
        let v = ld(d, p);
        st(d, p, v * 2);
    }
}

#[inline]
fn irotate1(a: &mut W, b: &mut W) {
    *a -= (*b + 1) >> 1;
    *b += (*a + 1) >> 1;
}

#[inline]
fn irotate2(a: &mut W, b: &mut W) {
    *a -= (*b * 3 + 4) >> 3;
    *b += (*a * 3 + 4) >> 3;
}

fn irotate1_at(d: &mut [i32], pa: isize, pb: isize) {
    let (mut a, mut b) = (ld(d, pa), ld(d, pb));
    irotate1(&mut a, &mut b);
    st(d, pa, a);
    st(d, pb, b);
}

/// jxrlib `invOdd`: Kron(Rotate(-pi/8), [1 1; 1 -1]/sqrt(2)).
fn inv_odd(d: &mut [i32], pa: isize, pb: isize, pc: isize, pd: isize) {
    let (mut a, mut b, mut c, mut dd) = (ld(d, pa), ld(d, pb), ld(d, pc), ld(d, pd));
    b += dd;
    a -= c;
    dd -= b >> 1;
    c += (a + 1) >> 1;
    irotate2(&mut a, &mut b);
    irotate2(&mut c, &mut dd);
    c -= (b + 1) >> 1;
    dd = ((a + 1) >> 1) - dd;
    b += c;
    a -= dd;
    st(d, pa, a);
    st(d, pb, b);
    st(d, pc, c);
    st(d, pd, dd);
}

/// jxrlib `invOddOdd`: Kron(Rotate(pi/8), Rotate(pi/8)).
fn inv_odd_odd(d: &mut [i32], pa: isize, pb: isize, pc: isize, pd: isize) {
    let (mut a, mut b, mut c, mut dd) = (ld(d, pa), ld(d, pb), ld(d, pc), ld(d, pd));
    dd += a;
    c -= b;
    let t1 = dd >> 1;
    a -= t1;
    let t2 = c >> 1;
    b += t2;
    a -= (b * 3 + 3) >> 3;
    b += (a * 3 + 3) >> 2;
    a -= (b * 3 + 4) >> 3;
    b -= t2;
    a += t1;
    c += b;
    dd -= a;
    st(d, pa, a);
    st(d, pb, -b);
    st(d, pc, -c);
    st(d, pd, dd);
}

/// jxrlib `invOddOddPost`.
fn inv_odd_odd_post(d: &mut [i32], pa: isize, pb: isize, pc: isize, pd: isize) {
    let (mut a, mut b, mut c, mut dd) = (ld(d, pa), ld(d, pb), ld(d, pc), ld(d, pd));
    dd += a;
    c -= b;
    let t1 = dd >> 1;
    a -= t1;
    let t2 = c >> 1;
    b += t2;
    a -= (b * 3 + 6) >> 3;
    b += (a * 3 + 2) >> 2;
    a -= (b * 3 + 4) >> 3;
    b -= t2;
    a += t1;
    c += b;
    dd -= a;
    st(d, pa, a);
    st(d, pb, b);
    st(d, pc, c);
    st(d, pd, dd);
}

/// jxrlib `strIDCT4x4Stage1`: inverse core transform of one 4x4 block.
pub(crate) fn idct4x4_stage1(d: &mut [i32], p: isize) {
    dct2x2up(d, p, p + 1, p + 2, p + 3);
    inv_odd(d, p + 5, p + 4, p + 7, p + 6);
    inv_odd(d, p + 10, p + 8, p + 11, p + 9);
    inv_odd_odd(d, p + 15, p + 14, p + 13, p + 12);
    for i in 0..4 {
        dct2x2dn(d, p + i, p + 4 + i, p + 8 + i, p + 12 + i);
    }
}

/// jxrlib `strIDCT4x4Stage2`: inverse core transform of the 16 block DCs of a macroblock.
fn idct4x4_stage2(d: &mut [i32], p: isize) {
    inv_odd(d, p + 32, p + 48, p + 96, p + 112);
    inv_odd(d, p + 128, p + 192, p + 144, p + 208);
    inv_odd_odd(d, p + 160, p + 224, p + 176, p + 240);
    dct2x2up(d, p, p + 64, p + 16, p + 80);
    dct2x2dn(d, p, p + 192, p + 48, p + 240);
    dct2x2dn(d, p + 64, p + 128, p + 112, p + 176);
    dct2x2dn(d, p + 16, p + 208, p + 32, p + 224);
    dct2x2dn(d, p + 80, p + 144, p + 96, p + 160);
}

/// jxrlib `strNormalizeDec`.
fn normalize_dec(d: &mut [i32], p: isize, chroma: bool) {
    if chroma {
        for i in (0..256).step_by(16) {
            let v = ld(d, p + i);
            st(d, p + i, v + v);
        }
    }
}

/// jxrlib `strPost2`.
fn post2(d: &mut [i32], pa: isize, pb: isize) {
    let (mut a, mut b) = (ld(d, pa), ld(d, pb));
    b += (a + 4) >> 3;
    a += (b + 2) >> 2;
    b += (a + 4) >> 3;
    st(d, pa, a);
    st(d, pb, b);
}

/// jxrlib `strPost2_alternate`.
fn post2_alt(d: &mut [i32], pa: isize, pb: isize) {
    let (mut a, mut b) = (ld(d, pa), ld(d, pb));
    b += (a + 2) >> 2;
    a += (b + 1) >> 1;
    a += b >> 5;
    a += b >> 9;
    a += b >> 13;
    b += (a + 2) >> 2;
    st(d, pa, a);
    st(d, pb, b);
}

/// jxrlib `strPost2x2` / `strPost2x2_alternate`.
fn post2x2(d: &mut [i32], pa: isize, pb: isize, pc: isize, pd: isize, alternate: bool) {
    let (mut a, mut b, mut c, mut dd) = (ld(d, pa), ld(d, pb), ld(d, pc), ld(d, pd));
    a += dd;
    b += c;
    dd -= (a + 1) >> 1;
    c -= (b + 1) >> 1;
    b += (a + 2) >> 2;
    a += (b + 1) >> 1;
    if alternate {
        a += b >> 5;
        a += b >> 9;
        a += b >> 13;
    }
    b += (a + 2) >> 2;
    dd += (a + 1) >> 1;
    c += (b + 1) >> 1;
    a -= dd;
    b -= c;
    st(d, pa, a);
    st(d, pb, b);
    st(d, pc, c);
    st(d, pd, dd);
}

/// jxrlib `strPost4`: 4-point overlap filter along an image edge.
fn post4(d: &mut [i32], pa: isize, pb: isize, pc: isize, pd: isize) {
    let (mut a, mut b, mut c, mut dd) = (ld(d, pa), ld(d, pb), ld(d, pc), ld(d, pd));
    a += dd;
    b += c;
    dd -= (a + 1) >> 1;
    c -= (b + 1) >> 1;
    irotate1(&mut c, &mut dd);
    dd += (a + 1) >> 1;
    c += (b + 1) >> 1;
    a -= dd - ((dd * 3 + 16) >> 5);
    b -= c - ((c * 3 + 16) >> 5);
    dd += (a * 3 + 8) >> 4;
    c += (b * 3 + 8) >> 4;
    a += (dd * 3 + 16) >> 5;
    b += (c * 3 + 16) >> 5;
    st(d, pa, a);
    st(d, pb, b);
    st(d, pc, c);
    st(d, pd, dd);
}

/// jxrlib `strHSTdec1_edge`.
fn hst_dec1_edge(a: &mut W, d: &mut W) {
    *a += *d;
    *d = (*a >> 1) - *d;
    *a += (*d * 3) >> 3;
    *d += (*a * 3) >> 4;
    *d += *a >> 7;
    *d -= *a >> 10;
    *a += (*d * 3 + 4) >> 3;
    *d -= *a >> 1;
    *a += *d;
    *d = -*d;
}

/// jxrlib `strPost4_alternate`.
fn post4_alt(d: &mut [i32], pa: isize, pb: isize, pc: isize, pd: isize) {
    let (mut a, mut b, mut c, mut dd) = (ld(d, pa), ld(d, pb), ld(d, pc), ld(d, pd));
    a += dd;
    b += c;
    dd -= (a + 1) >> 1;
    c -= (b + 1) >> 1;
    hst_dec1_edge(&mut a, &mut dd);
    hst_dec1_edge(&mut b, &mut c);
    irotate1(&mut c, &mut dd);
    dd += (a + 1) >> 1;
    c += (b + 1) >> 1;
    a -= dd;
    b -= c;
    st(d, pa, a);
    st(d, pb, b);
    st(d, pc, c);
    st(d, pd, dd);
}

/// jxrlib `strHSTdec1` / `strHSTdec1_alternate`.
fn hst_dec1(d: &mut [i32], pa: isize, pd: isize, alternate: bool) {
    let (mut a, mut dd) = (ld(d, pa), ld(d, pd));
    a += dd;
    dd = (a >> 1) - dd;
    a += (dd * 3) >> 3;
    dd += (a * 3) >> 4;
    if alternate {
        dd += a >> 7;
        dd -= a >> 10;
    }
    st(d, pa, a);
    st(d, pd, dd);
}

/// jxrlib `strHSTdec`.
fn hst_dec(d: &mut [i32], pa: isize, pb: isize, pc: isize, pd: isize) {
    let (mut a, mut b, mut c, mut dd) = (ld(d, pa), ld(d, pb), ld(d, pc), ld(d, pd));
    b -= c;
    a += (dd * 3 + 4) >> 3;
    dd -= b >> 1;
    c = ((a - b) >> 1) - c;
    st(d, pc, dd);
    st(d, pd, c);
    st(d, pa, a - c);
    st(d, pb, b + dd);
}

fn clip_dcl(dcl: i32, alt: i32) -> i32 {
    if dcl > 0 {
        if alt > 0 { dcl.min(alt) } else { 0 }
    } else if dcl < 0 {
        if alt < 0 { dcl.max(alt) } else { 0 }
    } else {
        0
    }
}

/// jxrlib `strPost4x4Stage1Split` (original operators, with the decoder's DC-leakage
/// compensation) and `strPost4x4Stage1Split_alternate` (`hp` is `None`).
fn post4x4_stage1_split(
    d: &mut [i32],
    p0: isize,
    p1: isize,
    offset: isize,
    hp: Option<(i32, bool)>,
) {
    let alternate = hp.is_none();
    let p2 = p0 + 72 - offset;
    let p3 = p1 + 64 - offset;
    let p0 = p0 + 12;
    let p1 = p1 + 4;
    for i in 0..4 {
        dct2x2dn(d, p0 + i, p2 + i, p1 + i, p3 + i);
    }
    inv_odd_odd_post(d, p3, p3 + 1, p3 + 2, p3 + 3);
    irotate1_at(d, p1 + 2, p1 + 3);
    irotate1_at(d, p1, p1 + 1);
    irotate1_at(d, p2 + 1, p2 + 3);
    irotate1_at(d, p2, p2 + 2);
    for i in 0..4 {
        hst_dec1(d, p0 + i, p3 + i, alternate);
    }
    for i in 0..4 {
        hst_dec(d, p0 + i, p2 + i, p1 + i, p3 + i);
    }
    if let Some((hpqp, hp_absent)) = hp {
        let mut dcl = [0i32; 4];
        for (i, v) in dcl.iter_mut().enumerate() {
            let i = i as isize;
            let tmp = (ld(d, p0 + i) + ld(d, p1 + i) + ld(d, p2 + i) + ld(d, p3 + i)) >> 1;
            *v = ((tmp * 595 + 65536) >> 17).0;
        }
        for (i, &v) in dcl.iter().enumerate() {
            let i = i as isize;
            if (v.wrapping_abs() < hpqp && hpqp > 20) || hp_absent {
                let alt = ((ld(d, p0 + i) - ld(d, p1 + i) - ld(d, p2 + i) + ld(d, p3 + i)) >> 1).0;
                let c = W(clip_dcl(v, alt)) >> 1;
                // DCCompensate(p0, p2, p1, p3)
                let (a, b, cc, dd) = (p0 + i, p2 + i, p1 + i, p3 + i);
                let va = ld(d, a) - c;
                st(d, a, va);
                let vd = ld(d, dd) - c;
                st(d, dd, vd);
                let vb = ld(d, b) + c;
                st(d, b, vb);
                let vc = ld(d, cc) + c;
                st(d, cc, vc);
            }
        }
    }
}

fn post4x4_stage1(d: &mut [i32], p: isize, offset: isize, hp: Option<(i32, bool)>) {
    post4x4_stage1_split(d, p, p + 16, offset, hp);
}

/// jxrlib `strPost4x4Stage2Split` / `_alternate`.
fn post4x4_stage2_split(d: &mut [i32], p0: isize, p1: isize, alternate: bool) {
    dct2x2dn(d, p0 - 96, p0 + 96, p1 - 112, p1 + 80);
    dct2x2dn(d, p0 - 32, p0 + 32, p1 - 48, p1 + 16);
    dct2x2dn(d, p0 - 80, p0 + 112, p1 - 128, p1 + 64);
    dct2x2dn(d, p0 - 16, p0 + 48, p1 - 64, p1);
    inv_odd_odd_post(d, p1, p1 + 64, p1 + 16, p1 + 80);
    irotate1_at(d, p0 + 48, p0 + 32);
    irotate1_at(d, p0 + 112, p0 + 96);
    irotate1_at(d, p1 - 64, p1 - 128);
    irotate1_at(d, p1 - 48, p1 - 112);
    hst_dec1(d, p0 - 96, p1 + 80, alternate);
    hst_dec1(d, p0 - 32, p1 + 16, alternate);
    hst_dec1(d, p0 - 80, p1 + 64, alternate);
    hst_dec1(d, p0 - 16, p1, alternate);
    hst_dec(d, p0 - 96, p1 - 112, p0 + 96, p1 + 80);
    hst_dec(d, p0 - 32, p1 - 48, p0 + 32, p1 + 16);
    hst_dec(d, p0 - 80, p1 - 128, p0 + 112, p1 + 64);
    hst_dec(d, p0 - 16, p1 - 64, p0 + 48, p1);
}

/// Where the current macroblock position is (jxrlib `cColumn`, `cRow` against the image size;
/// `right` and `bottom` are the extra pass one past the last column and row).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Edges {
    pub(crate) left: bool,
    pub(crate) right: bool,
    pub(crate) top: bool,
    pub(crate) bottom: bool,
    pub(crate) column: usize,
    pub(crate) row: usize,
    pub(crate) mb_width: usize,
}

/// What the transform needs to know about the codestream.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Setup {
    pub(crate) cf: u8,
    pub(crate) channels: usize,
    pub(crate) overlap: u8,
    pub(crate) scaled: bool,
    pub(crate) hp_absent: bool,
}

/// jxrlib `invTransformMacroblock` (codec sub-version 0). `hpqp[i]` is the HP quantizer of
/// channel `i` for the current macroblock; the chroma passes index it with 0 and 1, as jxrlib.
pub(crate) fn inv_transform_original(
    d: &mut [i32],
    p0s: &[isize],
    p1s: &[isize],
    s: Setup,
    e: Edges,
    hpqp: &[i32],
) {
    let Edges {
        left,
        right,
        top,
        bottom,
        ..
    } = e;
    let top_or_bottom = top || bottom;
    let left_or_right = left || right;
    let bottom_or_right = bottom || right;
    let two = s.overlap == 2;
    let any = s.overlap != 0;
    let luma_like = if s.cf == cf::YUV_420 || s.cf == cf::YUV_422 {
        1
    } else {
        s.channels
    };
    for i in 0..luma_like {
        let (p0, p1) = (p0s[i], p1s[i]);
        let hp = Some((if s.hp_absent { 255 } else { hpqp[i] }, s.hp_absent));
        if !bottom_or_right {
            idct4x4_stage2(d, p1);
            if s.scaled {
                normalize_dec(d, p1, i != 0);
            }
        }
        if two {
            if left_or_right && !top_or_bottom {
                let j = if left { 0 } else { -128 };
                post4(d, p0 + j + 32, p0 + j + 48, p1 + j, p1 + j + 16);
                post4(d, p0 + j + 96, p0 + j + 112, p1 + j + 64, p1 + j + 80);
            }
            if !left_or_right {
                if top_or_bottom {
                    let p = if top { p1 } else { p0 + 32 };
                    post4(d, p - 128, p - 64, p, p + 64);
                    post4(d, p - 112, p - 48, p + 16, p + 80);
                } else {
                    post4x4_stage2_split(d, p0, p1, false);
                }
            }
        }
        if !top {
            let mut j = if left { 32 } else { -96 };
            while j < if right { 32 } else { 160 } {
                idct4x4_stage1(d, p0 + j);
                idct4x4_stage1(d, p0 + j + 16);
                j += 64;
            }
        }
        if !bottom {
            let mut j = if left { 0 } else { -128 };
            while j < if right { 0 } else { 128 } {
                idct4x4_stage1(d, p1 + j);
                idct4x4_stage1(d, p1 + j + 16);
                j += 64;
            }
        }
        if any {
            if left_or_right {
                let j = if left { 10 } else { -64 + 14 };
                if !top {
                    let p = p0 + 16 + j;
                    post4(d, p, p - 2, p + 6, p + 8);
                    post4(d, p + 1, p - 1, p + 7, p + 9);
                    post4(d, p + 16, p + 14, p + 22, p + 24);
                    post4(d, p + 17, p + 15, p + 23, p + 25);
                }
                if !bottom {
                    let p = p1 + j;
                    post4(d, p, p - 2, p + 6, p + 8);
                    post4(d, p + 1, p - 1, p + 7, p + 9);
                }
                if !top_or_bottom {
                    post4(d, p0 + 48 + j, p0 + 48 + j - 2, p1 - 10 + j, p1 - 8 + j);
                    post4(d, p0 + 48 + j + 1, p0 + 48 + j - 1, p1 - 9 + j, p1 - 7 + j);
                }
            }
            let mut j = if left { 0 } else { -192 };
            let end = if right { -64 } else { 64 };
            while j < end {
                if top {
                    let p = p1 + j;
                    post4(d, p + 5, p + 4, p + 64, p + 65);
                    post4(d, p + 7, p + 6, p + 66, p + 67);
                    post4x4_stage1(d, p1 + j, 0, hp);
                } else if bottom {
                    post4x4_stage1(d, p0 + 16 + j, 0, hp);
                    post4x4_stage1(d, p0 + 32 + j, 0, hp);
                    let p = p0 + 48 + j;
                    post4(d, p + 15, p + 14, p + 74, p + 75);
                    post4(d, p + 13, p + 12, p + 72, p + 73);
                } else {
                    post4x4_stage1(d, p0 + 16 + j, 0, hp);
                    post4x4_stage1(d, p0 + 32 + j, 0, hp);
                    post4x4_stage1_split(d, p0 + 48 + j, p1 + j, 0, hp);
                    post4x4_stage1(d, p1 + j, 0, hp);
                }
                j += 64;
            }
        }
    }

    if s.cf == cf::YUV_420 {
        for i in 0..2usize {
            let (p0, p1) = (p0s[1 + i], p1s[1 + i]);
            let hp = Some((if s.hp_absent { 255 } else { hpqp[i] }, s.hp_absent));
            if !bottom_or_right {
                if s.scaled {
                    dct2x2dn_dec(d, p1, p1 + 32, p1 + 16, p1 + 48);
                } else {
                    dct2x2dn(d, p1, p1 + 32, p1 + 16, p1 + 48);
                }
            }
            if two {
                if left_or_right && !top_or_bottom {
                    let j = if left { 0 } else { -32 };
                    post2(d, p0 + j + 16, p1 + j);
                }
                if !left_or_right {
                    if top_or_bottom {
                        let p = if top { p1 } else { p0 + 16 };
                        post2(d, p - 32, p);
                    } else {
                        post2x2(d, p0 - 16, p0 + 16, p1 - 32, p1, false);
                    }
                }
            }
            if !top {
                let mut j = if left { 16 } else { -16 };
                while j < if right { 16 } else { 48 } {
                    idct4x4_stage1(d, p0 + j);
                    j += 32;
                }
            }
            if !bottom {
                let mut j = if left { 0 } else { -32 };
                while j < if right { 0 } else { 32 } {
                    idct4x4_stage1(d, p1 + j);
                    j += 32;
                }
            }
            if any {
                if !left && !top {
                    if bottom {
                        let mut j = -48;
                        while j < if right { -16 } else { 16 } {
                            let p = p0 + j;
                            post4(d, p + 15, p + 14, p + 42, p + 43);
                            post4(d, p + 13, p + 12, p + 40, p + 41);
                            j += 32;
                        }
                    } else {
                        let mut j = -48;
                        while j < if right { -16 } else { 16 } {
                            post4x4_stage1_split(d, p0 + j, p1 - 16 + j, 32, hp);
                            j += 32;
                        }
                    }
                    if right {
                        if !bottom {
                            post4(d, p0 - 2, p0 - 4, p1 - 28, p1 - 26);
                            post4(d, p0 - 1, p0 - 3, p1 - 27, p1 - 25);
                        }
                        post4(d, p0 - 18, p0 - 20, p0 - 12, p0 - 10);
                        post4(d, p0 - 17, p0 - 19, p0 - 11, p0 - 9);
                    } else {
                        post4x4_stage1(d, p0 - 32, 32, hp);
                    }
                    post4x4_stage1(d, p0 - 64, 32, hp);
                } else if top {
                    let mut j = if left { 0 } else { -64 };
                    while j < if right { -32 } else { 0 } {
                        let p = p1 + j + 4;
                        post4(d, p + 1, p, p + 28, p + 29);
                        post4(d, p + 3, p + 2, p + 30, p + 31);
                        j += 32;
                    }
                } else if left {
                    if !bottom {
                        post4(d, p0 + 26, p0 + 24, p1, p1 + 2);
                        post4(d, p0 + 27, p0 + 25, p1 + 1, p1 + 3);
                    }
                    post4(d, p0 + 10, p0 + 8, p0 + 16, p0 + 18);
                    post4(d, p0 + 11, p0 + 9, p0 + 17, p0 + 19);
                }
            }
        }
    }

    if s.cf == cf::YUV_422 {
        for i in 0..2usize {
            let (p0, p1) = (p0s[1 + i], p1s[1 + i]);
            let hp = Some((if s.hp_absent { 255 } else { hpqp[i] }, s.hp_absent));
            if !bottom_or_right {
                let a = ld(d, p1) - ((ld(d, p1 + 32) + 1) >> 1);
                st(d, p1, a);
                let b = ld(d, p1 + 32) + a;
                st(d, p1 + 32, b);
                if s.scaled {
                    dct2x2dn_dec(d, p1, p1 + 64, p1 + 16, p1 + 80);
                    dct2x2dn_dec(d, p1 + 32, p1 + 96, p1 + 48, p1 + 112);
                } else {
                    dct2x2dn(d, p1, p1 + 64, p1 + 16, p1 + 80);
                    dct2x2dn(d, p1 + 32, p1 + 96, p1 + 48, p1 + 112);
                }
            }
            if two {
                if !bottom {
                    if left_or_right {
                        if !top {
                            let j = if left { 0 } else { -64 };
                            post2(d, p0 + 48 + j, p1 + j);
                        }
                        let j = if left { 16 } else { -48 };
                        post2(d, p1 + j, p1 + j + 16);
                    } else {
                        if top {
                            post2(d, p1 - 64, p1);
                        } else {
                            post2x2(d, p0 - 16, p0 + 48, p1 - 64, p1, false);
                        }
                        post2x2(d, p1 - 48, p1 + 16, p1 - 32, p1 + 32, false);
                    }
                } else if !left_or_right {
                    post2(d, p0 - 16, p0 + 48);
                }
            }
            if !top {
                let mut j = if left { 48 } else { -16 };
                while j < if right { 48 } else { 112 } {
                    idct4x4_stage1(d, p0 + j);
                    j += 64;
                }
            }
            if !bottom {
                let mut j = if left { 0 } else { -64 };
                while j < if right { 0 } else { 64 } {
                    idct4x4_stage1(d, p1 + j);
                    idct4x4_stage1(d, p1 + j + 16);
                    idct4x4_stage1(d, p1 + j + 32);
                    j += 64;
                }
            }
            if any {
                if !top {
                    if left_or_right {
                        let j = if left { 32 + 10 } else { -32 + 14 };
                        let p = p0 + j;
                        post4(d, p, p - 2, p + 6, p + 8);
                        post4(d, p + 1, p - 1, p + 7, p + 9);
                    }
                    let mut j = if left { 0 } else { -128 };
                    while j < if right { -64 } else { 0 } {
                        post4x4_stage1(d, p0 + j + 32, 0, hp);
                        j += 64;
                    }
                }
                if !bottom {
                    if left_or_right {
                        let j = if left { 10 } else { -64 + 14 };
                        let mut p = p1 + j;
                        post4(d, p, p - 2, p + 6, p + 8);
                        post4(d, p + 1, p - 1, p + 7, p + 9);
                        p += 16;
                        post4(d, p, p - 2, p + 6, p + 8);
                        post4(d, p + 1, p - 1, p + 7, p + 9);
                    }
                    let mut j = if left { 0 } else { -128 };
                    while j < if right { -64 } else { 0 } {
                        post4x4_stage1(d, p1 + j, 0, hp);
                        post4x4_stage1(d, p1 + j + 16, 0, hp);
                        j += 64;
                    }
                }
                if top_or_bottom {
                    let p = if top { p1 + 5 } else { p0 + 48 + 13 };
                    let mut j = if left { 0 } else { -128 };
                    while j < if right { -64 } else { 0 } {
                        post4(d, p + j, p + j - 1, p + j + 59, p + j + 60);
                        post4(d, p + j + 2, p + j + 1, p + j + 61, p + j + 62);
                        j += 64;
                    }
                } else {
                    if left_or_right {
                        let j = if left { 0 } else { -64 + 4 };
                        post4(d, p0 + j + 58, p0 + j + 56, p1 + j, p1 + j + 2);
                        post4(d, p0 + j + 59, p0 + j + 57, p1 + j + 1, p1 + j + 3);
                    }
                    let mut j = if left { 0 } else { -128 };
                    while j < if right { -64 } else { 0 } {
                        post4x4_stage1_split(d, p0 + j + 48, p1 + j, 0, hp);
                        j += 64;
                    }
                }
            }
        }
    }
}

/// Persistent state of jxrlib `invTransformMacroblock_alteredOperators_hard` (hard tile
/// boundaries and the chroma corner corrections).
#[derive(Debug, Clone, Default)]
pub(crate) struct HardState {
    mb_y: usize,
    tile_x: usize,
    tile_y: usize,
    vert_tb: bool,
    hori_tb: bool,
    one_mb_left_vtb: bool,
    one_mb_right_vtb: bool,
    pred_before: [[i32; 2]; 2],
    pred_after: [[i32; 2]; 2],
}

/// Tile layout for the hard-tile transform.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Tiles<'a> {
    pub(crate) hard: bool,
    /// jxrlib `uiTileX` (vertical slice starts) and `uiTileY` (horizontal slice starts).
    pub(crate) tile_x: &'a [u32],
    pub(crate) tile_y: &'a [u32],
}

/// jxrlib `invTransformMacroblock_alteredOperators_hard` (codec sub-versions 1 and 9).
#[allow(clippy::nonminimal_bool)]
pub(crate) fn inv_transform_altered(
    d: &mut [i32],
    p0s: &[isize],
    p1s: &[isize],
    s: Setup,
    e: Edges,
    hs: &mut HardState,
    t: Tiles<'_>,
) {
    let Edges {
        left,
        right,
        top,
        bottom,
        column,
        row,
        mb_width,
    } = e;
    let top_or_bottom = top || bottom;
    let left_or_right = left || right;
    let bottom_or_right = bottom || right;
    let left_adjacent = column == 1;
    let right_adjacent = column + 1 == mb_width;
    let nh = t.tile_y.len() - 1;
    let nv = t.tile_x.len() - 1;
    // jxrlib compares columns with the horizontal-slice table and rows with the vertical one
    // (tileY / uiTileY for columns); kept as is, since that is what the files were checked
    // against.
    if t.hard {
        if column == 0 {
            hs.vert_tb = false;
            hs.tile_y = 0;
        }
        hs.one_mb_left_vtb = false;
        hs.one_mb_right_vtb = false;
        let ty = |k: usize| t.tile_y.get(k).map_or(u64::MAX, |&v| u64::from(v));
        let tx = |k: usize| t.tile_x.get(k).map_or(u64::MAX, |&v| u64::from(v));
        let col = column as u64;
        if hs.tile_y > 0 && hs.tile_y <= nh && col.wrapping_sub(1) == ty(hs.tile_y) {
            hs.one_mb_right_vtb = true;
        }
        if hs.tile_y < nh && col == ty(hs.tile_y + 1) {
            hs.vert_tb = true;
            hs.tile_y += 1;
        } else {
            hs.vert_tb = false;
        }
        if hs.tile_y < nh && col + 1 == ty(hs.tile_y + 1) {
            hs.one_mb_left_vtb = true;
        }
        if row == 0 {
            hs.hori_tb = false;
            hs.tile_x = 0;
        } else if hs.mb_y != row && hs.tile_x < nv && row as u64 == tx(hs.tile_x + 1) {
            hs.hori_tb = true;
            hs.tile_x += 1;
        } else if hs.mb_y != row {
            hs.hori_tb = false;
        }
    } else {
        hs.vert_tb = false;
        hs.hori_tb = false;
        hs.one_mb_left_vtb = false;
        hs.one_mb_right_vtb = false;
    }
    hs.mb_y = row;
    let vtb = hs.vert_tb;
    let htb = hs.hori_tb;
    let right_vtb = hs.one_mb_right_vtb;
    let left_vtb = hs.one_mb_left_vtb;
    let two = s.overlap == 2;
    let any = s.overlap != 0;

    let luma_like = if s.cf == cf::YUV_420 || s.cf == cf::YUV_422 {
        1
    } else {
        s.channels
    };
    for i in 0..luma_like {
        let (p0, p1) = (p0s[i], p1s[i]);
        if !bottom_or_right {
            idct4x4_stage2(d, p1);
            if s.scaled {
                normalize_dec(d, p1, i != 0);
            }
        }
        if two {
            if (top || htb) && (left || vtb) {
                post4_alt(d, p1, p1 + 64, p1 + 16, p1 + 80);
            }
            if (top || htb) && (right || vtb) {
                post4_alt(d, p1 - 128, p1 - 64, p1 - 112, p1 - 48);
            }
            if (bottom || htb) && (left || vtb) {
                post4_alt(d, p0 + 32, p0 + 96, p0 + 48, p0 + 112);
            }
            if (bottom || htb) && (right || vtb) {
                post4_alt(d, p0 - 96, p0 - 32, p0 - 80, p0 - 16);
            }
            if (left_or_right || vtb) && (!top_or_bottom && !htb) {
                if left || vtb {
                    post4_alt(d, p0 + 32, p0 + 48, p1, p1 + 16);
                    post4_alt(d, p0 + 96, p0 + 112, p1 + 64, p1 + 80);
                }
                if right || vtb {
                    let j = -128;
                    post4_alt(d, p0 + j + 32, p0 + j + 48, p1 + j, p1 + j + 16);
                    post4_alt(d, p0 + j + 96, p0 + j + 112, p1 + j + 64, p1 + j + 80);
                }
            }
            if !left_or_right {
                if (top_or_bottom || htb) && !vtb {
                    if top || htb {
                        let p = p1;
                        post4_alt(d, p - 128, p - 64, p, p + 64);
                        post4_alt(d, p - 112, p - 48, p + 16, p + 80);
                    }
                    if bottom || htb {
                        let p = p0 + 32;
                        post4_alt(d, p - 128, p - 64, p, p + 64);
                        post4_alt(d, p - 112, p - 48, p + 16, p + 80);
                    }
                }
                if !top_or_bottom && !htb && !vtb {
                    post4x4_stage2_split(d, p0, p1, true);
                }
            }
        }
        if !top {
            let mut j = if left { 32 } else { -96 };
            while j < if right { 32 } else { 160 } {
                idct4x4_stage1(d, p0 + j);
                idct4x4_stage1(d, p0 + j + 16);
                j += 64;
            }
        }
        if !bottom {
            let mut j = if left { 0 } else { -128 };
            while j < if right { 0 } else { 128 } {
                idct4x4_stage1(d, p1 + j);
                idct4x4_stage1(d, p1 + j + 16);
                j += 64;
            }
        }
        if any {
            if left_or_right || vtb {
                if (top || htb) && (left || vtb) {
                    post4_alt(d, p1, p1 + 1, p1 + 2, p1 + 3);
                }
                if (top || htb) && (right || vtb) {
                    post4_alt(d, p1 - 59, p1 - 60, p1 - 57, p1 - 58);
                }
                if (bottom || htb) && (left || vtb) {
                    post4_alt(d, p0 + 58, p0 + 59, p0 + 56, p0 + 57);
                }
                if (bottom || htb) && (right || vtb) {
                    post4_alt(d, p0 - 1, p0 - 2, p0 - 3, p0 - 4);
                }
                for (on, j) in [(left || vtb, 10isize), (right || vtb, -64 + 14)] {
                    if !on {
                        continue;
                    }
                    if !top {
                        let p = p0 + 16 + j;
                        post4_alt(d, p, p - 2, p + 6, p + 8);
                        post4_alt(d, p + 1, p - 1, p + 7, p + 9);
                        post4_alt(d, p + 16, p + 14, p + 22, p + 24);
                        post4_alt(d, p + 17, p + 15, p + 23, p + 25);
                    }
                    if !bottom {
                        let p = p1 + j;
                        post4_alt(d, p, p - 2, p + 6, p + 8);
                        post4_alt(d, p + 1, p - 1, p + 7, p + 9);
                    }
                    if !top_or_bottom && !htb {
                        post4_alt(d, p0 + 48 + j, p0 + 48 + j - 2, p1 - 10 + j, p1 - 8 + j);
                        post4_alt(d, p0 + 48 + j + 1, p0 + 48 + j - 1, p1 - 9 + j, p1 - 7 + j);
                    }
                }
            }
            let start = if left { 0 } else { -192 };
            let end = if right { -64 } else { 64 };
            if top || htb {
                let mut j = start;
                while j < end {
                    if !vtb || j != -64 {
                        let p = p1 + j;
                        post4_alt(d, p + 5, p + 4, p + 64, p + 65);
                        post4_alt(d, p + 7, p + 6, p + 66, p + 67);
                        post4x4_stage1(d, p1 + j, 0, None);
                    }
                    j += 64;
                }
            }
            if bottom || htb {
                let mut j = start;
                while j < end {
                    if !vtb || j != -64 {
                        post4x4_stage1(d, p0 + 16 + j, 0, None);
                        post4x4_stage1(d, p0 + 32 + j, 0, None);
                        let p = p0 + 48 + j;
                        post4_alt(d, p + 15, p + 14, p + 74, p + 75);
                        post4_alt(d, p + 13, p + 12, p + 72, p + 73);
                    }
                    j += 64;
                }
            }
            if !top && !bottom && !htb {
                let mut j = start;
                while j < end {
                    if !vtb || j != -64 {
                        post4x4_stage1(d, p0 + 16 + j, 0, None);
                        post4x4_stage1(d, p0 + 32 + j, 0, None);
                        post4x4_stage1_split(d, p0 + 48 + j, p1 + j, 0, None);
                        post4x4_stage1(d, p1 + j, 0, None);
                    }
                    j += 64;
                }
            }
        }
    }

    if s.cf == cf::YUV_420 {
        for i in 0..2usize {
            let (p0, p1) = (p0s[1 + i], p1s[1 + i]);
            if !bottom_or_right {
                if s.scaled {
                    dct2x2dn_dec(d, p1, p1 + 32, p1 + 16, p1 + 48);
                } else {
                    dct2x2dn(d, p1, p1 + 32, p1 + 16, p1 + 48);
                }
            }
            if two {
                let tl = (left_adjacent || right_vtb) && (top || htb);
                let tr_save = (right_adjacent || left_vtb) && (top || htb);
                let tr = (right || vtb) && (top || htb);
                let bl = (left_adjacent || right_vtb) && (bottom || htb);
                let br_save = (right_adjacent || left_vtb) && (bottom || htb);
                let br = (right || vtb) && (bottom || htb);
                if tl {
                    let v = ld(d, p1 - 64) - ld(d, p1 - 64 + 32);
                    st(d, p1 - 64, v);
                }
                if tr_save {
                    hs.pred_before[i][0] = d[p1 as usize];
                }
                if tr {
                    let v = ld(d, p1 - 64 + 32) - W(hs.pred_before[i][0]);
                    st(d, p1 - 64 + 32, v);
                }
                if bl {
                    let v = ld(d, p0 - 64 + 16) - ld(d, p0 - 64 + 48);
                    st(d, p0 - 64 + 16, v);
                }
                if br_save {
                    hs.pred_before[i][1] = d[(p0 + 16) as usize];
                }
                if br {
                    let v = ld(d, p0 - 64 + 48) - W(hs.pred_before[i][1]);
                    st(d, p0 - 64 + 48, v);
                }
                if (left_or_right || vtb) && !top_or_bottom && !htb {
                    if left || vtb {
                        post2_alt(d, p0 + 16, p1);
                    }
                    if right || vtb {
                        post2_alt(d, p0 - 32 + 16, p1 - 32);
                    }
                }
                if !left_or_right {
                    if (top_or_bottom || htb) && !vtb {
                        if top || htb {
                            post2_alt(d, p1 - 32, p1);
                        }
                        if bottom || htb {
                            post2_alt(d, p0 + 16 - 32, p0 + 16);
                        }
                    } else if !top_or_bottom && !htb && !vtb {
                        post2x2(d, p0 - 16, p0 + 16, p1 - 32, p1, true);
                    }
                }
                if tl {
                    let v = ld(d, p1 - 64) + ld(d, p1 - 64 + 32);
                    st(d, p1 - 64, v);
                }
                if tr_save {
                    hs.pred_after[i][0] = d[p1 as usize];
                }
                if tr {
                    let v = ld(d, p1 - 64 + 32) + W(hs.pred_after[i][0]);
                    st(d, p1 - 64 + 32, v);
                }
                if bl {
                    let v = ld(d, p0 - 64 + 16) + ld(d, p0 - 64 + 48);
                    st(d, p0 - 64 + 16, v);
                }
                if br_save {
                    hs.pred_after[i][1] = d[(p0 + 16) as usize];
                }
                if br {
                    let v = ld(d, p0 - 64 + 48) + W(hs.pred_after[i][1]);
                    st(d, p0 - 64 + 48, v);
                }
            }
            if !top {
                let mut j = if left {
                    48
                } else if left_adjacent || right_vtb {
                    -48
                } else {
                    -16
                };
                while j < if right || vtb { 16 } else { 48 } {
                    idct4x4_stage1(d, p0 + j);
                    j += 32;
                }
            }
            if !bottom {
                let mut j = if left {
                    32
                } else if left_adjacent || right_vtb {
                    -64
                } else {
                    -32
                };
                while j < if right || vtb { 0 } else { 32 } {
                    idct4x4_stage1(d, p1 + j);
                    j += 32;
                }
            }
            if any {
                if (top || htb) && (left_adjacent || right_vtb) {
                    post4_alt(d, p1 - 64, p1 - 64 + 1, p1 - 64 + 2, p1 - 64 + 3);
                }
                if (top || htb) && (right || vtb) {
                    post4_alt(d, p1 - 27, p1 - 28, p1 - 25, p1 - 26);
                }
                if (bottom || htb) && (left_adjacent || right_vtb) {
                    post4_alt(d, p0 - 64 + 26, p0 - 64 + 27, p0 - 64 + 24, p0 - 64 + 25);
                }
                if (bottom || htb) && (right || vtb) {
                    post4_alt(d, p0 - 1, p0 - 2, p0 - 3, p0 - 4);
                }
                if !left && !top {
                    if left_adjacent || right_vtb {
                        if !bottom && !htb {
                            post4_alt(d, p0 - 64 + 26, p0 - 64 + 24, p1 - 64, p1 - 64 + 2);
                            post4_alt(d, p0 - 64 + 27, p0 - 64 + 25, p1 - 64 + 1, p1 - 64 + 3);
                        }
                        post4_alt(d, p0 - 64 + 10, p0 - 64 + 8, p0 - 64 + 16, p0 - 64 + 18);
                        post4_alt(d, p0 - 64 + 11, p0 - 64 + 9, p0 - 64 + 17, p0 - 64 + 19);
                    }
                    if bottom || htb {
                        let p = p0 - 48;
                        post4_alt(d, p + 15, p + 14, p + 42, p + 43);
                        post4_alt(d, p + 13, p + 12, p + 40, p + 41);
                        if !right && !vtb {
                            let p = p0 - 16;
                            post4_alt(d, p + 15, p + 14, p + 42, p + 43);
                            post4_alt(d, p + 13, p + 12, p + 40, p + 41);
                        }
                    } else {
                        post4x4_stage1_split(d, p0 - 48, p1 - 16 - 48, 32, None);
                        if !right && !vtb {
                            post4x4_stage1_split(d, p0 - 16, p1 - 16 - 16, 32, None);
                        }
                    }
                    if right || vtb {
                        if !bottom && !htb {
                            post4_alt(d, p0 - 2, p0 - 4, p1 - 28, p1 - 26);
                            post4_alt(d, p0 - 1, p0 - 3, p1 - 27, p1 - 25);
                        }
                        post4_alt(d, p0 - 18, p0 - 20, p0 - 12, p0 - 10);
                        post4_alt(d, p0 - 17, p0 - 19, p0 - 11, p0 - 9);
                    } else {
                        post4x4_stage1(d, p0 - 32, 32, None);
                    }
                    post4x4_stage1(d, p0 - 64, 32, None);
                }
                if top || htb {
                    if !left {
                        let p = p1 - 64 + 4;
                        post4_alt(d, p + 1, p, p + 28, p + 29);
                        post4_alt(d, p + 3, p + 2, p + 30, p + 31);
                    }
                    if !left && !right && !vtb {
                        let p = p1 - 32 + 4;
                        post4_alt(d, p + 1, p, p + 28, p + 29);
                        post4_alt(d, p + 3, p + 2, p + 30, p + 31);
                    }
                }
            }
        }
    }

    if s.cf == cf::YUV_422 {
        for i in 0..2usize {
            let (p0, p1) = (p0s[1 + i], p1s[1 + i]);
            if !bottom_or_right {
                let a = ld(d, p1) - ((ld(d, p1 + 32) + 1) >> 1);
                st(d, p1, a);
                let b = ld(d, p1 + 32) + a;
                st(d, p1 + 32, b);
                if s.scaled {
                    dct2x2dn_dec(d, p1, p1 + 64, p1 + 16, p1 + 80);
                    dct2x2dn_dec(d, p1 + 32, p1 + 96, p1 + 48, p1 + 112);
                } else {
                    dct2x2dn(d, p1, p1 + 64, p1 + 16, p1 + 80);
                    dct2x2dn(d, p1 + 32, p1 + 96, p1 + 48, p1 + 112);
                }
            }
            if two {
                let tl = (left_adjacent || right_vtb) && (top || htb);
                let tr_save = (right_adjacent || left_vtb) && (top || htb);
                let tr = (right || vtb) && (top || htb);
                let bl = (left_adjacent || right_vtb) && (bottom || htb);
                let br_save = (right_adjacent || left_vtb) && (bottom || htb);
                let br = (right || vtb) && (bottom || htb);
                if tl {
                    let v = ld(d, p1 - 128) - ld(d, p1 - 128 + 64);
                    st(d, p1 - 128, v);
                }
                if tr_save {
                    hs.pred_before[i][0] = d[p1 as usize];
                }
                if tr {
                    let v = ld(d, p1 - 128 + 64) - W(hs.pred_before[i][0]);
                    st(d, p1 - 128 + 64, v);
                }
                if bl {
                    let v = ld(d, p0 - 128 + 48) - ld(d, p0 - 128 + 112);
                    st(d, p0 - 128 + 48, v);
                }
                if br_save {
                    hs.pred_before[i][1] = d[(p0 + 48) as usize];
                }
                if br {
                    let v = ld(d, p0 - 128 + 112) - W(hs.pred_before[i][1]);
                    st(d, p0 - 128 + 112, v);
                }
                if !bottom {
                    if left_or_right || vtb {
                        if !top && !htb {
                            if left || vtb {
                                post2_alt(d, p0 + 48, p1);
                            }
                            if right || vtb {
                                post2_alt(d, p0 + 48 - 64, p1 - 64);
                            }
                        }
                        if left || vtb {
                            post2_alt(d, p1 + 16, p1 + 32);
                        }
                        if right || vtb {
                            post2_alt(d, p1 - 48, p1 - 32);
                        }
                    }
                    if !left_or_right && !vtb {
                        if top || htb {
                            post2_alt(d, p1 - 64, p1);
                        } else {
                            post2x2(d, p0 - 16, p0 + 48, p1 - 64, p1, true);
                        }
                        post2x2(d, p1 - 48, p1 + 16, p1 - 32, p1 + 32, true);
                    }
                }
                if (bottom || htb) && (!left_or_right && !vtb) {
                    post2_alt(d, p0 - 16, p0 + 48);
                }
                if tl {
                    let v = ld(d, p1 - 128) + ld(d, p1 - 128 + 64);
                    st(d, p1 - 128, v);
                }
                if tr_save {
                    hs.pred_after[i][0] = d[p1 as usize];
                }
                if tr {
                    let v = ld(d, p1 - 128 + 64) + W(hs.pred_after[i][0]);
                    st(d, p1 - 128 + 64, v);
                }
                if bl {
                    let v = ld(d, p0 - 128 + 48) + ld(d, p0 - 128 + 112);
                    st(d, p0 - 128 + 48, v);
                }
                if br_save {
                    hs.pred_after[i][1] = d[(p0 + 48) as usize];
                }
                if br {
                    let v = ld(d, p0 - 128 + 112) + W(hs.pred_after[i][1]);
                    st(d, p0 - 128 + 112, v);
                }
            }
            if !top {
                let mut j = if left {
                    112
                } else if left_adjacent || right_vtb {
                    -80
                } else {
                    -16
                };
                while j < if right || vtb { 48 } else { 112 } {
                    idct4x4_stage1(d, p0 + j);
                    j += 64;
                }
            }
            if !bottom {
                let mut j = if left {
                    64
                } else if left_adjacent || right_vtb {
                    -128
                } else {
                    -64
                };
                while j < if right || vtb { 0 } else { 64 } {
                    idct4x4_stage1(d, p1 + j);
                    idct4x4_stage1(d, p1 + j + 16);
                    idct4x4_stage1(d, p1 + j + 32);
                    j += 64;
                }
            }
            if any {
                if (top || htb) && (left_adjacent || right_vtb) {
                    post4_alt(d, p1 - 128, p1 - 128 + 1, p1 - 128 + 2, p1 - 128 + 3);
                }
                if (top || htb) && (right || vtb) {
                    post4_alt(d, p1 - 59, p1 - 60, p1 - 57, p1 - 58);
                }
                if (bottom || htb) && (left_adjacent || right_vtb) {
                    post4_alt(
                        d,
                        p0 - 128 + 58,
                        p0 - 128 + 59,
                        p0 - 128 + 56,
                        p0 - 128 + 57,
                    );
                }
                if (bottom || htb) && (right || vtb) {
                    post4_alt(d, p0 - 1, p0 - 2, p0 - 3, p0 - 4);
                }
                if !top {
                    if left_adjacent || right_vtb {
                        let p = p0 + 32 + 10 - 128;
                        post4_alt(d, p, p - 2, p + 6, p + 8);
                        post4_alt(d, p + 1, p - 1, p + 7, p + 9);
                    }
                    if right || vtb {
                        let p = p0 - 32 + 14;
                        post4_alt(d, p, p - 2, p + 6, p + 8);
                        post4_alt(d, p + 1, p - 1, p + 7, p + 9);
                    }
                    let mut j = if left { 0 } else { -128 };
                    while j < if right || vtb { -64 } else { 0 } {
                        post4x4_stage1(d, p0 + j + 32, 0, None);
                        j += 64;
                    }
                }
                if !bottom {
                    if left_adjacent || right_vtb {
                        let mut p = p1 + 10 - 128;
                        post4_alt(d, p, p - 2, p + 6, p + 8);
                        post4_alt(d, p + 1, p - 1, p + 7, p + 9);
                        p += 16;
                        post4_alt(d, p, p - 2, p + 6, p + 8);
                        post4_alt(d, p + 1, p - 1, p + 7, p + 9);
                    }
                    if right || vtb {
                        let mut p = p1 - 64 + 14;
                        post4_alt(d, p, p - 2, p + 6, p + 8);
                        post4_alt(d, p + 1, p - 1, p + 7, p + 9);
                        p += 16;
                        post4_alt(d, p, p - 2, p + 6, p + 8);
                        post4_alt(d, p + 1, p - 1, p + 7, p + 9);
                    }
                    let mut j = if left { 0 } else { -128 };
                    while j < if right || vtb { -64 } else { 0 } {
                        post4x4_stage1(d, p1 + j, 0, None);
                        post4x4_stage1(d, p1 + j + 16, 0, None);
                        j += 64;
                    }
                }
                if top_or_bottom || htb {
                    if top || htb {
                        let p = p1 + 5;
                        let mut j = if left { 0 } else { -128 };
                        while j < if right || vtb { -64 } else { 0 } {
                            post4_alt(d, p + j, p + j - 1, p + j + 59, p + j + 60);
                            post4_alt(d, p + j + 2, p + j + 1, p + j + 61, p + j + 62);
                            j += 64;
                        }
                    }
                    if bottom || htb {
                        let p = p0 + 48 + 13;
                        let mut j = if left { 0 } else { -128 };
                        while j < if right || vtb { -64 } else { 0 } {
                            post4_alt(d, p + j, p + j - 1, p + j + 59, p + j + 60);
                            post4_alt(d, p + j + 2, p + j + 1, p + j + 61, p + j + 62);
                            j += 64;
                        }
                    }
                } else {
                    if left_adjacent || right_vtb {
                        let j = -128;
                        post4_alt(d, p0 + j + 58, p0 + j + 56, p1 + j, p1 + j + 2);
                        post4_alt(d, p0 + j + 59, p0 + j + 57, p1 + j + 1, p1 + j + 3);
                    }
                    if right || vtb {
                        let j = -64 + 4;
                        post4_alt(d, p0 + j + 58, p0 + j + 56, p1 + j, p1 + j + 2);
                        post4_alt(d, p0 + j + 59, p0 + j + 57, p1 + j + 1, p1 + j + 3);
                    }
                    let mut j = if left { 0 } else { -128 };
                    while j < if right || vtb { -64 } else { 0 } {
                        post4x4_stage1_split(d, p0 + j + 48, p1 + j, 0, None);
                        j += 64;
                    }
                }
            }
        }
    }
}
