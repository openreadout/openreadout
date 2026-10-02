//! Sample-level helpers on little-endian plane bytes: 2×2 mean downsampling, RGB sample
//! (de)interleaving and value ranges.

use openreadout_core::PixelType;

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

/// Split interleaved samples (`spp` per pixel) into `spp` single-sample planes.
pub fn deinterleave(data: &[u8], spp: usize, bytes_per_sample: usize) -> Vec<Vec<u8>> {
    if spp <= 1 {
        return vec![data.to_vec()];
    }
    let px = spp * bytes_per_sample;
    let n = data.len() / px;
    let mut out = vec![Vec::with_capacity(n * bytes_per_sample); spp];
    for pixel in data.chunks_exact(px) {
        for (s, o) in out.iter_mut().enumerate() {
            o.extend_from_slice(&pixel[s * bytes_per_sample..(s + 1) * bytes_per_sample]);
        }
    }
    out
}

/// Inverse of [`deinterleave`]. Planes must have equal length.
pub fn interleave(planes: &[Vec<u8>], bytes_per_sample: usize) -> Vec<u8> {
    if planes.len() == 1 {
        return planes[0].clone();
    }
    let n = planes
        .first()
        .map_or(0, |p| p.len() / bytes_per_sample.max(1));
    let mut out = Vec::with_capacity(n * bytes_per_sample * planes.len());
    for i in 0..n {
        for p in planes {
            out.extend_from_slice(&p[i * bytes_per_sample..(i + 1) * bytes_per_sample]);
        }
    }
    out
}

/// Min and max sample value of a single-sample plane (NaNs ignored).
pub fn range(data: &[u8], pt: PixelType) -> ChannelRange {
    macro_rules! go {
        ($t:ty, $n:expr) => {{
            let mut r = ChannelRange::EMPTY;
            for c in data.as_chunks::<$n>().0 {
                let v = <$t>::from_le_bytes(*c).to_f64();
                if v < r.min {
                    r.min = v;
                }
                if v > r.max {
                    r.max = v;
                }
            }
            r
        }};
    }
    for_type!(pt, go, ChannelRange::EMPTY)
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
    fn ranges() {
        let r = range(&u16s(&[7, 3, 900]), PixelType::Uint16);
        assert_eq!((r.min, r.max), (3.0, 900.0));
        let e = range(&[], PixelType::Uint8);
        assert!(e.is_empty());
        let u = union(e, r);
        assert_eq!((u.min, u.max), (3.0, 900.0));
    }
}
