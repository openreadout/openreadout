//! Sample-level helpers on little-endian plane bytes: 2×2 mean downsampling, RGB sample
//! (de)interleaving and value ranges.

use openreadout_core::PixelType;
use rayon::prelude::*;

use crate::ngff::ChannelRange;

macro_rules! for_type {
    ($pt:expr, $mac:ident, $unknown:expr) => {
        match $pt {
            PixelType::Int8 => $mac!(i8, 1),
            PixelType::Int16 => $mac!(i16, 2),
            PixelType::Int32 => $mac!(i32, 4),
            PixelType::Uint8 => $mac!(u8, 1),
            PixelType::Uint16 => $mac!(u16, 2),
            PixelType::Uint32 => $mac!(u32, 4),
            PixelType::Float => $mac!(f32, 4),
            PixelType::Double => $mac!(f64, 8),
            // A pixel type added to the core model after this writer: the export entry
            // points reject it before any pixels are processed.
            _ => $unknown,
        }
    };
}

/// Conversion of an accumulated mean back to the sample type.
trait FromMean: Copy {
    fn from_mean(v: f64) -> Self;
    fn to_f64(self) -> f64;
}

macro_rules! int_mean {
    ($($t:ty),*) => {$(
        impl FromMean for $t {
            fn from_mean(v: f64) -> Self {
                // Round half away from zero; the mean of in-range values is in range.
                v.round().clamp(<$t>::MIN as f64, <$t>::MAX as f64) as $t
            }
            fn to_f64(self) -> f64 {
                self as f64
            }
        }
    )*};
}
int_mean!(i8, i16, i32, u8, u16, u32);

impl FromMean for f32 {
    fn from_mean(v: f64) -> Self {
        v as f32
    }
    fn to_f64(self) -> f64 {
        f64::from(self)
    }
}
impl FromMean for f64 {
    fn from_mean(v: f64) -> Self {
        v
    }
    fn to_f64(self) -> f64 {
        self
    }
}

/// Halve a single-sample plane in both dimensions (output `ceil(w/2) × ceil(h/2)`), each
/// output pixel being the mean of the (up to) 2×2 input block it covers.
/// `data` must hold exactly `w * h` samples of `pt`, little-endian.
pub fn downsample_2x(data: &[u8], w: u32, h: u32, pt: PixelType) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let (ow, oh) = (w.div_ceil(2), h.div_ceil(2));
    macro_rules! go {
        ($t:ty, $n:expr) => {{
            let src: Vec<$t> = data
                .as_chunks::<$n>()
                .0
                .iter()
                .map(|c| <$t>::from_le_bytes(*c))
                .collect();
            let mut out = Vec::with_capacity(ow * oh * $n);
            for oy in 0..oh {
                let y0 = oy * 2;
                let y1 = (y0 + 1).min(h - 1);
                for ox in 0..ow {
                    let x0 = ox * 2;
                    let x1 = (x0 + 1).min(w - 1);
                    let mut sum = 0.0f64;
                    let mut n = 0.0f64;
                    for y in [y0, y1].into_iter().take(if y1 == y0 { 1 } else { 2 }) {
                        for x in [x0, x1].into_iter().take(if x1 == x0 { 1 } else { 2 }) {
                            if let Some(v) = src.get(y * w + x) {
                                sum += v.to_f64();
                                n += 1.0;
                            }
                        }
                    }
                    let m = if n > 0.0 { sum / n } else { 0.0 };
                    out.extend_from_slice(&<$t>::from_mean(m).to_le_bytes());
                }
            }
            out
        }};
    }
    if w == 0 || h == 0 {
        return Vec::new();
    }
    for_type!(pt, go, Vec::new())
}

/// Pixels per task when a block is shuffled or scanned on the thread pool.
const PIXELS_PER_TASK: usize = 1 << 16;

/// Split interleaved samples (`spp` per pixel) into `spp` single-sample planes.
pub fn deinterleave(data: &[u8], spp: usize, bytes_per_sample: usize) -> Vec<Vec<u8>> {
    if spp <= 1 {
        return vec![data.to_vec()];
    }
    let bps = bytes_per_sample.max(1);
    let px = spp * bps;
    let n = data.len() / px;
    let src = &data[..n * px];
    (0..spp)
        .map(|s| {
            let mut plane = vec![0u8; n * bps];
            plane
                .par_chunks_mut(PIXELS_PER_TASK * bps)
                .zip(src.par_chunks(PIXELS_PER_TASK * px))
                .for_each(|(out, pixels)| {
                    if bps == 1 {
                        for (o, p) in out.iter_mut().zip(pixels.chunks_exact(px)) {
                            *o = p[s];
                        }
                    } else {
                        for (o, p) in out.chunks_exact_mut(bps).zip(pixels.chunks_exact(px)) {
                            o.copy_from_slice(&p[s * bps..(s + 1) * bps]);
                        }
                    }
                });
            plane
        })
        .collect()
}

