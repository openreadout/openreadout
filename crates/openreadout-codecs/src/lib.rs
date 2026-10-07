//! Pure-Rust decoders used by the format readers. No C toolchain is needed, which keeps
//! the musl static builds simple. Every function returns owned bytes in the caller's
//! expected layout; nothing here knows about file formats.
//!
//! Every decoder is bounded: output never grows past the caller's expected size (plus one
//! byte to detect overlong streams), and size hints are only trusted up to
//! [`MAX_PREALLOC`] for up-front allocation.
//!
//! Decoding failures are reported as [`CodecError`] values, never by exiting the process.
//!
//! # Cargo features
//!
//! All enabled by default. A disabled codec's functions still exist and return
//! [`CodecError::NotCompiled`].
//!
//! | feature | codecs |
//! | --- | --- |
//! | `zstd` | [`zstd_decode`], [`zstd1_decode`] (Zstandard, via `ruzstd`) |
//! | `jpeg` | [`jpeg_decode`], [`jpeg_decode_tiff`] and their `_limited` forms (baseline, progressive, lossless) |
//! | `jpegxr` | [`jpegxr_decode`] (JPEG XR, as in CZI files; `openreadout-jpegxr`, a safe port of jxrlib) |
//! | `jpeg2000` | [`jpeg2000_decode`] and [`jpeg2000_decode_limited`] (`rust-j2k`, fallback `dicom-toolkit-jpeg2000`) |
//! | `lzw` | [`lzw_decode`] (TIFF LZW) |
//! | `deflate` | [`zlib_decode`] |
//! | `lz4` | [`lz4_block_decode`], [`lz4_sized_decode`], [`hdf5_lz4_decode`] and LZ4 inside Blosc |
//!
//! [`chunked_decode`] (CZI chunked compression) uses the `zstd` and `lz4` features.
//!
//! [`unshuffle_hilo`], [`packbits_decode`], [`lzf_decode`], [`blosc_decode`], [`blosclz_decode`] and
//! [`lzma2_decode`] (raw LZMA2 chunks) need no feature.
//!
//! # Example
//!
//! ```
//! use openreadout_codecs::{packbits_decode, unshuffle_hilo};
//!
//! // PackBits: a literal run of 2 bytes, then the byte 0xAA repeated 3 times.
//! let raw = packbits_decode(&[0x01, 0x10, 0x20, 0xFE, 0xAA], 5)?;
//! assert_eq!(raw, [0x10, 0x20, 0xAA, 0xAA, 0xAA]);
//!
//! // "HiLo": all low bytes of 16-bit samples, then all high bytes.
//! assert_eq!(unshuffle_hilo(&[0x01, 0x02, 0xA0, 0xB0]), [0x01, 0xA0, 0x02, 0xB0]);
//! # Ok::<(), openreadout_codecs::CodecError>(())
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod blosc;
mod chunked;
#[cfg(feature = "jpeg2000")]
mod j2k;
#[cfg(feature = "jpeg")]
mod jpeg12;
#[cfg(feature = "jpegxl")]
mod jxl;
#[cfg(feature = "lerc")]
mod lerc;
mod lzma;
#[cfg(feature = "webp")]
mod webp;

pub use blosc::{
    BloscCompressor, BloscHeader, blosc_decode, blosc_header, blosclz_decode, hdf5_lz4_decode,
    lz4_block_decode, lz4_sized_decode, snappy_decode,
};
pub use chunked::chunked_decode;
pub use lzma::lzma2_decode;

/// Codec failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CodecError {
    /// The data is not a valid stream for this codec.
    #[error("{codec}: {detail}")]
    Decode {
        /// Codec name.
        codec: &'static str,
        /// What is wrong with the stream.
        detail: String,
    },
    /// The codec was left out at build time.
    #[error("{codec} support was not compiled in (feature `{feature}`)")]
    NotCompiled {
        /// Codec name.
        codec: &'static str,
        /// Cargo feature that enables it.
        feature: &'static str,
    },
    /// A valid stream that uses a feature this decoder does not implement.
    #[error("{codec}: {detail} is not supported")]
    Unsupported {
        /// Codec name.
        codec: &'static str,
        /// The feature.
        detail: String,
    },
    /// The stream decoded, but not to the expected number of bytes.
    #[error("decoded {codec} data is {got} bytes, expected {expected}")]
    SizeMismatch {
        /// Codec name.
        codec: &'static str,
        /// Bytes decoded.
        got: usize,
        /// Bytes the caller expected.
        expected: usize,
    },
}

/// Result alias of this crate.
pub type Result<T> = std::result::Result<T, CodecError>;

/// Largest buffer a decoder pre-allocates from a caller-supplied size hint (64 MiB). Output
/// beyond this grows on demand, so a lying size field cannot force a huge up-front allocation.
pub const MAX_PREALLOC: usize = 64 << 20;

/// Output cap for streams whose decoded size is unknown (`expected_len == 0`): 1 GiB.
pub const MAX_UNSIZED_OUTPUT: usize = 1 << 30;

/// How many bytes a decoder may produce: one more than expected (so "too long" is
/// detectable without decoding a bomb to the end), or the unsized cap.
fn output_limit(expected_len: usize) -> usize {
    if expected_len == 0 {
        MAX_UNSIZED_OUTPUT
    } else {
        expected_len.saturating_add(1)
    }
}

