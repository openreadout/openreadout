//! Pixel types and the in-memory plane representation.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Sample type of one channel value. Names follow OME-XML `PixelType` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum PixelType {
    /// Signed 8-bit integer.
    Int8,
    /// Signed 16-bit integer.
    Int16,
    /// Signed 32-bit integer.
    Int32,
    /// Unsigned 8-bit integer.
    Uint8,
    /// Unsigned 16-bit integer (the usual microscope camera output).
    Uint16,
    /// Unsigned 32-bit integer.
    Uint32,
    /// 32-bit IEEE 754 floating point.
    Float,
    /// 64-bit IEEE 754 floating point.
    Double,
    /// Signed 64-bit integer.
    Int64,
    /// Unsigned 64-bit integer.
    Uint64,
    /// Complex number of two 32-bit floats (real, then imaginary): OME `complex`.
    #[serde(rename = "complex")]
    ComplexFloat,
    /// Complex number of two 64-bit floats (real, then imaginary): OME `double-complex`.
    #[serde(rename = "double-complex")]
    ComplexDouble,
}

impl PixelType {
    /// Size of one sample in bytes.
    pub fn bytes_per_sample(self) -> usize {
        match self {
            PixelType::Int8 | PixelType::Uint8 => 1,
            PixelType::Int16 | PixelType::Uint16 => 2,
            PixelType::Int32 | PixelType::Uint32 | PixelType::Float => 4,
            PixelType::Double | PixelType::Int64 | PixelType::Uint64 | PixelType::ComplexFloat => 8,
            PixelType::ComplexDouble => 16,
        }
    }

    /// OME-XML spelling.
    pub fn ome_name(self) -> &'static str {
        match self {
            PixelType::Int8 => "int8",
            PixelType::Int16 => "int16",
            PixelType::Int32 => "int32",
            PixelType::Uint8 => "uint8",
            PixelType::Uint16 => "uint16",
            PixelType::Uint32 => "uint32",
            PixelType::Float => "float",
            PixelType::Double => "double",
            PixelType::Int64 => "int64",
            PixelType::Uint64 => "uint64",
            PixelType::ComplexFloat => "complex",
            PixelType::ComplexDouble => "double-complex",
        }
    }

    /// `NumPy` dtype string, handy for Python users.
    pub fn numpy_dtype(self) -> &'static str {
        match self {
            PixelType::Int8 => "int8",
            PixelType::Int16 => "int16",
            PixelType::Int32 => "int32",
            PixelType::Uint8 => "uint8",
            PixelType::Uint16 => "uint16",
            PixelType::Uint32 => "uint32",
            PixelType::Float => "float32",
            PixelType::Double => "float64",
            PixelType::Int64 => "int64",
            PixelType::Uint64 => "uint64",
            PixelType::ComplexFloat => "complex64",
            PixelType::ComplexDouble => "complex128",
        }
    }

    /// Complex samples (a real and an imaginary part per sample).
    pub fn is_complex(self) -> bool {
        matches!(self, PixelType::ComplexFloat | PixelType::ComplexDouble)
    }

    /// Floating-point samples, real or complex.
    pub fn is_float(self) -> bool {
        matches!(
            self,
            PixelType::Float
                | PixelType::Double
                | PixelType::ComplexFloat
                | PixelType::ComplexDouble
        )
    }

    /// The value of one little-endian sample as `f64`; `b` holds at least
    /// [`bytes_per_sample`](Self::bytes_per_sample) bytes (missing bytes read as 0). 64-bit
    /// integers above 2^53 are rounded. A complex sample's value is its modulus `|z|`: every
    /// scalar summary of complex data (statistics, previews, comparisons) is of the amplitude.
    pub fn sample_f64(self, b: &[u8]) -> f64 {
        fn arr<const N: usize>(b: &[u8]) -> [u8; N] {
            let mut a = [0u8; N];
            let n = b.len().min(N);
            a[..n].copy_from_slice(&b[..n]);
            a
        }
        match self {
            PixelType::Uint8 => b.first().map_or(0.0, |v| f64::from(*v)),
            PixelType::Int8 => b.first().map_or(0.0, |v| f64::from(v.cast_signed())),
            PixelType::Uint16 => f64::from(u16::from_le_bytes(arr(b))),
            PixelType::Int16 => f64::from(i16::from_le_bytes(arr(b))),
            PixelType::Uint32 => f64::from(u32::from_le_bytes(arr(b))),
            PixelType::Int32 => f64::from(i32::from_le_bytes(arr(b))),
            PixelType::Float => f64::from(f32::from_le_bytes(arr(b))),
            PixelType::Double => f64::from_le_bytes(arr(b)),
            #[allow(clippy::cast_precision_loss)]
            PixelType::Int64 => i64::from_le_bytes(arr(b)) as f64,
            #[allow(clippy::cast_precision_loss)]
            PixelType::Uint64 => u64::from_le_bytes(arr(b)) as f64,
            PixelType::ComplexFloat => {
                let a: [u8; 8] = arr(b);
                let re = f32::from_le_bytes([a[0], a[1], a[2], a[3]]);
                let im = f32::from_le_bytes([a[4], a[5], a[6], a[7]]);
                f64::from(re).hypot(f64::from(im))
            }
            PixelType::ComplexDouble => {
                let a: [u8; 16] = arr(b);
                let (r, i) = a.split_at(8);
                f64::from_le_bytes(arr(r)).hypot(f64::from_le_bytes(arr(i)))
            }
        }
    }

    /// Every sample of `data` (little-endian, tightly packed) as `f64`, as
    /// [`sample_f64`](Self::sample_f64) reads one.
    pub fn samples_f64(self, data: &[u8]) -> impl Iterator<Item = f64> + '_ {
        data.chunks_exact(self.bytes_per_sample())
            .map(move |c| self.sample_f64(c))
    }
}

