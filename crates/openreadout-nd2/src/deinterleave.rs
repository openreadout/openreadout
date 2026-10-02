//! Channel de-interleaving of ND2 frames: a frame stores every component of a pixel next to
//! each other (`c0 c1 … cN-1`, rows padded to `stride` bytes); a plane is one channel's span of
//! components. Splitting a whole frame in one pass (every channel at once) reads the frame once
//! however many channels are asked for; the fixed-size paths let the compiler unroll the copy
//! for the common layouts (1, 2 or 4 bytes per channel, 2–8 channels).

/// One channel's bytes within a pixel: `(offset, length)`.
pub(crate) type Span = (usize, usize);

/// Split every channel of a `w` × `h` frame whose rows start `stride` bytes apart and whose
/// pixels are `px_in` bytes: one plane per span, rows packed. `raw` must hold `stride * (h - 1)
/// + w * px_in` bytes and every span must lie inside the pixel (the caller checks both).
pub(crate) fn split_channels(
    raw: &[u8],
    stride: usize,
    w: usize,
    h: usize,
    px_in: usize,
    spans: &[Span],
) -> Vec<Vec<u8>> {
    let mut outs: Vec<Vec<u8>> = spans.iter().map(|&(_, n)| vec![0u8; w * h * n]).collect();
    if w == 0 || h == 0 || px_in == 0 || spans.is_empty() {
        return outs;
    }
    let uniform = spans
        .iter()
        .enumerate()
        .all(|(i, &(o, n))| o == i * spans[0].1 && n == spans[0].1)
        && spans.len() * spans[0].1 == px_in;
    macro_rules! fixed {
        ($b:literal, $c:literal) => {{
            // Row by row (a row stays in L1), one channel at a time: the copy of each sample
            // is a fixed-size move the compiler unrolls.
            for y in 0..h {
                let row = &raw[y * stride..y * stride + w * $b * $c];
                let px = row.as_chunks::<{ $b * $c }>().0;
                for c in 0..$c {
                    let dst = &mut outs[c][y * w * $b..(y + 1) * w * $b];
                    for (d, s) in dst.as_chunks_mut::<$b>().0.iter_mut().zip(px) {
                        d.copy_from_slice(&s[c * $b..c * $b + $b]);
                    }
                }
            }
            return outs;
        }};
    }
    if uniform {
        match (spans[0].1, spans.len()) {
            (1, 2) => fixed!(1, 2),
            (1, 3) => fixed!(1, 3),
            (1, 4) => fixed!(1, 4),
            (2, 2) => fixed!(2, 2),
            (2, 3) => fixed!(2, 3),
            (2, 4) => fixed!(2, 4),
            (2, 5) => fixed!(2, 5),
            (2, 6) => fixed!(2, 6),
            (2, 7) => fixed!(2, 7),
            (2, 8) => fixed!(2, 8),
            (4, 2) => fixed!(4, 2),
            (4, 3) => fixed!(4, 3),
            (4, 4) => fixed!(4, 4),
            _ => {}
        }
    }
    for y in 0..h {
        let row = &raw[y * stride..y * stride + w * px_in];
        for (x, px) in row.chunks_exact(px_in).enumerate() {
            for (out, &(o, n)) in outs.iter_mut().zip(spans) {
                let at = (y * w + x) * n;
                out[at..at + n].copy_from_slice(&px[o..o + n]);
            }
        }
    }
    outs
}