/// Decompress a plain zstd frame. `expected_len` is a sanity bound (0 = unknown); decoding
/// stops one byte past it, so a decompression bomb costs at most `expected_len + 1` bytes.
pub fn zstd_decode(data: &[u8], expected_len: usize) -> Result<Vec<u8>> {
    #[cfg(feature = "zstd")]
    {
        use std::io::Read;
        // Fast path when the size is known: decode straight into a buffer of that size (no
        // `Read` adapter). Any failure, including trailing bytes after the frame or a size
        // other than expected, falls through to the streaming decoder below, which produces
        // the same results and errors.
        if expected_len != 0 && expected_len <= MAX_PREALLOC {
            let mut out = vec![0u8; expected_len];
            let mut fd = ruzstd::decoding::FrameDecoder::new();
            if let Ok(n) = fd.decode_all(data, &mut out)
                && n == expected_len
            {
                return Ok(out);
            }
        }
        let dec =
            ruzstd::decoding::StreamingDecoder::new(data).map_err(|e| CodecError::Decode {
                codec: "zstd",
                detail: e.to_string(),
            })?;
        let limit = output_limit(expected_len);
        let mut out = Vec::with_capacity(expected_len.min(MAX_PREALLOC));
        dec.take(limit as u64)
            .read_to_end(&mut out)
            .map_err(|e| CodecError::Decode {
                codec: "zstd",
                detail: e.to_string(),
            })?;
        if (expected_len != 0 && out.len() != expected_len) || out.len() >= MAX_UNSIZED_OUTPUT {
            return Err(CodecError::SizeMismatch {
                codec: "zstd",
                got: out.len(),
                expected: expected_len,
            });
        }
        Ok(out)
    }
    #[cfg(not(feature = "zstd"))]
    {
        let _ = (data, expected_len);
        Err(CodecError::NotCompiled {
            codec: "zstd",
            feature: "zstd",
        })
    }
}

/// Parsed "zstd1" prefix used by CZI compression id 6: `header_size`, `chunk_kind`, flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Zstd1Header {
    /// Size of the header in bytes (byte 0).
    pub header_size: u8,
    /// Kind of the first header chunk (byte 1).
    pub chunk_kind: u8,
    /// Bit 0 of the flags byte: samples were byte-shuffled into low-byte and high-byte halves.
    pub hilo: bool,
}

/// Parse the zstd1 header; returns the header and the offset of the zstd frame.
pub fn zstd1_header(data: &[u8]) -> Result<(Zstd1Header, usize)> {
    if data.len() < 2 {
        return Err(CodecError::Decode {
            codec: "zstd1",
            detail: "buffer shorter than the header".into(),
        });
    }
    let header_size = data[0] as usize;
    if header_size < 1 || header_size > data.len() {
        return Err(CodecError::Decode {
            codec: "zstd1",
            detail: format!("implausible header_size {header_size}"),
        });
    }
    let chunk_kind = data.get(1).copied().unwrap_or(0);
    let flags = if header_size >= 3 { data[2] } else { 0 };
    Ok((
        Zstd1Header {
            header_size: header_size as u8,
            chunk_kind,
            hilo: flags & 1 != 0,
        },
        header_size,
    ))
}

/// Decode a zstd1 payload into interleaved samples of `bytes_per_sample` bytes.
pub fn zstd1_decode(data: &[u8], expected_len: usize, bytes_per_sample: usize) -> Result<Vec<u8>> {
    let (h, off) = zstd1_header(data)?;
    let raw = zstd_decode(data.get(off..).unwrap_or_default(), expected_len)?;
    if h.hilo && bytes_per_sample == 2 {
        Ok(unshuffle_hilo(&raw))
    } else {
        Ok(raw)
    }
}

/// Undo the `HiLo` byte shuffle: input is all low bytes followed by all high bytes of
/// 16-bit little-endian samples; output is the interleaved samples.
pub fn unshuffle_hilo(shuffled: &[u8]) -> Vec<u8> {
    let n = shuffled.len() / 2;
    let (lo, rest) = shuffled.split_at(n);
    let hi = &rest[..n];
    let mut out = vec![0u8; shuffled.len()];
    // Zipped fixed-size chunks: no per-byte capacity or bounds checks in the loop.
    for (pair, (&l, &h)) in out.as_chunks_mut::<2>().0.iter_mut().zip(lo.iter().zip(hi)) {
        *pair = [l, h];
    }
    if shuffled.len() % 2 == 1 {
        out[shuffled.len() - 1] = shuffled[shuffled.len() - 1];
    }
    out
}

/// A decoded raster: interleaved samples, no row padding.
#[derive(Debug, Clone)]
pub struct Raster {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Samples per pixel (1 for grayscale, 3 for colour).
    pub channels: u32,
    /// Bits per sample.
    pub bits_per_sample: u32,
    /// True for floating-point samples.
    pub float: bool,
    /// True when the codestream declares B,G,R sample order (the caller may want R,G,B).
    pub bgr: bool,
    /// The samples, interleaved, `width × channels × bits_per_sample / 8` bytes per row.
    pub data: Vec<u8>,
}

/// Decode a JPEG XR codestream (as embedded in CZI subblocks).
pub fn jpegxr_decode(data: &[u8]) -> Result<Raster> {
    jpegxr_decode_expect(data, None)
}

/// Decode a JPEG XR codestream, refusing before any pixel allocation when the codestream's
/// own dimensions differ from `expect` (width, height), e.g. the geometry a CZI directory
/// entry declares. Without an expectation the decoded size is capped at
/// [`MAX_UNSIZED_OUTPUT`].
///
/// Backed by `openreadout-jpegxr`, our `forbid(unsafe_code)` port of the decoder path of
/// Microsoft's jxrlib (bit-exact with it). Features that decoder does not implement (alpha
/// planes, CMYK, packed formats, ...) are [`CodecError::Unsupported`].
pub fn jpegxr_decode_expect(data: &[u8], expect: Option<(u32, u32)>) -> Result<Raster> {
    #[cfg(feature = "jpegxr")]
    {
        use openreadout_jpegxr as jxr;
        let map = |e: jxr::Error| match e {
            jxr::Error::Unsupported(m) => CodecError::Unsupported {
                codec: "jpeg-xr",
                detail: m,
            },
            other => CodecError::Decode {
                codec: "jpeg-xr",
                detail: other.to_string(),
            },
        };
        let info = jxr::probe(data).map_err(map)?;
        if let Some((ew, eh)) = expect
            && (ew, eh) != (info.width, info.height)
        {
            return Err(CodecError::Decode {
                codec: "jpeg-xr",
                detail: format!(
                    "codestream is {}x{} but {ew}x{eh} was declared",
                    info.width, info.height
                ),
            });
        }
        let limit = match expect {
            // The declared geometry at the widest sample layout (4 x 32-bit per pixel).
            Some((w, h)) => {
                (u64::from(w) * u64::from(h) * 16).min(MAX_UNSIZED_OUTPUT as u64) as usize
            }
            None => MAX_UNSIZED_OUTPUT,
        };
        let img = jxr::decode(data, limit).map_err(map)?;
        let (bits, float) = match img.sample {
            jxr::SampleType::U8 => (8, false),
            jxr::SampleType::U16 => (16, false),
            jxr::SampleType::F32 => (32, true),
            other => {
                return Err(CodecError::Unsupported {
                    codec: "jpeg-xr",
                    detail: format!("{other:?} samples (signed fixed point or half float)"),
                });
            }
        };
        Ok(Raster {
            width: img.width,
            height: img.height,
            channels: img.channels,
            bits_per_sample: bits,
            float,
            bgr: img.bgr,
            data: img.data,
        })
    }
    #[cfg(not(feature = "jpegxr"))]
    {
        let _ = (data, expect);
        Err(CodecError::NotCompiled {
            codec: "jpeg-xr",
            feature: "jpegxr",
        })
    }
}