/// Largest plane assembled in memory (4 GiB). Planes are the unit of reading and export, so
/// this bounds the memory of every command; a stitched whole-slide scan above it (e.g. a
/// 190 000 × 69 000 RGB mosaic, 39 GB) is refused with a clean error instead of exhausting RAM.
/// Its pyramid levels stay readable.
pub const MAX_PLANE_BYTES: u64 = 4 << 30;

/// Byte size of a `width` × `height` plane of `bytes_per_pixel`, or an "unsupported" error
/// (exit 6) when it exceeds [`MAX_PLANE_BYTES`] or does not fit in memory.
pub fn plane_bytes_checked(
    format: &'static str,
    width: u32,
    height: u32,
    bytes_per_pixel: usize,
) -> crate::Result<usize> {
    let n = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|v| v.checked_mul(bytes_per_pixel as u64));
    match n.and_then(|v| usize::try_from(v).ok()) {
        Some(v) if v as u64 <= MAX_PLANE_BYTES => Ok(v),
        _ => Err(plane_too_large(
            format,
            width,
            height,
            n.unwrap_or(u64::MAX),
        )),
    }
}

/// The error for a plane above [`MAX_PLANE_BYTES`].
pub(crate) fn plane_too_large(
    format: &'static str,
    width: u32,
    height: u32,
    bytes: u64,
) -> crate::Error {
    crate::Error::unsupported(
        format,
        format!(
            "a {width} x {height} plane of {:.1} GiB (planes are assembled in memory; the limit is {} GiB)",
            bytes as f64 / f64::from(1u32 << 30),
            MAX_PLANE_BYTES >> 30
        ),
        "Read a downsampled pyramid level instead (`info` lists `pyramid_levels`; `check --planes --level N`, `preview`), or pick another image with `--image`.",
    )
}

/// One 2-D plane of samples, row-major, no row padding, native (little-endian) byte order.
/// `samples_per_pixel` is 1 for grayscale and 3 for interleaved RGB (4 for RGBA). A complex
/// sample is one sample of [`PixelType::ComplexFloat`] or [`PixelType::ComplexDouble`] (real
/// part, then imaginary part).
#[derive(Debug, Clone)]
pub struct Plane {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Sample type.
    pub pixel_type: PixelType,
    /// Samples per pixel: 1 for grayscale, 3 for interleaved RGB.
    pub samples_per_pixel: u32,
    /// The samples, `height` rows of `row_bytes()` bytes each.
    pub data: Vec<u8>,
}

impl Plane {
    /// Expected byte length for the declared geometry.
    pub fn expected_len(&self) -> usize {
        self.width as usize
            * self.height as usize
            * self.samples_per_pixel as usize
            * self.pixel_type.bytes_per_sample()
    }