/// Inverse of [`deinterleave`]. Planes must have equal length (only the samples all of them
/// hold are interleaved).
pub fn interleave(planes: &[Vec<u8>], bytes_per_sample: usize) -> Vec<u8> {
    if planes.len() == 1 {
        return planes[0].clone();
    }
    let bps = bytes_per_sample.max(1);
    let spp = planes.len();
    let px = spp * bps;
    let n = planes.iter().map(|p| p.len() / bps).min().unwrap_or(0);
    let mut out = vec![0u8; n * px];
    out.par_chunks_mut(PIXELS_PER_TASK * px)
        .enumerate()
        .for_each(|(k, chunk)| {
            let first = k * PIXELS_PER_TASK;
            for (s, p) in planes.iter().enumerate() {
                let samples = &p[first * bps..(first + chunk.len() / px) * bps];
                if bps == 1 {
                    for (o, v) in chunk.chunks_exact_mut(px).zip(samples) {
                        o[s] = *v;
                    }
                } else {
                    for (o, v) in chunk.chunks_exact_mut(px).zip(samples.chunks_exact(bps)) {
                        o[s * bps..(s + 1) * bps].copy_from_slice(v);
                    }
                }
            }
        });
    out
}

/// Min and max sample value of a single-sample plane (NaNs ignored). Equal to a scan from the
/// first sample to the last that keeps the first of equal values (`-0.0` and `0.0` compare
/// equal), whatever the thread count: the parts are scanned on the thread pool and merged in
/// order by the same rule.
pub fn range(data: &[u8], pt: PixelType) -> ChannelRange {
    fn scan<T: FromMean, const N: usize>(data: &[u8], from: fn([u8; N]) -> T) -> ChannelRange {
        let part = |chunk: &[u8]| {
            let mut r = ChannelRange::EMPTY;
            for c in chunk.as_chunks::<N>().0 {
                let v = from(*c).to_f64();
                if v < r.min {
                    r.min = v;
                }
                if v > r.max {
                    r.max = v;
                }
            }
            r
        };
        data.par_chunks(PIXELS_PER_TASK * N).map(part).reduce(
            || ChannelRange::EMPTY,
            |a, b| ChannelRange {
                min: if b.min < a.min { b.min } else { a.min },
                max: if b.max > a.max { b.max } else { a.max },
            },
        )
    }
    // Integers have no signed zero or NaN, so their parts can be scanned with integer
    // comparisons and converted once.
    fn scan_int<T: FromMean + Ord + Send, const N: usize>(
        data: &[u8],
        from: fn([u8; N]) -> T,
    ) -> ChannelRange {
        data.par_chunks(PIXELS_PER_TASK * N)
            .filter_map(|chunk| {
                let mut it = chunk.as_chunks::<N>().0.iter().map(|c| from(*c));
                let first = it.next()?;
                Some(it.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v))))
            })
            .map(|(lo, hi)| ChannelRange {
                min: lo.to_f64(),
                max: hi.to_f64(),
            })
            .reduce(
                || ChannelRange::EMPTY,
                |a, b| ChannelRange {
                    min: a.min.min(b.min),
                    max: a.max.max(b.max),
                },
            )
    }
    match pt {
        PixelType::Int8 => scan_int(data, i8::from_le_bytes),
        PixelType::Int16 => scan_int(data, i16::from_le_bytes),
        PixelType::Int32 => scan_int(data, i32::from_le_bytes),
        PixelType::Uint8 => scan_int(data, u8::from_le_bytes),
        PixelType::Uint16 => scan_int(data, u16::from_le_bytes),
        PixelType::Uint32 => scan_int(data, u32::from_le_bytes),
        PixelType::Float => scan(data, f32::from_le_bytes),
        PixelType::Double => scan(data, f64::from_le_bytes),
        _ => ChannelRange::EMPTY,
    }
}