/// `io::Write` sink that refuses to grow past `limit` bytes (decompression-bomb guard).
#[cfg(feature = "lzw")]
struct LimitedWriter {
    buf: Vec<u8>,
    limit: usize,
}

#[cfg(feature = "lzw")]
impl std::io::Write for LimitedWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let room = self.limit - self.buf.len();
        if room == 0 && !data.is_empty() {
            return Err(std::io::Error::other(
                "decoded data exceeds the declared size",
            ));
        }
        let n = data.len().min(room);
        self.buf.extend_from_slice(&data[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Decode a JPEG stream (ITU-T T.81): baseline and progressive 8-bit, sequential 12-bit
/// (extended process, Huffman coding, no chroma subsampling), lossless (SOF3) up to 16 bit.
/// Returns gray (`channels == 1`) or R,G,B (`channels == 3`) samples; samples above 8 bits
/// come back as little-endian u16. Progressive or subsampled 12-bit streams and CMYK are
/// rejected with an error.
pub fn jpeg_decode(data: &[u8]) -> Result<Raster> {
    jpeg_decode_with(data, None, MAX_UNSIZED_OUTPUT)
}

/// [`jpeg_decode`], refusing (before decoding anything) a frame whose decoded size would
/// exceed `max_bytes`. Callers that know the tile or plane size pass a bound derived from it,
/// so a few-byte stream declaring 60000 x 15000 pixels costs nothing.
pub fn jpeg_decode_limited(data: &[u8], max_bytes: usize) -> Result<Raster> {
    jpeg_decode_with(data, None, max_bytes.min(MAX_UNSIZED_OUTPUT))
}

/// `color`: `None` keeps the decoder's default (Adobe/JFIF markers, else YCbCr for three
/// components); `Some` forces the transform. `max_bytes`: largest decoded frame accepted.
fn jpeg_decode_with(data: &[u8], color: Option<JpegColor>, max_bytes: usize) -> Result<Raster> {
    #[cfg(feature = "jpeg")]
    {
        // jpeg-decoder stops at 8-bit DCT samples; 12-bit sequential streams have their own
        // decoder (`jpeg12`).
        if jpeg12::frame_precision(data) == Some(12) {
            return jpeg12::decode(data, color, max_bytes);
        }
        let mut dec = jpeg_decoder::Decoder::new(data);
        // A frame header can declare 65535 x 65535 x 4 samples; cap what the decoder allocates.
        dec.set_max_decoding_buffer_size(MAX_UNSIZED_OUTPUT);
        dec.read_info().map_err(|e| CodecError::Decode {
            codec: "jpeg",
            detail: e.to_string(),
        })?;
        // That cap applies to the output only: the decoder sizes its per-component planes
        // from the frame header before checking it (a 342-byte stream made it allocate 4 GB).
        if let Some(i) = dec.info() {
            let bytes =
                u64::from(i.width) * u64::from(i.height) * i.pixel_format.pixel_bytes() as u64;
            if bytes > max_bytes as u64 {
                return Err(CodecError::Decode {
                    codec: "jpeg",
                    detail: format!(
                        "frame header declares {} x {} pixels ({bytes} bytes decoded)",
                        i.width, i.height
                    ),
                });
            }
        }
        if let Some(color) = color {
            // Only three-component streams have a colour transform to choose.
            if dec
                .info()
                .is_some_and(|i| i.pixel_format == jpeg_decoder::PixelFormat::RGB24)
            {
                dec.set_color_transform(match color {
                    // "RGB" in jpeg-decoder: the components already are R, G, B (no conversion).
                    JpegColor::AsCoded => jpeg_decoder::ColorTransform::RGB,
                    JpegColor::ToRgb => jpeg_decoder::ColorTransform::YCbCr,
                });
            }
        }
        let px = dec.decode().map_err(|e| CodecError::Decode {
            codec: "jpeg",
            detail: e.to_string(),
        })?;
        let info = dec.info().ok_or_else(|| CodecError::Decode {
            codec: "jpeg",
            detail: "stream has no frame header".into(),
        })?;
        let (channels, bits_per_sample, data) = match info.pixel_format {
            jpeg_decoder::PixelFormat::L8 => (1, 8, px),
            jpeg_decoder::PixelFormat::RGB24 => (3, 8, px),
            // jpeg-decoder hands back native-endian u16; normalize to little-endian.
            jpeg_decoder::PixelFormat::L16 => (
                1,
                16,
                px.as_chunks::<2>()
                    .0
                    .iter()
                    .flat_map(|c| u16::from_ne_bytes(*c).to_le_bytes())
                    .collect(),
            ),
            jpeg_decoder::PixelFormat::CMYK32 => {
                return Err(CodecError::Decode {
                    codec: "jpeg",
                    detail: "CMYK JPEG streams are not supported".into(),
                });
            }
        };
        let (width, height) = (u32::from(info.width), u32::from(info.height));
        let need =
            width as usize * height as usize * channels as usize * (bits_per_sample as usize / 8);
        if data.len() < need {
            return Err(CodecError::SizeMismatch {
                codec: "jpeg",
                got: data.len(),
                expected: need,
            });
        }
        Ok(Raster {
            width,
            height,
            channels,
            bits_per_sample,
            float: false,
            bgr: false,
            data,
        })
    }
    #[cfg(not(feature = "jpeg"))]
    {
        let _ = (data, color);
        Err(CodecError::NotCompiled {
            codec: "jpeg",
            feature: "jpeg",
        })
    }
}

/// Decode a JPEG 2000 codestream (raw J2K `FF 4F FF 51…` or a JP2 file) at its native bit depth.
/// Samples are interleaved; precisions above 8 bits come back as little-endian u16. Used for the
/// frames of legacy (NIS-Elements 2.x) ND2 files, VSI/ETS tiles and Aperio SVS tiles. Backed by
/// `rust-j2k` (pure Rust, no `unsafe`, OpenJPEG-style mid-point reconstruction), with
/// `dicom-toolkit-jpeg2000` for codestreams it does not support (see `j2k.rs`). The caller
/// converts colour: components come back as coded (after the codestream's own RCT/ICT).
pub fn jpeg2000_decode(data: &[u8]) -> Result<Raster> {
    jpeg2000_decode_limited(data, MAX_UNSIZED_OUTPUT)
}

/// [`jpeg2000_decode`], refusing (before decoding) an image that would decode to more than
/// `max_bytes` (counted at 2 bytes per sample).
pub fn jpeg2000_decode_limited(data: &[u8], max_bytes: usize) -> Result<Raster> {
    #[cfg(feature = "jpeg2000")]
    {
        j2k::decode(data, max_bytes)
    }
    #[cfg(not(feature = "jpeg2000"))]
    {
        let _ = (data, max_bytes);
        Err(CodecError::NotCompiled {
            codec: "jpeg2000",
            feature: "jpeg2000",
        })
    }
}

/// Decode a WebP file (lossy VP8 or lossless VP8L; TIFF compression 50001) to 8-bit R,G,B, or
/// R,G,B,A when the file has alpha, refusing (before decoding) an image larger than
/// `max_bytes`. Backed by `image-webp` (pure Rust, no `unsafe`).
pub fn webp_decode_limited(data: &[u8], max_bytes: usize) -> Result<Raster> {
    #[cfg(feature = "webp")]
    {
        webp::decode(data, max_bytes)
    }
    #[cfg(not(feature = "webp"))]
    {
        let _ = (data, max_bytes);
        Err(CodecError::NotCompiled {
            codec: "webp",
            feature: "webp",
        })
    }
}

/// Decode a JPEG XL codestream or container (TIFF compression 50002/52546): the first frame,
/// colour channels then extra channels (alpha, ...), interleaved; u8 up to 8 bits per sample,
/// little-endian u16 up to 16, f32 for float images. Refuses (before rendering) an image larger
/// than `max_bytes`. Backed by `jxl-oxide`.
pub fn jpegxl_decode_limited(data: &[u8], max_bytes: usize) -> Result<Raster> {
    #[cfg(feature = "jpegxl")]
    {
        jxl::decode(data, max_bytes)
    }
    #[cfg(not(feature = "jpegxl"))]
    {
        let _ = (data, max_bytes);
        Err(CodecError::NotCompiled {
            codec: "jpeg-xl",
            feature: "jpegxl",
        })
    }
}

/// Decode a LERC2 blob (TIFF compression 34887, after any deflate/zstd wrapping is removed) to
/// interleaved little-endian samples of the blob's type (`channels` = depth x bands); pixels
/// its mask marks invalid are 0. Refuses (before decoding) an image larger than `max_bytes`.
/// Backed by `lerc-rs`.
pub fn lerc_decode_limited(data: &[u8], max_bytes: usize) -> Result<Raster> {
    #[cfg(feature = "lerc")]
    {
        lerc::decode(data, max_bytes)
    }
    #[cfg(not(feature = "lerc"))]
    {
        let _ = (data, max_bytes);
        Err(CodecError::NotCompiled {
            codec: "lerc",
            feature: "lerc",
        })
    }
}

/// TIFF-flavoured LZW (MSB-first, 8-bit symbols), as used by CZI compression id 2.
pub fn lzw_decode(data: &[u8], expected_len: usize) -> Result<Vec<u8>> {
    #[cfg(feature = "lzw")]
    {
        let mut dec = weezl::decode::Decoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
        let mut out = LimitedWriter {
            buf: Vec::with_capacity(expected_len.min(MAX_PREALLOC)),
            limit: output_limit(expected_len),
        };
        // Reuse a small per-thread staging buffer across TIFF strips. The bounded writer
        // still stops decompression bombs, without zeroing weezl's default 16 MiB per strip.
        thread_local! {
            static BUFFER: std::cell::RefCell<Vec<u8>> =
                std::cell::RefCell::new(vec![0; 64 << 10]);
        }
        BUFFER
            .with_borrow_mut(|buffer| {
                let mut stream = dec.into_stream(&mut out);
                stream.set_buffer(buffer);
                stream.decode_all(data).status
            })
            .or_else(|e| {
                // A strip that ends without the end-of-information code (libtiff: "Strip not
                // terminated with EOI code") keeps what it decoded, as libtiff and tifffile do;
                // a short result is caught by the caller's length check.
                if e.kind() == std::io::ErrorKind::UnexpectedEof && !out.buf.is_empty() {
                    Ok(())
                } else {
                    Err(e)
                }
            })
            .map_err(|e| CodecError::Decode {
                codec: "lzw",
                detail: e.to_string(),
            })?;
        let out = out.buf;
        if (expected_len != 0 && out.len() != expected_len) || out.len() >= MAX_UNSIZED_OUTPUT {
            return Err(CodecError::SizeMismatch {
                codec: "lzw",
                got: out.len(),
                expected: expected_len,
            });
        }
        Ok(out)
    }
    #[cfg(not(feature = "lzw"))]
    {
        let _ = (data, expected_len);
        Err(CodecError::NotCompiled {
            codec: "lzw",
            feature: "lzw",
        })
    }
}

/// zlib/deflate (used by some ND2 chunks and by OME-TIFF). With a nonzero `expected_len`
/// at most that many bytes are inflated (callers only use that prefix).
pub fn zlib_decode(data: &[u8], expected_len: usize) -> Result<Vec<u8>> {
    #[cfg(feature = "deflate")]
    {
        use std::io::Read;
        let limit = if expected_len == 0 {
            MAX_UNSIZED_OUTPUT
        } else {
            expected_len
        };
        let mut out = Vec::with_capacity(expected_len.min(MAX_PREALLOC));
        flate2::read::ZlibDecoder::new(data)
            .take(limit as u64)
            .read_to_end(&mut out)
            .map_err(|e| CodecError::Decode {
                codec: "zlib",
                detail: e.to_string(),
            })?;
        if expected_len == 0 && out.len() >= MAX_UNSIZED_OUTPUT {
            return Err(CodecError::SizeMismatch {
                codec: "zlib",
                got: out.len(),
                expected: expected_len,
            });
        }
        Ok(out)
    }
    #[cfg(not(feature = "deflate"))]
    {
        let _ = (data, expected_len);
        Err(CodecError::NotCompiled {
            codec: "zlib",
            feature: "deflate",
        })
    }
}

/// How [`jpeg_decode_tiff`] treats the colour components of the codestream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum JpegColor {
    /// Return the components exactly as coded (no colour conversion). Used when the
    /// container says the components already are R, G, B (e.g. TIFF photometric RGB).
    AsCoded,
    /// Convert three YCbCr-coded components to R, G, B (e.g. TIFF photometric YCbCr).
    ToRgb,
}

/// What a JPEG stream's markers say about its colour coding, read up to the first scan
/// (`SOS`) without decoding anything. See [`jpeg_markers`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct JpegMarkers {
    /// An `APP0` segment starting `JFIF\0` is present (JFIF implies Y, Cb, Cr components).
    pub jfif: bool,
    /// The transform byte of an `APP14` `Adobe` segment: 0 = components as coded (RGB or
    /// CMYK), 1 = YCbCr, 2 = YCCK.
    pub adobe_transform: Option<u8>,
    /// Component identifiers of the frame header (`SOF`), in coding order.
    pub component_ids: Vec<u8>,
    /// Horizontal and vertical sampling factors per component (1 = full resolution).
    pub sampling: Vec<(u8, u8)>,
    /// The coding process of the frame: the low nibble of its `SOF` marker (0 baseline,
    /// 1 extended sequential, 2 progressive, 3 lossless, 9 and up arithmetic coding).
    pub process: u8,
    /// Sample precision of the frame in bits (8 or 12 for DCT, 2 to 16 for lossless).
    pub precision: u8,
}