/// One channel (`span`) of the frame, as [`split_channels`] would return it.
pub(crate) fn gather_channel(
    raw: &[u8],
    stride: usize,
    w: usize,
    h: usize,
    px_in: usize,
    span: Span,
) -> Vec<u8> {
    let (off, n) = span;
    let mut out = vec![0u8; w * h * n];
    if w == 0 || h == 0 || n == 0 || px_in == 0 {
        return out;
    }
    if off == 0 && n == px_in {
        // The channel is the whole pixel: only the row padding goes.
        for (y, dst) in out.chunks_exact_mut(w * n).enumerate() {
            dst.copy_from_slice(&raw[y * stride..y * stride + w * n]);
        }
        return out;
    }
    macro_rules! fixed {
        ($i:literal, $o:literal) => {{
            for (y, dst) in out.chunks_exact_mut(w * $o).enumerate() {
                let row = &raw[y * stride..y * stride + w * $i];
                for (s, d) in row
                    .as_chunks::<$i>()
                    .0
                    .iter()
                    .zip(dst.as_chunks_mut::<$o>().0.iter_mut())
                {
                    d.copy_from_slice(&s[off..off + $o]);
                }
            }
            return out;
        }};
    }
    match (px_in, n) {
        (2, 1) => fixed!(2, 1),
        (3, 1) => fixed!(3, 1),
        (4, 1) => fixed!(4, 1),
        (4, 2) => fixed!(4, 2),
        (6, 2) => fixed!(6, 2),
        (8, 2) => fixed!(8, 2),
        (10, 2) => fixed!(10, 2),
        (12, 2) => fixed!(12, 2),
        (14, 2) => fixed!(14, 2),
        (16, 2) => fixed!(16, 2),
        (8, 4) => fixed!(8, 4),
        (12, 4) => fixed!(12, 4),
        (16, 4) => fixed!(16, 4),
        _ => {}
    }
    for (y, dst) in out.chunks_exact_mut(w * n).enumerate() {
        let row = &raw[y * stride..y * stride + w * px_in];
        for (s, d) in row.chunks_exact(px_in).zip(dst.chunks_exact_mut(n)) {
            d.copy_from_slice(&s[off..off + n]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Straightforward reference: byte by byte.
    fn naive(raw: &[u8], stride: usize, w: usize, h: usize, px_in: usize, span: Span) -> Vec<u8> {
        let mut out = Vec::new();
        for y in 0..h {
            for x in 0..w {
                for k in 0..span.1 {
                    out.push(raw[y * stride + x * px_in + span.0 + k]);
                }
            }
        }
        out
    }

    #[test]
    fn fast_paths_match_the_reference() {
        let mut seed = 0x9E37_79B9_u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed & 0xFF) as u8
        };
        for &(bps, nch) in &[
            (1, 2),
            (1, 3),
            (1, 4),
            (2, 2),
            (2, 3),
            (2, 4),
            (2, 5),
            (2, 6),
            (2, 7),
            (2, 8),
            (4, 2),
            (4, 3),
            (4, 4),
            (2, 9),
            (1, 1),
        ] {
            let (w, h) = (7, 5);
            let px_in = bps * nch;
            let stride = w * px_in + 3; // padded rows
            let raw: Vec<u8> = (0..stride * h).map(|_| next()).collect();
            let spans: Vec<Span> = (0..nch).map(|c| (c * bps, bps)).collect();
            let all = split_channels(&raw, stride, w, h, px_in, &spans);
            for (c, &s) in spans.iter().enumerate() {
                let want = naive(&raw, stride, w, h, px_in, s);
                assert_eq!(all[c], want, "split bps={bps} nch={nch} c={c}");
                assert_eq!(
                    gather_channel(&raw, stride, w, h, px_in, s),
                    want,
                    "gather bps={bps} nch={nch} c={c}"
                );
            }
        }
        // uneven spans (an RGB channel next to a mono one) take the general path
        let (w, h, px_in) = (4, 3, 8);
        let raw: Vec<u8> = (0..w * h * px_in).map(|_| next()).collect();
        let spans = [(0, 6), (6, 2)];
        let all = split_channels(&raw, w * px_in, w, h, px_in, &spans);
        for (c, &s) in spans.iter().enumerate() {
            assert_eq!(all[c], naive(&raw, w * px_in, w, h, px_in, s));
            assert_eq!(gather_channel(&raw, w * px_in, w, h, px_in, s), all[c]);
        }
    }
}