/// Merge two ranges.
pub fn union(a: ChannelRange, b: ChannelRange) -> ChannelRange {
    ChannelRange {
        min: a.min.min(b.min),
        max: a.max.max(b.max),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16s(v: &[u16]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    #[test]
    fn mean_of_blocks_even() {
        let d = u16s(&[1, 3, 10, 20, 5, 7, 30, 40]);
        let o = downsample_2x(&d, 4, 2, PixelType::Uint16);
        // (1+3+5+7)/4 = 4, (10+20+30+40)/4 = 25
        assert_eq!(o, u16s(&[4, 25]));
    }

    #[test]
    fn odd_edges_average_what_is_there() {
        // 3x3 -> 2x2
        let d: Vec<u8> = vec![0, 2, 9, 4, 6, 9, 8, 8, 1];
        let o = downsample_2x(&d, 3, 3, PixelType::Uint8);
        assert_eq!(o, vec![3, 9, 8, 1]);
    }

    #[test]
    fn rounding_and_signed() {
        let d: Vec<u8> = [-1i16, -2, 1, 1]
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect();
        let o = downsample_2x(&d, 2, 2, PixelType::Int16);
        // mean = -0.25 -> 0 (round) ; stored as 0
        assert_eq!(o, 0i16.to_le_bytes().to_vec());
        let d: Vec<u8> = vec![1, 2, 2, 2];
        assert_eq!(downsample_2x(&d, 2, 2, PixelType::Uint8), vec![2]);
    }

    #[test]
    fn float_mean() {
        let d: Vec<u8> = [0.5f32, 1.5, 2.0, 4.0]
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect();
        let o = downsample_2x(&d, 2, 2, PixelType::Float);
        assert_eq!(o, 2.0f32.to_le_bytes().to_vec());
    }

    #[test]
    fn interleave_roundtrip() {
        let rgb: Vec<u8> = (0u8..12).collect();
        let planes = deinterleave(&rgb, 3, 1);
        assert_eq!(planes[0], vec![0, 3, 6, 9]);
        assert_eq!(planes[2], vec![2, 5, 8, 11]);
        assert_eq!(interleave(&planes, 1), rgb);
        let rgb16 = u16s(&[1, 2, 3, 4, 5, 6]);
        let p = deinterleave(&rgb16, 3, 2);
        assert_eq!(p[1], u16s(&[2, 5]));
        assert_eq!(interleave(&p, 2), rgb16);
    }

    #[test]
    fn shuffles_and_ranges_span_tasks() {
        // More pixels than one task holds, so the work is split and merged.
        let n = PIXELS_PER_TASK * 2 + 7;
        let rgb: Vec<u8> = (0..n * 3).map(|i| (i * 7 % 251) as u8).collect();
        let planes = deinterleave(&rgb, 3, 1);
        assert_eq!(planes[1][n - 1], rgb[(n - 1) * 3 + 1]);
        assert_eq!(interleave(&planes, 1), rgb);
        let wide: Vec<u8> = (0..n * 4 * 2).map(|i| (i % 253) as u8).collect();
        let p = deinterleave(&wide, 4, 2);
        assert_eq!(&p[3][..2], &wide[6..8]);
        assert_eq!(interleave(&p, 2), wide);
        let r = range(&planes[0], PixelType::Uint8);
        let lo = planes[0].iter().min().copied().unwrap_or(0);
        let hi = planes[0].iter().max().copied().unwrap_or(0);
        assert_eq!((r.min, r.max), (f64::from(lo), f64::from(hi)));
        // The first of equal values is kept, as a scan in order keeps it.
        let mut z = vec![0.0f32; n];
        z[n - 1] = -0.0;
        let zb: Vec<u8> = z.iter().flat_map(|x| x.to_le_bytes()).collect();
        let r = range(&zb, PixelType::Float);
        assert!(r.min.is_sign_positive() && r.max.is_sign_positive());
        let mut z = vec![-0.0f32; n];
        z[n - 1] = 0.0;
        z[3] = f32::NAN;
        let zb: Vec<u8> = z.iter().flat_map(|x| x.to_le_bytes()).collect();
        let r = range(&zb, PixelType::Float);
        assert!(r.min.is_sign_negative() && r.max.is_sign_negative());
    }

    #[test]
    fn ranges() {
        let r = range(&u16s(&[7, 3, 900]), PixelType::Uint16);
        assert_eq!((r.min, r.max), (3.0, 900.0));
        let e = range(&[], PixelType::Uint8);
        assert!(e.is_empty());
        let u = union(e, r);
        assert_eq!((u.min, u.max), (3.0, 900.0));
    }
}