impl JpegMarkers {
    /// The colour coding of a three-component stream by the rules libjpeg applies when the
    /// container says nothing (JFIF, then Adobe, then component ids `R`,`G`,`B`, else
    /// YCbCr). `None` for streams that are not three-component.
    #[must_use]
    pub fn coded_color(&self) -> Option<JpegColor> {
        if self.component_ids.len() != 3 {
            return None;
        }
        Some(if self.jfif {
            JpegColor::ToRgb
        } else if let Some(t) = self.adobe_transform {
            if t == 0 {
                JpegColor::AsCoded
            } else {
                JpegColor::ToRgb
            }
        } else if self.component_ids == *b"RGB" {
            JpegColor::AsCoded
        } else {
            JpegColor::ToRgb
        })
    }
}

/// Scan the marker segments of a JPEG stream (`tables` first when given, as for a TIFF
/// `JPEGTables` tag) up to the first `SOS`. Only segment lengths are followed; nothing is
/// decoded. Fails when the stream does not start with `SOI` or a segment runs past the end
/// before a frame header was seen.
pub fn jpeg_markers(data: &[u8], tables: Option<&[u8]>) -> Result<JpegMarkers> {
    let mut m = JpegMarkers::default();
    let mut frame_seen = false;
    for s in tables.into_iter().chain(std::iter::once(data)) {
        if s.len() < 2 || s[..2] != [0xFF, 0xD8] {
            return Err(CodecError::Decode {
                codec: "jpeg",
                detail: "stream lacks an SOI marker".into(),
            });
        }
        let mut i = 2usize;
        while let Some(&b) = s.get(i) {
            if b != 0xFF {
                break;
            }
            let Some(&mk) = s.get(i + 1) else { break };
            if mk == 0xFF {
                i += 1; // fill byte
                continue;
            }
            if mk == 0xD8 || mk == 0x01 || (0xD0..=0xD7).contains(&mk) {
                i += 2;
                continue;
            }
            if mk == 0xD9 || mk == 0xDA {
                break;
            }
            let Some(len) = s
                .get(i + 2..i + 4)
                .map(|l| usize::from(u16::from_be_bytes([l[0], l[1]])))
            else {
                break;
            };
            let Some(seg) = len.checked_sub(2).and_then(|n| s.get(i + 4..i + 4 + n)) else {
                break;
            };
            match mk {
                0xE0 if seg.starts_with(b"JFIF\0") => m.jfif = true,
                0xEE if seg.starts_with(b"Adobe") && seg.len() >= 12 => {
                    m.adobe_transform = Some(seg[11]);
                }
                0xC0..=0xCF if !matches!(mk, 0xC4 | 0xC8 | 0xCC) => {
                    m.process = mk & 0x0F;
                    m.precision = seg.first().copied().unwrap_or(0);
                    if let Some(&n) = seg.get(5) {
                        let comps = seg.get(6..6 + 3 * usize::from(n)).unwrap_or_default();
                        m.component_ids = comps.as_chunks::<3>().0.iter().map(|c| c[0]).collect();
                        m.sampling = comps
                            .as_chunks::<3>()
                            .0
                            .iter()
                            .map(|c| (c[1] >> 4, c[1] & 0x0F))
                            .collect();
                        frame_seen = true;
                    }
                }
                _ => {}
            }
            i += 2 + len;
        }
    }
    if !frame_seen {
        return Err(CodecError::Decode {
            codec: "jpeg",
            detail: "no frame header (SOF) before the first scan".into(),
        });
    }
    Ok(m)
}