    /// 128-bit xxh3 of the raw sample bytes, as 32 lowercase hex chars.
    /// This is the value the oracle harness compares against.
    pub fn xxh3_hex(&self) -> String {
        format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&self.data))
    }

    /// Size of one row in bytes.
    pub fn row_bytes(&self) -> usize {
        self.width as usize * self.samples_per_pixel as usize * self.pixel_type.bytes_per_sample()
    }
}

/// IEEE 754 binary16 → binary32, exactly (every half value is representable). Subnormals are
/// normalized; NaNs keep their payload and become quiet, as hardware (F16C) and NumPy widen them.
/// Shared by the readers of formats that store half floats (Leica LIF FLIM maps, MRC mode 12).
pub fn half_to_f32(h: u16) -> f32 {
    let sign = u32::from(h >> 15) << 31;
    let exp = u32::from((h >> 10) & 0x1f);
    let man = u32::from(h & 0x3ff);
    let bits = match (exp, man) {
        (0, 0) => sign,
        (0, m) => {
            // subnormal: value = m * 2^-24; shift the leading 1 up to bit 10 (k shifts), then the
            // value is 1.frac * 2^(-14 - k), i.e. a binary32 exponent field of 127 - 14 - k.
            let mut k = 0u32;
            let mut m = m;
            while m & 0x400 == 0 {
                m <<= 1;
                k += 1;
            }
            sign | ((113 - k) << 23) | ((m & 0x3ff) << 13)
        }
        (0x1f, 0) => sign | 0x7f80_0000,
        (0x1f, m) => sign | 0x7fc0_0000 | (m << 13),
        (e, m) => sign | ((e + 127 - 15) << 23) | (m << 13),
    };
    f32::from_bits(bits)
}

/// Little-endian half floats → little-endian `f32` bytes (a trailing odd byte is dropped).
pub fn widen_half(data: &[u8]) -> Vec<u8> {
    data.as_chunks::<2>()
        .0
        .iter()
        .flat_map(|c| half_to_f32(u16::from_le_bytes(*c)).to_le_bytes())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every one of the 65 536 half values against a reference computed in `f64` arithmetic.
    #[test]
    fn half_floats_widen_exactly_for_every_value() {
        for h in 0..=u16::MAX {
            let sign = if h >> 15 == 1 { -1.0f64 } else { 1.0 };
            let e = i32::from((h >> 10) & 0x1f);
            let m = f64::from(h & 0x3ff);
            let got = half_to_f32(h);
            let want = match e {
                0x1f if h & 0x3ff == 0 => sign * f64::INFINITY,
                0x1f => {
                    assert!(got.is_nan(), "{h:#06x}");
                    let quiet =
                        (u32::from(h >> 15) << 31) | 0x7fc0_0000 | (u32::from(h & 0x3ff) << 13);
                    assert_eq!(got.to_bits(), quiet, "{h:#06x}");
                    continue;
                }
                0 => sign * m * 2f64.powi(-24),
                _ => sign * (1.0 + m / 1024.0) * 2f64.powi(e - 15),
            };
            // bit for bit, so -0.0 and the sign of subnormals are checked too
            assert_eq!(f64::from(got).to_bits(), want.to_bits(), "{h:#06x}");
        }
        assert_eq!(
            widen_half(&[0x00, 0x3c, 0x00, 0xc0, 0x7f]),
            [1.0f32.to_le_bytes(), (-2.0f32).to_le_bytes()].concat()
        );
    }

    #[test]
    fn plane_size_limit() {
        assert_eq!(plane_bytes_checked("t", 1024, 1024, 2).unwrap(), 2 << 20);
        assert_eq!(
            plane_bytes_checked("t", 65_536, 32_768, 2).unwrap() as u64,
            MAX_PLANE_BYTES
        );
        // The 3.7 GB Young-mouse CZI: one stitched 190309 x 69378 RGB plane (39.6 GB).
        let e = plane_bytes_checked("czi", 190_309, 69_378, 3).unwrap_err();
        assert_eq!(e.exit_code(), 6);
        assert!(e.to_string().contains("190309 x 69378"), "{e}");
        assert!(plane_bytes_checked("t", u32::MAX, u32::MAX, 8).is_err());
    }
}