/// Decode a JPEG chunk of a TIFF (compression 7). `tables` is the abbreviated
/// table-specification stream of the `JPEGTables` tag (`SOI`, DQT/DHT segments, `EOI`), spliced
/// in front of the chunk; `color` says whether the components are already R, G, B (TIFF
/// photometric RGB, decoded as coded) or YCbCr (photometric YCbCr, converted to R, G, B).
/// Same decoder and output conventions as [`jpeg_decode`].
pub fn jpeg_decode_tiff(data: &[u8], tables: Option<&[u8]>, color: JpegColor) -> Result<Raster> {
    jpeg_decode_tiff_limited(data, tables, color, MAX_UNSIZED_OUTPUT)
}

/// [`jpeg_decode_tiff`] with the frame-size bound of [`jpeg_decode_limited`].
pub fn jpeg_decode_tiff_limited(
    data: &[u8],
    tables: Option<&[u8]>,
    color: JpegColor,
    max_bytes: usize,
) -> Result<Raster> {
    let stream: std::borrow::Cow<'_, [u8]> = match tables {
        Some(t) if t.len() >= 4 && data.len() >= 2 => {
            if t[..2] != [0xFF, 0xD8] || data[..2] != [0xFF, 0xD8] {
                return Err(CodecError::Decode {
                    codec: "jpeg",
                    detail: "JPEG tables or image stream lack an SOI marker".into(),
                });
            }
            let t_end = if t[t.len() - 2..] == [0xFF, 0xD9] {
                t.len() - 2
            } else {
                t.len()
            };
            let mut v = Vec::with_capacity(t_end + data.len());
            v.extend_from_slice(&t[..t_end]);
            v.extend_from_slice(&data[2..]);
            std::borrow::Cow::Owned(v)
        }
        _ => std::borrow::Cow::Borrowed(data),
    };
    jpeg_decode_with(&stream, Some(color), max_bytes.min(MAX_UNSIZED_OUTPUT))
}

/// Apple/TIFF PackBits run-length decoding (TIFF compression 32773). Stops when
/// `expected_len` bytes have been produced (0 = decode the whole input).
pub fn packbits_decode(data: &[u8], expected_len: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(expected_len.min(MAX_PREALLOC));
    let mut i = 0usize;
    while i < data.len() && (expected_len == 0 || out.len() < expected_len) {
        let n = data[i] as i8;
        i += 1;
        if n >= 0 {
            let count = n as usize + 1;
            let end = i
                .checked_add(count)
                .filter(|&e| e <= data.len())
                .ok_or_else(|| CodecError::Decode {
                    codec: "packbits",
                    detail: format!("literal run of {count} bytes runs past the input"),
                })?;
            out.extend_from_slice(&data[i..end]);
            i = end;
        } else if n != -128 {
            let count = (1 - i16::from(n)) as usize;
            let b = *data.get(i).ok_or_else(|| CodecError::Decode {
                codec: "packbits",
                detail: "repeat run without a value byte".into(),
            })?;
            i += 1;
            out.extend(std::iter::repeat_n(b, count));
        }
    }
    if expected_len != 0 {
        out.truncate(expected_len);
    }
    Ok(out)
}

/// LZF decompression (the public liblzf scheme, as in Agilent `MSProfile.bin` and early
/// Bruker TDF frames): a control byte below 32 copies that many plus one literal bytes;
/// otherwise its top three bits are a back-reference length (an extra length byte follows
/// when all three are set) and its low five bits, with the next byte, the distance back.
/// `max_len` caps the output (0 = [`MAX_UNSIZED_OUTPUT`]); a stream that would produce more
/// is an error.
pub fn lzf_decode(data: &[u8], max_len: usize) -> Result<Vec<u8>> {
    let limit = if max_len == 0 {
        MAX_UNSIZED_OUTPUT
    } else {
        max_len
    };
    let bad = |detail: &str| CodecError::Decode {
        codec: "lzf",
        detail: detail.into(),
    };
    let mut out: Vec<u8> = Vec::with_capacity(limit.min(MAX_PREALLOC).min(data.len() * 4));
    let mut i = 0usize;
    while i < data.len() {
        let c = usize::from(data[i]);
        i += 1;
        if c < 32 {
            let n = c + 1;
            let lit = data
                .get(i..i + n)
                .ok_or_else(|| bad("literal run past the end of the input"))?;
            if out.len() + n > limit {
                return Err(bad("output exceeds the expected size"));
            }
            out.extend_from_slice(lit);
            i += n;
        } else {
            let mut len = c >> 5;
            if len == 7 {
                len += usize::from(*data.get(i).ok_or_else(|| bad("length byte missing"))?);
                i += 1;
            }
            let lo = usize::from(*data.get(i).ok_or_else(|| bad("distance byte missing"))?);
            i += 1;
            let back = ((c & 0x1f) << 8) + lo + 1;
            let from = out
                .len()
                .checked_sub(back)
                .ok_or_else(|| bad("back-reference before the start of the output"))?;
            let n = len + 2;
            if out.len() + n > limit {
                return Err(bad("output exceeds the expected size"));
            }
            // Overlapping copies repeat bytes written by this very reference.
            for k in 0..n {
                let v = out[from + k];
                out.push(v);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lzf_literals_and_overlapping_references() {
        // literal "ab", then a back-reference of length 4 at distance 2 ("abab"), then "c"
        let s = [0x01, b'a', b'b', 0x40, 0x01, 0x00, b'c'];
        assert_eq!(lzf_decode(&s, 0).unwrap(), b"abababc");
        assert!(lzf_decode(&s, 3).is_err(), "output cap");
        assert!(lzf_decode(&[0x05, b'a'], 0).is_err(), "short literal");
        assert!(
            lzf_decode(&[0x20, 0x05], 0).is_err(),
            "reference before start"
        );
        assert!(lzf_decode(&[0xE0], 0).is_err(), "missing length byte");
    }

    /// SOI, optional APP0 JFIF / APP14 Adobe, SOF0 with the given component ids (sampling of
    /// the first 2x2, others 1x1), SOS.
    fn jpeg_head(jfif: bool, adobe: Option<u8>, ids: &[u8]) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8];
        if jfif {
            v.extend_from_slice(&[0xFF, 0xE0, 0, 16]);
            v.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        }
        if let Some(t) = adobe {
            v.extend_from_slice(&[0xFF, 0xEE, 0, 14]);
            v.extend_from_slice(b"Adobe\0\x64\0\0\0\0");
            v.push(t);
        }
        let n = u8::try_from(ids.len()).unwrap();
        let len = 8 + 3 * u16::from(n);
        v.extend_from_slice(&[0xFF, 0xC0]);
        v.extend_from_slice(&len.to_be_bytes());
        v.extend_from_slice(&[8, 0, 16, 0, 16, n]);
        for (k, id) in ids.iter().enumerate() {
            v.extend_from_slice(&[*id, if k == 0 { 0x22 } else { 0x11 }, 0]);
        }
        v.extend_from_slice(&[0xFF, 0xDA, 0, 8, 1, 1, 0, 0, 63, 0]);
        v
    }

    #[test]
    fn jpeg_markers_and_libjpeg_colour_rules() {
        let m = jpeg_markers(&jpeg_head(true, None, &[1, 2, 3]), None).unwrap();
        assert!(m.jfif);
        assert_eq!(m.component_ids, [1, 2, 3]);
        assert_eq!(m.sampling, [(2, 2), (1, 1), (1, 1)]);
        assert_eq!(m.coded_color(), Some(JpegColor::ToRgb));
        // JFIF wins over R,G,B ids (libjpeg 6b)
        let m = jpeg_markers(&jpeg_head(true, None, b"RGB"), None).unwrap();
        assert_eq!(m.coded_color(), Some(JpegColor::ToRgb));
        // Adobe transform 0 / 1 without JFIF
        let m = jpeg_markers(&jpeg_head(false, Some(0), &[1, 2, 3]), None).unwrap();
        assert_eq!(
            (m.adobe_transform, m.coded_color()),
            (Some(0), Some(JpegColor::AsCoded))
        );
        let m = jpeg_markers(&jpeg_head(false, Some(1), b"RGB"), None).unwrap();
        assert_eq!(m.coded_color(), Some(JpegColor::ToRgb));
        // ids only
        let m = jpeg_markers(&jpeg_head(false, None, b"RGB"), None).unwrap();
        assert_eq!(m.coded_color(), Some(JpegColor::AsCoded));
        let m = jpeg_markers(&jpeg_head(false, None, &[0, 1, 2]), None).unwrap();
        assert_eq!(m.coded_color(), Some(JpegColor::ToRgb));
        let m = jpeg_markers(&jpeg_head(false, None, &[1]), None).unwrap();
        assert_eq!(m.coded_color(), None);
        // tables stream (JPEGTables) scanned first; the frame header is in the chunk
        let tables = [0xFF, 0xD8, 0xFF, 0xD9];
        let m = jpeg_markers(&jpeg_head(true, None, &[1, 2, 3]), Some(&tables)).unwrap();
        assert!(m.jfif);
        // malformed: no SOI, truncated segment, no frame header
        assert!(jpeg_markers(&[0, 1, 2], None).is_err());
        let mut t = jpeg_head(true, None, &[1, 2, 3]);
        t.truncate(12);
        assert!(jpeg_markers(&t, None).is_err());
        assert!(jpeg_markers(&[0xFF, 0xD8, 0xFF, 0xDA, 0, 2], None).is_err());
        assert!(jpeg_markers(&[0xFF, 0xD8, 0xFF, 0xC0, 0, 1], None).is_err());
    }

    /// A lossless WebP whose Huffman code counts overflow image-webp's u16 arithmetic: an error,
    /// not a panic, in every build (the workspace turns off overflow checks for image-webp).
    #[cfg(feature = "webp")]
    #[test]
    fn webp_huffman_overflow_is_an_error() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fuzz/known-upstream/image-webp-0.2.4-huffman-overflow.webp");
        let Ok(d) = std::fs::read(p) else {
            return;
        };
        assert!(webp_decode_limited(&d, 1 << 20).is_err());
    }

    /// OpenJPEG-encoded fixtures (imagecodecs 2026.8.16) and OpenJPEG's own decodes of them:
    /// lossless 14-bit grey (exact), a JP2-wrapped lossless RGB (exact) and an irreversible
    /// 9/7 + ICT RGB (within one grey level).
    #[cfg(feature = "jpeg2000")]
    #[test]
    fn jpeg2000_matches_openjpeg() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/j2k");
        for (name, channels, bits, tol) in [
            ("gray16-lossless", 1, 16, 0),
            ("rgb8-lossless-jp2", 3, 8, 0),
            ("rgb8-lossy-ict", 3, 8, 1),
        ] {
            let cs = std::fs::read(dir.join(format!("{name}.j2k"))).unwrap();
            let want = std::fs::read(dir.join(format!("{name}.openjpeg.raw"))).unwrap();
            let r = jpeg2000_decode(&cs).unwrap();
            assert_eq!(
                (r.width, r.height, r.channels, r.bits_per_sample),
                (37, 21, channels, bits),
                "{name}"
            );
            assert_eq!(r.data.len(), want.len(), "{name}");
            if bits == 16 {
                assert_eq!(r.data, want, "{name}");
            } else {
                let worst = r.data.iter().zip(&want).map(|(a, b)| a.abs_diff(*b)).max();
                assert!(worst <= Some(tol), "{name}: off by {worst:?}");
            }
            assert!(
                jpeg2000_decode(&cs[..cs.len() / 3]).is_err(),
                "{name}: truncated"
            );
            assert!(jpeg2000_decode_limited(&cs, 100).is_err(), "{name}: limit");
        }
        assert!(jpeg2000_decode(b"not a codestream").is_err());
    }

    #[test]
    fn hilo_roundtrip() {
        // samples 0x0201, 0x0403 -> lo bytes [01,03], hi bytes [02,04]
        assert_eq!(
            unshuffle_hilo(&[0x01, 0x03, 0x02, 0x04]),
            vec![0x01, 0x02, 0x03, 0x04]
        );
    }

    /// 4x4 lossless (SOF3) 16-bit JPEG of `i * 1000`, written by libjpeg-turbo via imagecodecs.
    #[cfg(feature = "jpeg")]
    #[test]
    fn jpeg_lossless_16bit_is_exact() {
        let data = [
            0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00,
            0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0xff, 0xc3, 0x00, 0x0b, 0x10, 0x00, 0x04, 0x00,
            0x04, 0x01, 0x01, 0x11, 0x00, 0xff, 0xc4, 0x00, 0x16, 0x00, 0x01, 0x01, 0x01, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0a, 0x0c,
            0x10, 0xff, 0xda, 0x00, 0x08, 0x01, 0x01, 0x00, 0x01, 0x00, 0x00, 0xcf, 0xa1, 0xf4,
            0x3e, 0x8b, 0xe8, 0x1f, 0x43, 0xe8, 0x7d, 0x17, 0xd0, 0x3e, 0x87, 0xd0, 0xfa, 0x2f,
            0xa0, 0x7d, 0x0f, 0xa1, 0xf4, 0x7f, 0xff, 0xd9,
        ];
        let r = jpeg_decode(&data).unwrap();
        assert_eq!(
            (r.width, r.height, r.channels, r.bits_per_sample),
            (4, 4, 1, 16)
        );
        let want: Vec<u8> = (0..16u16).flat_map(|i| (i * 1000).to_le_bytes()).collect();
        assert_eq!(r.data, want);
    }

    #[cfg(feature = "jpeg")]
    #[test]
    fn jpeg_rejects_garbage_cleanly() {
        assert!(jpeg_decode(&[0xFF, 0xD8, 0xFF, 0x00, 0x01]).is_err());
        assert!(jpeg_decode(&[]).is_err());
    }

    #[test]
    fn packbits_spec_example() {
        // The example from the TIFF 6.0 specification, section 9.
        let packed = [
            0xFE, 0xAA, 0x02, 0x80, 0x00, 0x2A, 0xFD, 0xAA, 0x03, 0x80, 0x00, 0x2A, 0x22, 0xF7,
            0xAA,
        ];
        let expected = [
            0xAA, 0xAA, 0xAA, 0x80, 0x00, 0x2A, 0xAA, 0xAA, 0xAA, 0xAA, 0x80, 0x00, 0x2A, 0x22,
            0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA,
        ];
        assert_eq!(packbits_decode(&packed, 0).unwrap(), expected);
        assert_eq!(packbits_decode(&packed, 5).unwrap(), expected[..5]);
        assert!(packbits_decode(&[0x05, 0x01], 0).is_err());
    }

    #[cfg(feature = "jpeg")]
    #[test]
    fn jpeg_tiff_rejects_garbage() {
        assert!(jpeg_decode_tiff(&[0xFF, 0xD8, 0x00, 0x01], None, JpegColor::AsCoded).is_err());
        assert!(
            jpeg_decode_tiff(
                &[0xFF, 0xD8, 0x00, 0x01],
                Some(&[0, 0, 0, 0]),
                JpegColor::ToRgb
            )
            .is_err()
        );
    }

    #[test]
    fn zstd1_header_parses() {
        let (h, off) = zstd1_header(&[3, 1, 1, 0x28, 0xB5, 0x2F, 0xFD]).unwrap();
        assert_eq!(off, 3);
        assert!(h.hilo);
        assert_eq!(h.chunk_kind, 1);
    }

    #[cfg(feature = "deflate")]
    #[test]
    fn zlib_output_is_capped_at_expected_len() {
        use std::io::Write;
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&vec![0u8; 1 << 20]).unwrap();
        let bomb = enc.finish().unwrap();
        assert_eq!(zlib_decode(&bomb, 100).unwrap().len(), 100);
    }

    #[cfg(feature = "lzw")]
    #[test]
    fn lzw_reused_buffer_preserves_output_and_enforces_limit() {
        let raw: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
        let packed = weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8)
            .encode(&raw)
            .unwrap();
        // Multiple staging-buffer fills, an overlong stream, then reuse after that error.
        assert_eq!(lzw_decode(&packed, raw.len()).unwrap(), raw);
        assert!(lzw_decode(&packed, 100).is_err());
        assert!(lzw_decode(&packed, raw.len() - 1).is_err());
        assert!(lzw_decode(&packed, raw.len() + 1).is_err());
        assert_eq!(lzw_decode(&packed, 0).unwrap(), raw);
        assert!(lzw_decode(&packed[..packed.len() / 2], raw.len()).is_err());
        assert_eq!(lzw_decode(&packed, raw.len()).unwrap(), raw);
        // A stream without its end-of-information code keeps what it decoded (libtiff and
        // tifffile accept such strips; the Harmony corpus has one); the length check still
        // rejects it when a size is expected.
        let head = lzw_decode(&packed[..packed.len() / 2], 0).unwrap();
        assert!(!head.is_empty() && raw.starts_with(&head));
    }

    #[cfg(feature = "zstd")]
    #[test]
    fn zstd_fast_path_and_fallback_enforce_size() {
        let raw = vec![42; 4096];
        let packed = ruzstd::encoding::compress_to_vec(
            &raw[..],
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        assert_eq!(zstd_decode(&packed, raw.len()).unwrap(), raw);
        assert_eq!(zstd_decode(&packed, 0).unwrap(), raw);
        assert!(zstd_decode(&packed, 100).is_err());
        assert!(zstd_decode(&packed, raw.len() + 1).is_err());
        // A bogus hint must not cause a hint-sized allocation in the fast path.
        assert!(zstd_decode(&packed, usize::MAX).is_err());
    }

    #[test]
    fn garbage_is_a_clean_error() {
        let junk = [0xFFu8; 64];
        assert!(zstd_decode(&junk, 16).is_err());
        assert!(zstd1_decode(&junk, 16, 2).is_err());
        assert!(zlib_decode(&junk, 16).is_err());
        assert!(zstd1_decode(&[], 16, 2).is_err());
    }
}
