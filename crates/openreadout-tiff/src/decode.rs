//! Strip/tile decoding into one plane. See `docs/formats/tiff.md` § Pixel data.
//!
//! The output is always the stored sample values, little-endian, rows without padding:
//! no photometric inversion, no palette expansion, no bit-depth scaling.

use openreadout_codecs::{
    JpegColor, JpegMarkers, jpeg_decode_tiff_limited, jpeg_markers, lzw_decode, packbits_decode,
    zlib_decode, zstd_decode,
};
use openreadout_core::bytes::{le_u16, le_u32, le_u64};
use openreadout_core::limits::plane_len;
use std::sync::Arc;

use openreadout_core::region::{Region, TileCache};
use openreadout_core::{Error, PixelType, Result};
use rayon::prelude::*;

use crate::FORMAT_ID;
use crate::container::{ByteOrder, ByteSource, Ifd};
use crate::tags;

/// Largest plane `read_plane` will materialize in memory.
pub const MAX_PLANE_BYTES: u64 = 4 << 30;

/// Everything needed to decode the pixel data of one IFD.
#[derive(Debug, Clone)]
pub struct PageLayout {
    pub width: u32,
    pub height: u32,
    pub bits_per_sample: u16,
    /// 1 = unsigned integer, 2 = signed integer, 3 = IEEE float.
    pub sample_format: u16,
    pub samples_per_pixel: u16,
    /// 1 = chunky (interleaved samples), 2 = planar (one sample per chunk).
    pub planar: u16,
    pub compression: u16,
    pub photometric: u16,
    pub predictor: u16,
    /// FillOrder: 1 = most significant bit first (default), 2 = least significant bit first.
    pub fill_order: u16,
    pub tiled: bool,
    /// Tile size, or (image width, rows per strip) for strips.
    pub chunk_width: u32,
    pub chunk_height: u32,
    pub offsets: Vec<u64>,
    pub byte_counts: Vec<u64>,
    pub jpeg_tables: Option<Vec<u8>>,
    pub byte_order: ByteOrder,
    /// Tags only some codecs need (LERC parameters, old-style JPEG tables).
    pub(crate) codec_tags: crate::chunk_codecs::CodecTags,
    /// NDPI: a complete JPEG header each chunk's bytes are wrapped in (restart-interval tiles
    /// of a full-resolution page; see the `ndpi` module).
    pub jpeg_frame: Option<std::sync::Arc<Vec<u8>>>,
    /// NDPI: offsets of the restart intervals of a single-strip JPEG page, relative to the
    /// strip (tags 65426 and 65432); region reads use them to read the page by intervals.
    pub ndpi_mcu_starts: Option<Vec<u64>>,
    /// EER (compressions 65000–65002): how the electron events of a strip are coded.
    pub eer: Option<crate::eer::EerCoding>,
    /// Byte every sample of a sparse (never written) chunk takes: 0, or 255 where the writer
    /// renders unscanned tiles white (Philips TIFF).
    pub sparse_fill: u8,
}

/// How stored samples become the samples we return (`docs/formats/tiff.md` § Sample formats).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleCoding {
    /// Stored as the returned type, in the file's byte order.
    Native,
    /// Integers of this many bits packed most significant bit first, rows padded to a byte
    /// (1-7, 9-15, 17-31 bits); returned widened to 8, 16 or 32 bits, sign-extended when signed.
    Packed { bits: u16, signed: bool },
    /// IEEE half floats, returned as 32-bit floats (exactly).
    Float16,
    /// 24-bit floats (1 sign, 7 exponent, 16 mantissa bits), returned as 32-bit floats.
    Float24,
    /// Complex integers (two signed integers of this many bits), returned as complex floats
    /// (16-bit parts) or complex doubles (32-bit parts), exactly.
    ComplexInt(u16),
}

impl SampleCoding {
    /// Bytes of one stored number, the unit of byte swapping and prediction (0 for packed bits).
    fn unit_bytes(self, out: PixelType) -> usize {
        match self {
            SampleCoding::Native if out.is_complex() => out.bytes_per_sample() / 2,
            SampleCoding::Native => out.bytes_per_sample(),
            SampleCoding::Packed { .. } => 0,
            SampleCoding::Float16 => 2,
            SampleCoding::Float24 => 3,
            SampleCoding::ComplexInt(b) => usize::from(b / 8),
        }
    }

    /// Stored bytes of `n` samples in one row.
    fn row_bytes(self, out: PixelType, bits: u16, n: usize) -> usize {
        match self {
            SampleCoding::Native => n * out.bytes_per_sample(),
            SampleCoding::Packed { .. } => (n * usize::from(bits)).div_ceil(8),
            SampleCoding::Float16 => n * 2,
            SampleCoding::Float24 => n * 3,
            SampleCoding::ComplexInt(b) => n * 2 * usize::from(b / 8),
        }
    }
}

use openreadout_core::pixel::half_to_f32;

/// 24-bit float (1 sign, 7 exponent bits biased by 63, 16 mantissa bits) → binary32 (exact).
fn float24_to_f32(v: u32) -> f32 {
    let sign = (v >> 23) & 1;
    let exp = (v >> 16) & 0x7f;
    let man = v & 0xffff;
    let bits = match (exp, man) {
        (0, 0) => sign << 31,
        (0, m) => {
            // subnormal: m * 2^(1 - 63 - 16)
            let f = (m as f32) * 2f32.powi(-78);
            return if sign == 1 { -f } else { f };
        }
        (0x7f, 0) => (sign << 31) | 0x7f80_0000,
        (0x7f, m) => (sign << 31) | 0x7fc0_0000 | (m << 7),
        (e, m) => (sign << 31) | ((e + 127 - 63) << 23) | (m << 7),
    };
    f32::from_bits(bits)
}

/// Convert one decoded chunk (`rows` rows of `n` stored samples, after byte order and predictor
/// are undone) to returned little-endian samples.
fn widen_chunk(
    coding: SampleCoding,
    out: PixelType,
    data: &[u8],
    rows: usize,
    n: usize,
    row_in: usize,
) -> Vec<u8> {
    let obps = out.bytes_per_sample();
    let mut o = Vec::with_capacity(rows * n * obps);
    for r in 0..rows {
        let row = data.get(r * row_in..(r + 1) * row_in).unwrap_or(&[]);
        match coding {
            SampleCoding::Native => o.extend_from_slice(row),
            SampleCoding::Packed { bits, signed } => {
                let bits = u32::from(bits);
                let mut acc: u64 = 0;
                let mut have = 0u32;
                let mut bytes = row.iter();
                for _ in 0..n {
                    while have < bits {
                        acc = (acc << 8) | u64::from(*bytes.next().unwrap_or(&0));
                        have += 8;
                    }
                    let mut v = ((acc >> (have - bits)) & ((1u64 << bits) - 1)) as u32;
                    have -= bits;
                    acc &= (1u64 << have) - 1;
                    if signed && v & (1 << (bits - 1)) != 0 {
                        v |= !0u32 << bits;
                    }
                    match obps {
                        1 => o.push(v as u8),
                        2 => o.extend_from_slice(&(v as u16).to_le_bytes()),
                        _ => o.extend_from_slice(&v.to_le_bytes()),
                    }
                }
            }
            SampleCoding::Float16 => {
                for c in row.as_chunks::<2>().0.iter().take(n) {
                    o.extend_from_slice(&half_to_f32(u16::from_le_bytes(*c)).to_le_bytes());
                }
            }
            SampleCoding::Float24 => {
                for c in row.as_chunks::<3>().0.iter().take(n) {
                    let v = u32::from(c[0]) | u32::from(c[1]) << 8 | u32::from(c[2]) << 16;
                    o.extend_from_slice(&float24_to_f32(v).to_le_bytes());
                }
            }
            SampleCoding::ComplexInt(16) => {
                for c in row.as_chunks::<2>().0.iter().take(n * 2) {
                    o.extend_from_slice(&f32::from(i16::from_le_bytes(*c)).to_le_bytes());
                }
            }
            SampleCoding::ComplexInt(_) => {
                for c in row.as_chunks::<4>().0.iter().take(n * 2) {
                    o.extend_from_slice(&f64::from(i32::from_le_bytes(*c)).to_le_bytes());
                }
            }
        }
    }
    o.resize(rows * n * obps, 0);
    o
}

/// Which samples of a multi-sample page to return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleSelect {
    /// All samples, interleaved (RGB).
    All,
    /// One sample (a channel stored as a sample).
    One(u16),
}

impl PageLayout {
    pub fn from_ifd(ifd: &Ifd, byte_order: ByteOrder) -> Result<Self> {
        let width = ifd.uint(tags::IMAGE_WIDTH).unwrap_or(0);
        let height = ifd.uint(tags::IMAGE_LENGTH).unwrap_or(0);
        if width == 0 || height == 0 || width > u64::from(u32::MAX) || height > u64::from(u32::MAX)
        {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                ifd.offset,
                format!(
                    "IFD at {} has invalid dimensions {width}×{height}",
                    ifd.offset
                ),
            ));
        }
        let (width, height) = (width as u32, height as u32);
        let spp = ifd.uint(tags::SAMPLES_PER_PIXEL).unwrap_or(1).clamp(1, 64) as u16;
        let mut bits = ifd.uints(tags::BITS_PER_SAMPLE).unwrap_or_else(|| vec![1]);
        // Only the first SamplesPerPixel entries count (some LSM writers list one per
        // channel of the whole acquisition, including the thumbnail's).
        bits.truncate(usize::from(spp).max(1));
        let bits_per_sample = bits.first().copied().unwrap_or(1) as u16;
        if bits.iter().any(|&b| b != u64::from(bits_per_sample)) {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("samples with different bit depths {bits:?}"),
                "Pages whose samples differ in bit depth are not decoded.",
            ));
        }
        let sample_format = ifd.uint(tags::SAMPLE_FORMAT).unwrap_or(1) as u16;
        let tiled = ifd.field(tags::TILE_WIDTH).is_some();
        let (chunk_width, chunk_height, offsets, byte_counts) = if tiled {
            let tw = ifd.uint(tags::TILE_WIDTH).unwrap_or(0);
            let th = ifd.uint(tags::TILE_LENGTH).unwrap_or(0);
            if tw == 0 || th == 0 || tw > 1 << 16 || th > 1 << 16 {
                return Err(Error::corrupt_at(
                    FORMAT_ID,
                    ifd.offset,
                    format!("invalid tile size {tw}×{th}"),
                ));
            }
            (
                tw as u32,
                th as u32,
                ifd.uints(tags::TILE_OFFSETS).unwrap_or_default(),
                ifd.uints(tags::TILE_BYTE_COUNTS).unwrap_or_default(),
            )
        } else {
            let rps = ifd
                .uint(tags::ROWS_PER_STRIP)
                .filter(|&r| r > 0)
                .unwrap_or(u64::from(height))
                .min(u64::from(height)) as u32;
            (
                width,
                rps,
                ifd.uints(tags::STRIP_OFFSETS).unwrap_or_default(),
                ifd.uints(tags::STRIP_BYTE_COUNTS).unwrap_or_default(),
            )
        };
        Ok(PageLayout {
            width,
            height,
            bits_per_sample,
            sample_format,
            samples_per_pixel: spp,
            planar: ifd.uint(tags::PLANAR_CONFIGURATION).unwrap_or(1) as u16,
            compression: ifd.uint(tags::COMPRESSION).unwrap_or(1) as u16,
            photometric: ifd.uint(tags::PHOTOMETRIC).unwrap_or(1) as u16,
            predictor: ifd.uint(tags::PREDICTOR).unwrap_or(1) as u16,
            fill_order: ifd.uint(tags::FILL_ORDER).unwrap_or(1) as u16,
            tiled,
            chunk_width,
            chunk_height,
            offsets,
            byte_counts,
            jpeg_tables: ifd.bytes(tags::JPEG_TABLES).map(<[u8]>::to_vec),
            byte_order,
            codec_tags: crate::chunk_codecs::CodecTags::from_ifd(ifd),
            jpeg_frame: None,
            ndpi_mcu_starts: ndpi_starts(ifd, tiled),
            eer: crate::eer::EerCoding::of(ifd.uint(tags::COMPRESSION).unwrap_or(1) as u16, ifd),
            sparse_fill: 0,
        })
    }

    /// Returned sample type, or an unsupported-feature error for bit depths we do not decode.
    pub fn pixel_type(&self) -> Result<PixelType> {
        Ok(self.coding()?.0)
    }

    /// Returned sample type and how stored samples become it.
    pub fn coding(&self) -> Result<(PixelType, SampleCoding)> {
        use SampleCoding as S;
        let native = |pt| Ok((pt, S::Native));
        if self.eer.is_some() {
            // electron events decode to one count per pixel (BitsPerSample, 1 or absent, is
            // not the coding)
            return native(PixelType::Uint8);
        }
        match (self.bits_per_sample, self.sample_format) {
            // 12-bit JPEG: the codec returns whole 16-bit samples, not packed bits.
            (12, 1 | 4) if self.compression == 7 => native(PixelType::Uint16),
            (8, 1 | 4) => native(PixelType::Uint8),
            (8, 2) => native(PixelType::Int8),
            (16, 1 | 4) => native(PixelType::Uint16),
            (16, 2) => native(PixelType::Int16),
            (32, 1 | 4) => native(PixelType::Uint32),
            (32, 2) => native(PixelType::Int32),
            (32, 3) => native(PixelType::Float),
            (64, 3) => native(PixelType::Double),
            (64, 1 | 4) => native(PixelType::Uint64),
            (64, 2) => native(PixelType::Int64),
            (64, 6) => native(PixelType::ComplexFloat),
            (128, 6) => native(PixelType::ComplexDouble),
            (16, 3) => Ok((PixelType::Float, S::Float16)),
            (24, 3) => Ok((PixelType::Float, S::Float24)),
            (32, 5) => Ok((PixelType::ComplexFloat, S::ComplexInt(16))),
            (64, 5) => Ok((PixelType::ComplexDouble, S::ComplexInt(32))),
            (bits @ 1..=31, fmt @ (1 | 2 | 4)) => {
                let out = match bits {
                    1..=8 if fmt == 2 => PixelType::Int8,
                    1..=8 => PixelType::Uint8,
                    9..=16 if fmt == 2 => PixelType::Int16,
                    9..=16 => PixelType::Uint16,
                    _ if fmt == 2 => PixelType::Int32,
                    _ => PixelType::Uint32,
                };
                Ok((
                    out,
                    S::Packed {
                        bits,
                        signed: fmt == 2,
                    },
                ))
            }
            (bits, fmt) => Err(Error::unsupported(
                FORMAT_ID,
                format!("{bits}-bit samples with sample format {fmt}"),
                "Decoded: 1-31-bit integers, 64-bit integers, 16/24/32/64-bit floats and complex floats and integers; this bit depth and sample format are not.",
            )),
        }
    }

    pub fn chunks_across(&self) -> u32 {
        self.width.div_ceil(self.chunk_width.max(1))
    }
    pub fn chunks_down(&self) -> u32 {
        self.height.div_ceil(self.chunk_height.max(1))
    }
    /// Chunks per sample plane.
    pub fn chunks_per_plane(&self) -> u64 {
        u64::from(self.chunks_across()) * u64::from(self.chunks_down())
    }
    /// Chunks the geometry requires.
    pub fn expected_chunks(&self) -> u64 {
        let planes = if self.planar == 2 {
            u64::from(self.samples_per_pixel)
        } else {
            1
        };
        self.chunks_per_plane() * planes
    }
    /// True for RGB/YCbCr pages with interleaved samples.
    pub fn is_rgb(&self) -> bool {
        matches!(self.photometric, 2 | 6) && self.samples_per_pixel >= 3 && self.planar != 2
    }
    /// Human name of the compression scheme.
    pub fn compression_name(&self) -> String {
        compression_name(self.compression)
    }
}

/// NDPI restart-interval offsets of a single-strip JPEG page (low 32 bits in tag 65426, high
/// bits, when present, in tag 65432).
fn ndpi_starts(ifd: &Ifd, tiled: bool) -> Option<Vec<u64>> {
    if tiled || ifd.uint(tags::COMPRESSION) != Some(7) {
        return None;
    }
    let low = ifd.uints(crate::ndpi::MCU_STARTS)?;
    if low.is_empty() || ifd.uints(tags::STRIP_OFFSETS).is_none_or(|v| v.len() != 1) {
        return None;
    }
    let high = ifd.uints(crate::ndpi::MCU_STARTS_HIGH);
    Some(
        low.iter()
            .enumerate()
            .map(|(i, &l)| {
                let h = high.as_ref().and_then(|v| v.get(i)).copied().unwrap_or(0);
                (h << 32) | (l & 0xFFFF_FFFF)
            })
            .collect(),
    )
}

/// Human name of a TIFF compression code.
pub fn compression_name(code: u16) -> String {
    match code {
        1 => "none".into(),
        5 => "lzw".into(),
        6 => "old-jpeg".into(),
        7 => "jpeg".into(),
        8 | 32946 => "deflate".into(),
        32773 => "packbits".into(),
        33003 | 33004 | 33005 | 34712 => "jpeg-2000".into(),
        34887 => "lerc".into(),
        50000 => "zstd".into(),
        50001 => "webp".into(),
        50002 | 52546 => "jpeg-xl".into(),
        65000..=65002 => "eer".into(),
        other => format!("code-{other}"),
    }
}

/// How the JPEG chunks of a page are turned into samples, decided from the page's tags and
/// the chunk's own markers (see `docs/formats/tiff.md` § JPEG colour).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct JpegDecision {
    pub color: JpegColor,
    /// The markers and the photometric tag disagree and the tag was followed (an `Adobe`
    /// segment declaring YCbCr in a photometric-RGB page without JFIF).
    pub conflict: bool,
}

/// Page-level refusals for JPEG (compression 7): combinations whose samples we cannot turn
/// into what the page declares without guessing.
pub(crate) fn jpeg_page_supported(layout: &PageLayout) -> Result<()> {
    match (layout.photometric, layout.planar) {
        (0..=2, _) | (6, 1) => Ok(()),
        (6, _) => Err(Error::unsupported(
            FORMAT_ID,
            "JPEG-compressed YCbCr stored as separate sample planes (PlanarConfiguration 2)",
            "Each plane holds Y, Cb or Cr, not R, G, B; converting them is not implemented, so no pixels are returned rather than wrong ones. Re-save the image as RGB with interleaved samples or a lossless codec (e.g. bfconvert -compression LZW).",
        )),
        (p, _) => Err(Error::unsupported(
            FORMAT_ID,
            format!("JPEG compression with photometric interpretation {p}"),
            "Only grey, RGB and YCbCr JPEG pages are decoded; re-save the image with a lossless codec.",
        )),
    }
}

/// The colour transform for one JPEG chunk. One-component chunks need none. For three
/// components the rules follow libjpeg as tifffile drives it (BSD-3, read as documentation):
/// photometric YCbCr → the stream's own markers decide (JFIF, Adobe, component ids, else
/// YCbCr); photometric RGB → as coded, unless a JFIF segment says the components are Y, Cb,
/// Cr (Bio-Formats writes JFIF streams with 2×2 chroma subsampling into RGB pages; Aperio
/// SVS and PerkinElmer QPTIFF write plain R, G, B without JFIF).
pub(crate) fn jpeg_color(layout: &PageLayout, m: &JpegMarkers) -> Result<JpegDecision> {
    jpeg_page_supported(layout)?;
    let plain = JpegDecision {
        color: JpegColor::AsCoded,
        conflict: false,
    };
    match m.component_ids.len() {
        1 => return Ok(plain),
        3 => {}
        n => {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("a JPEG stream with {n} components"),
                "Only one- and three-component JPEG streams are decoded.",
            ));
        }
    }
    Ok(match layout.photometric {
        6 => JpegDecision {
            color: m.coded_color().unwrap_or(JpegColor::ToRgb),
            conflict: false,
        },
        2 if m.jfif => JpegDecision {
            color: JpegColor::ToRgb,
            conflict: false,
        },
        2 => JpegDecision {
            color: JpegColor::AsCoded,
            conflict: m.adobe_transform.is_some_and(|t| t != 0),
        },
        p => {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!(
                    "a three-component JPEG stream in a page with photometric interpretation {p}"
                ),
                "The page's photometric interpretation does not say how to read colour samples; no pixels are returned rather than guessed ones.",
            ));
        }
    })
}

/// Decode one page (all chunks) into a plane buffer.
pub fn read_page(
    src: &mut ByteSource,
    layout: &PageLayout,
    select: SampleSelect,
) -> Result<Vec<u8>> {
    let pt = layout.pixel_type()?;
    let bps = pt.bytes_per_sample();
    let spp = usize::from(layout.samples_per_pixel);
    if let SampleSelect::One(s) = select
        && usize::from(s) >= spp
    {
        return Err(Error::Usage(format!(
            "sample {s} out of range (page has {spp} samples)"
        )));
    }
    if layout.photometric == 6
        && !matches!(layout.compression, 6 | 7 | 33003 | 33004 | 33005 | 34712)
    {
        return Err(Error::unsupported(
            FORMAT_ID,
            "YCbCr samples outside JPEG compression",
            "Only JPEG-compressed YCbCr pages are decoded.",
        ));
    }
    if layout.compression == 7 {
        jpeg_page_supported(layout)?;
    }
    let out_spp = match select {
        SampleSelect::All => spp,
        SampleSelect::One(_) => 1,
    };
    let (w, h) = (layout.width as usize, layout.height as usize);
    let out_len = (w as u64)
        .checked_mul(h as u64)
        .and_then(|v| v.checked_mul((out_spp * bps) as u64))
        .filter(|&v| v <= MAX_PLANE_BYTES)
        .ok_or_else(|| {
            Error::unsupported(
                FORMAT_ID,
                format!("a {w}×{h} plane larger than {} GiB", MAX_PLANE_BYTES >> 30),
                "Planes this large are listed by info/ls and checked by check, but not read into memory; region and pyramid-level reads are not supported yet.",
            )
        })?;
    // Also capped relative to the file, so a few-KB file cannot make us allocate gigabytes.
    let out_len = plane_len(FORMAT_ID, out_len, 1, 1, 1, src.len)?;
    let prepared = crate::chunk_codecs::prepare(src, layout)?;
    let mut out = vec![0u8; out_len];
    let per_plane = layout.chunks_per_plane();
    let across = u64::from(layout.chunks_across());
    let (cw, ch) = (layout.chunk_width as usize, layout.chunk_height as usize);
    let chunk_spp = if layout.planar == 2 { 1 } else { spp };
    let sample_planes: Vec<usize> = if layout.planar == 2 {
        match select {
            SampleSelect::All => (0..spp).collect(),
            SampleSelect::One(s) => vec![usize::from(s)],
        }
    } else {
        vec![0]
    };
    // (sample plane, chunk index, x0, y0, rows coded in the chunk)
    let jobs: Vec<(usize, u64, usize, usize, usize)> = sample_planes
        .iter()
        .flat_map(|&sp| {
            (0..per_plane).map(move |i| {
                let idx = if layout.planar == 2 {
                    sp as u64 * per_plane + i
                } else {
                    i
                };
                let x0 = (i % across) as usize * cw;
                let y0 = (i / across) as usize * ch;
                let rows_in_chunk = if layout.tiled { ch } else { ch.min(h - y0) };
                (sp, idx, x0, y0, rows_in_chunk)
            })
        })
        .collect();
    // Chunks are read in file order, then decoded in parallel when the codec is costly (JPEG,
    // JPEG 2000, deflate, zstd, ...): a batch of a few chunks per thread at a time, so memory
    // stays at a few compressed and decoded tiles per thread.
    let threads = rayon::current_num_threads();
    let batch = if threads > 1 && cpu_heavy(layout.compression) {
        threads * 4
    } else {
        1
    };
    for group in jobs.chunks(batch) {
        let raws = group
            .iter()
            .map(|&(_, idx, ..)| fetch_chunk(src, layout, idx))
            .collect::<Result<Vec<_>>>()?;
        let len = src.len;
        let decode_one =
            |(&(_, idx, _, _, rows), raw): (&(usize, u64, usize, usize, usize), Fetched)| {
                let chunk = Chunk {
                    idx,
                    cw,
                    rows,
                    spp: chunk_spp,
                    bps,
                };
                decode_fetched(layout, &prepared, chunk, raw, len)
            };
        let decoded: Vec<Result<Vec<u8>>> = if group.len() > 1 {
            group
                .par_iter()
                .zip(raws.into_par_iter())
                .map(decode_one)
                .collect()
        } else {
            group.iter().zip(raws).map(decode_one).collect()
        };
        for (&(sp, _, x0, y0, rows_in_chunk), chunk) in group.iter().zip(decoded) {
            let chunk = chunk?;
            let rows = rows_in_chunk.min(h - y0);
            let cols = cw.min(w - x0);
            let chunk_row = cw * chunk_spp * bps;
            for r in 0..rows {
                let src_row = &chunk[r * chunk_row..(r + 1) * chunk_row];
                let dst_base = ((y0 + r) * w + x0) * out_spp * bps;
                match (layout.planar == 2, select) {
                    (false, SampleSelect::All) | (true, SampleSelect::One(_)) => {
                        let n = cols * out_spp * bps;
                        out[dst_base..dst_base + n].copy_from_slice(&src_row[..n]);
                    }
                    (false, SampleSelect::One(s)) => {
                        let s = usize::from(s);
                        for x in 0..cols {
                            let from = (x * spp + s) * bps;
                            let to = dst_base + x * bps;
                            out[to..to + bps].copy_from_slice(&src_row[from..from + bps]);
                        }
                    }
                    (true, SampleSelect::All) => {
                        for x in 0..cols {
                            let from = x * bps;
                            let to = dst_base + (x * spp + sp) * bps;
                            out[to..to + bps].copy_from_slice(&src_row[from..from + bps]);
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Decoded chunks kept between region reads, keyed by (file-set member, chunk offset).
pub type ChunkCache = TileCache<(usize, u64), Vec<u8>>;

/// One chunk's pixels for a region read: columns `bx..bx + bw` of rows `by..by + bh` (plane
/// coordinates), rows of `bw * chunk_spp * bps` bytes, of sample plane `sp`.
struct Piece {
    sp: usize,
    bx: usize,
    by: usize,
    bw: usize,
    bh: usize,
    data: Arc<Vec<u8>>,
}

/// A chunk read from the file, to be decoded into `pieces[slot]`.
struct Pending {
    slot: usize,
    idx: u64,
    off: u64,
    raw: Vec<u8>,
    rows: usize,
}

/// Decode only the rectangle `region` of one page: the chunks (tiles or strips) it overlaps
/// are read, decoded in parallel and cropped into a region-sized buffer. Uncompressed chunks
/// are read row segment by row segment, so a window of a huge uncompressed page reads only its
/// own bytes. `cache` keeps decoded chunks for the next read (`member` names the file-set
/// member in its keys). Same sample conventions as [`read_page`].
pub fn read_region(
    src: &mut ByteSource,
    layout: &PageLayout,
    select: SampleSelect,
    region: Region,
    mut cache: Option<(&mut ChunkCache, usize)>,
) -> Result<Vec<u8>> {
    let pt = layout.pixel_type()?;
    let bps = pt.bytes_per_sample();
    let spp = usize::from(layout.samples_per_pixel);
    check_region_request(layout, select, spp)?;
    region.check_within(layout.width, layout.height, "the page")?;
    if let Some(starts) = &layout.ndpi_mcu_starts {
        // A whole-slide JPEG strip: read it by restart intervals.
        let t = crate::ndpi::tiled_layout(src, layout, starts)?;
        return read_region(src, &t, select, region, cache);
    }
    let prepared = crate::chunk_codecs::prepare(src, layout)?;
    let out_spp = match select {
        SampleSelect::All => spp,
        SampleSelect::One(_) => 1,
    };
    let mut out = openreadout_core::region::region_buffer(FORMAT_ID, region, out_spp * bps)?;
    let g = RegionGeometry {
        w: layout.width as usize,
        h: layout.height as usize,
        cw: layout.chunk_width.max(1) as usize,
        spp,
        chunk_spp: if layout.planar == 2 { 1 } else { spp },
        out_spp,
        bps,
        rx0: region.x as usize,
        ry0: region.y as usize,
        rx1: region.right() as usize,
        ry1: region.bottom() as usize,
        out_row: region.width as usize * out_spp * bps,
    };
    let (h, cw, chunk_spp) = (g.h, g.cw, g.chunk_spp);
    let ch = layout.chunk_height.max(1) as usize;
    let per_plane = layout.chunks_per_plane();
    let across = u64::from(layout.chunks_across());
    let sample_planes: Vec<usize> = if layout.planar == 2 {
        match select {
            SampleSelect::All => (0..spp).collect(),
            SampleSelect::One(s) => vec![usize::from(s)],
        }
    } else {
        vec![0]
    };
    let (rx0, ry0, rx1, ry1) = (g.rx0, g.ry0, g.rx1, g.ry1);
    let raw_rows = layout.compression == 1
        && layout.predictor == 1
        && layout.coding()?.1 == SampleCoding::Native;
    let mut pieces: Vec<Piece> = Vec::new();
    let mut pending: Vec<Pending> = Vec::new();
    for &sp in &sample_planes {
        for cy in ry0 / ch..=(ry1 - 1) / ch {
            for cx in rx0 / cw..=(rx1 - 1) / cw {
                let i = cy as u64 * across + cx as u64;
                let idx = if layout.planar == 2 {
                    sp as u64 * per_plane + i
                } else {
                    i
                };
                let (x0, y0) = (cx * cw, cy * ch);
                let rows = if layout.tiled { ch } else { ch.min(h - y0) };
                let ui = usize::try_from(idx).unwrap_or(usize::MAX);
                let (Some(&off), Some(&count)) =
                    (layout.offsets.get(ui), layout.byte_counts.get(ui))
                else {
                    return Err(missing_chunk(layout, idx));
                };
                let mut piece = Piece {
                    sp,
                    bx: x0,
                    by: y0,
                    bw: cw,
                    bh: rows,
                    data: Arc::new(Vec::new()),
                };
                if raw_rows {
                    pieces.push(read_raw_rows(src, layout, &g, &piece, idx, off, count)?);
                    continue;
                }
                if let Some((c, member)) = cache.as_mut()
                    && off != 0
                    && let Some(d) = c.get(&(*member, off))
                {
                    piece.data = d;
                    pieces.push(piece);
                    continue;
                }
                match fetch_chunk(src, layout, idx)? {
                    None => {
                        let n = plane_len(
                            FORMAT_ID,
                            cw as u64,
                            rows as u64,
                            chunk_spp as u64,
                            bps as u64,
                            src.len,
                        )?;
                        piece.data = Arc::new(vec![layout.sparse_fill; n]);
                    }
                    Some((off, raw)) => pending.push(Pending {
                        slot: pieces.len(),
                        idx,
                        off,
                        raw,
                        rows,
                    }),
                }
                pieces.push(piece);
            }
        }
    }
    let file_len = src.len;
    let decoded: Vec<(usize, u64, Result<Vec<u8>>)> = pending
        .into_par_iter()
        .map(|p| {
            let chunk = Chunk {
                idx: p.idx,
                cw,
                rows: p.rows,
                spp: chunk_spp,
                bps,
            };
            let d = decode_fetched(layout, &prepared, chunk, Some((p.off, p.raw)), file_len);
            (p.slot, p.off, d)
        })
        .collect();
    for (slot, off, d) in decoded {
        let d = Arc::new(d?);
        if let Some((c, member)) = cache.as_mut() {
            let n = d.len();
            c.put((*member, off), d.clone(), n);
        }
        if let Some(p) = pieces.get_mut(slot) {
            p.data = d;
        }
    }
    paste_pieces(&mut out, &pieces, layout, select, &g)?;
    Ok(out)
}

/// The sample `select` picks must exist, and YCbCr pages are decoded only from JPEG.
fn check_region_request(layout: &PageLayout, select: SampleSelect, spp: usize) -> Result<()> {
    if let SampleSelect::One(s) = select
        && usize::from(s) >= spp
    {
        return Err(Error::Usage(format!(
            "sample {s} out of range (page has {spp} samples)"
        )));
    }
    if layout.photometric == 6
        && !matches!(layout.compression, 6 | 7 | 33003 | 33004 | 33005 | 34712)
    {
        return Err(Error::unsupported(
            FORMAT_ID,
            "YCbCr samples outside JPEG compression",
            "Only JPEG-compressed YCbCr pages are decoded.",
        ));
    }
    Ok(())
}

/// Page, chunk and region geometry shared by the steps of [`read_region`]: page `w × h`,
/// chunks `cw` wide, the region's plane coordinates `rx0..rx1 × ry0..ry1` and its output rows
/// of `out_row` bytes.
struct RegionGeometry {
    w: usize,
    h: usize,
    cw: usize,
    spp: usize,
    chunk_spp: usize,
    out_spp: usize,
    bps: usize,
    rx0: usize,
    ry0: usize,
    rx1: usize,
    ry1: usize,
    out_row: usize,
}

/// Uncompressed: read only the row segments of `chunk` (chunk `idx`, `count` bytes at `off`)
/// that overlap the region.
fn read_raw_rows(
    src: &mut ByteSource,
    layout: &PageLayout,
    g: &RegionGeometry,
    chunk: &Piece,
    idx: u64,
    off: u64,
    count: u64,
) -> Result<Piece> {
    let (cw, rows, chunk_spp, bps) = (g.cw, chunk.bh, g.chunk_spp, g.bps);
    let (x0, y0) = (chunk.bx, chunk.by);
    let (ox0, ox1) = (x0.max(g.rx0), (x0 + cw).min(g.rx1).min(g.w));
    let (oy0, oy1) = (y0.max(g.ry0), (y0 + rows).min(g.ry1).min(g.h));
    let bw = ox1 - ox0;
    let row_bytes = cw * chunk_spp * bps;
    let seg = bw * chunk_spp * bps;
    let mut data = vec![0u8; seg * (oy1 - oy0)];
    if off != 0 && count != 0 {
        for (k, y) in (oy0..oy1).enumerate() {
            let at = ((y - y0) * row_bytes + (ox0 - x0) * chunk_spp * bps) as u64;
            if at + seg as u64 > count {
                return Err(Error::corrupt_at(
                    FORMAT_ID,
                    off,
                    format!(
                        "uncompressed chunk {idx} holds {count} bytes, fewer than its {cw}x{rows} geometry"
                    ),
                ));
            }
            let got = src.read_at(off + at, seg as u64)?;
            data[k * seg..(k + 1) * seg].copy_from_slice(&got);
        }
        if layout.byte_order == ByteOrder::Big && bps > 1 {
            for v in data.chunks_exact_mut(bps) {
                v.reverse();
            }
        }
    }
    Ok(Piece {
        sp: chunk.sp,
        bx: ox0,
        by: oy0,
        bw,
        bh: oy1 - oy0,
        data: Arc::new(data),
    })
}

/// Copy the part of each piece inside the region into `out`, keeping the samples `select`
/// asks for.
fn paste_pieces(
    out: &mut [u8],
    pieces: &[Piece],
    layout: &PageLayout,
    select: SampleSelect,
    g: &RegionGeometry,
) -> Result<()> {
    let (spp, chunk_spp, out_spp, bps) = (g.spp, g.chunk_spp, g.out_spp, g.bps);
    let (rx0, ry0, rx1, ry1) = (g.rx0, g.ry0, g.rx1, g.ry1);
    let out_row = g.out_row;
    for p in pieces {
        let row = p.bw * chunk_spp * bps;
        let (ox0, ox1) = (p.bx.max(rx0), (p.bx + p.bw).min(rx1).min(g.w));
        let (oy0, oy1) = (p.by.max(ry0), (p.by + p.bh).min(ry1).min(g.h));
        if ox1 <= ox0 || oy1 <= oy0 {
            continue;
        }
        let cols = ox1 - ox0;
        for y in oy0..oy1 {
            let start = (y - p.by) * row + (ox0 - p.bx) * chunk_spp * bps;
            let Some(src_row) = p.data.get(start..(y - p.by + 1) * row) else {
                return Err(Error::corrupt(
                    FORMAT_ID,
                    format!("chunk data for row {y} is shorter than the chunk geometry"),
                ));
            };
            let dst_base = (y - ry0) * out_row + (ox0 - rx0) * out_spp * bps;
            match (layout.planar == 2, select) {
                (false, SampleSelect::All) | (true, SampleSelect::One(_)) => {
                    let n = cols * out_spp * bps;
                    out[dst_base..dst_base + n].copy_from_slice(&src_row[..n]);
                }
                (false, SampleSelect::One(s)) => {
                    let s = usize::from(s);
                    for x in 0..cols {
                        let from = (x * spp + s) * bps;
                        let to = dst_base + x * bps;
                        out[to..to + bps].copy_from_slice(&src_row[from..from + bps]);
                    }
                }
                (true, SampleSelect::All) => {
                    for x in 0..cols {
                        let from = x * bps;
                        let to = dst_base + (x * spp + p.sp) * bps;
                        out[to..to + bps].copy_from_slice(&src_row[from..from + bps]);
                    }
                }
            }
        }
    }
    Ok(())
}

/// The error for a chunk index beyond the page's offset or byte-count table.
fn missing_chunk(layout: &PageLayout, idx: u64) -> Error {
    Error::corrupt(
        FORMAT_ID,
        format!(
            "chunk {idx} is missing: the page lists {} offsets and {} byte counts but its geometry needs {}",
            layout.offsets.len(),
            layout.byte_counts.len(),
            layout.expected_chunks()
        ),
    )
}

/// Compressions worth decoding on several threads.
fn cpu_heavy(compression: u16) -> bool {
    matches!(
        compression,
        5 | 6
            | 7
            | 8
            | 32946
            | 50000
            | 33003
            | 33004
            | 33005
            | 34712
            | 34887
            | 50001
            | 50002
            | 52546
            | 65000
            | 65001
            | 65002
    )
}

/// True when chunks of this compression are decoded (`check` reports the others).
pub fn is_decoded(compression: u16) -> bool {
    matches!(
        compression,
        1 | 5
            | 6
            | 7
            | 8
            | 32946
            | 32773
            | 50000
            | 33003
            | 33004
            | 33005
            | 34712
            | 50001
            | 50002
            | 52546
            | 65000
            | 65001
            | 65002
    ) || (compression == 34887 && cfg!(feature = "lerc"))
}

/// The stored bytes of one chunk and where they start; `None` for a sparse (never written)
/// chunk.
type Fetched = Option<(u64, Vec<u8>)>;

/// Read the stored bytes of chunk `idx` (sequential I/O; decoding may then run elsewhere).
fn fetch_chunk(src: &mut ByteSource, layout: &PageLayout, idx: u64) -> Result<Fetched> {
    let i = usize::try_from(idx).unwrap_or(usize::MAX);
    let (Some(&off), Some(&count)) = (layout.offsets.get(i), layout.byte_counts.get(i)) else {
        return Err(missing_chunk(layout, idx));
    };
    if off == 0 || count == 0 {
        // Sparse chunk: never written. Read as `layout.sparse_fill` (zeros, white for Philips).
        return Ok(None);
    }
    if off.checked_add(count).is_none_or(|e| e > src.len) {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            off,
            format!(
                "chunk {idx} ({count} bytes at offset {off}) runs past the end of the file ({} bytes); the file is truncated",
                src.len
            ),
        ));
    }
    Ok(Some((off, src.read_at(off, count)?)))
}

/// Chunk `idx` and the `cw × rows × spp` little-endian samples of `bps` bytes it decodes to.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Chunk {
    pub(crate) idx: u64,
    pub(crate) cw: usize,
    pub(crate) rows: usize,
    pub(crate) spp: usize,
    pub(crate) bps: usize,
}

/// Decode a chunk (as read by [`fetch_chunk`]) to `cw × rows × spp` little-endian samples.
/// `file_len` bounds the allocation (see `plane_len`).
fn decode_fetched(
    layout: &PageLayout,
    prepared: &crate::chunk_codecs::Prepared,
    chunk: Chunk,
    fetched: Fetched,
    file_len: u64,
) -> Result<Vec<u8>> {
    let Chunk {
        idx,
        cw,
        rows,
        spp: chunk_spp,
        bps,
    } = chunk;
    let expected = plane_len(
        FORMAT_ID,
        cw as u64,
        rows as u64,
        chunk_spp as u64,
        bps as u64,
        file_len,
    )?;
    let Some((off, raw)) = fetched else {
        return Ok(vec![layout.sparse_fill; expected]);
    };
    let (out_pt, coding) = layout.coding()?;
    if coding != SampleCoding::Native {
        return decode_coded(layout, chunk, off, raw, out_pt, coding, expected);
    }
    let codec_err = |e: openreadout_codecs::CodecError| match e {
        openreadout_codecs::CodecError::Unsupported { detail, .. } => Error::unsupported(
            FORMAT_ID,
            format!("chunk {idx}: {detail}"),
            "This codestream uses a feature the decoder does not implement.",
        ),
        e => Error::corrupt_at(FORMAT_ID, off, format!("chunk {idx}: {e}")),
    };
    // Codecs that return decoded little-endian samples: no byte swap, no predictor.
    let native = matches!(
        layout.compression,
        33003 | 33004 | 33005 | 34712 | 65000..=65002
    ) || crate::chunk_codecs::handles(layout.compression)
        || (layout.compression == 7 && bps == 2);
    let mut data = match layout.compression {
        1 => raw,
        5 => lzw_decode(&raw, 0).map_err(codec_err)?,
        8 | 32946 => zlib_decode(&raw, expected).map_err(codec_err)?,
        32773 => packbits_decode(&raw, expected).map_err(codec_err)?,
        50000 => zstd_decode(&raw, 0).map_err(codec_err)?,
        7 => {
            if !(bps == 1 || (bps == 2 && layout.bits_per_sample == 12)) {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    format!("{}-bit JPEG", layout.bits_per_sample),
                    "Only 8-bit and 12-bit JPEG are decoded.",
                ));
            }
            // An NDPI restart interval has no header of its own: frame it before the markers
            // are read (the header holds the frame's components and any JFIF/Adobe segment).
            let framed = layout
                .jpeg_frame
                .as_ref()
                .map(|h| crate::ndpi::frame(h, &raw));
            let (stream, tables) = match &framed {
                Some(f) => (f.as_slice(), None),
                None => (raw.as_slice(), layout.jpeg_tables.as_deref()),
            };
            let markers = jpeg_markers(stream, tables).map_err(codec_err)?;
            let color = jpeg_color(layout, &markers)?.color;
            // The frame may be padded to whole MCUs (and a last strip coded at full height);
            // anything much larger than the chunk is refused before decoding.
            let cap = cw
                .saturating_mul(rows.max(layout.chunk_height as usize))
                .saturating_mul(chunk_spp)
                .saturating_mul(2)
                .max(1 << 20);
            let r = jpeg_decode_tiff_limited(stream, tables, color, cap).map_err(codec_err)?;
            if r.channels as usize != chunk_spp {
                return Err(Error::corrupt_at(
                    FORMAT_ID,
                    off,
                    format!(
                        "chunk {idx}: JPEG has {} components, the page declares {chunk_spp} samples",
                        r.channels
                    ),
                ));
            }
            let (jw, jh) = (r.width as usize, r.height as usize);
            if jw == cw && jh >= rows {
                let mut d = r.data;
                d.truncate(expected);
                d
            } else {
                // Re-grid into the chunk's geometry (JPEG narrower/shorter than the chunk).
                let mut d = vec![0u8; expected];
                let row_out = cw * chunk_spp;
                let row_in = jw * chunk_spp;
                for y in 0..rows.min(jh) {
                    let n = row_out.min(row_in);
                    d[y * row_out..y * row_out + n]
                        .copy_from_slice(&r.data[y * row_in..y * row_in + n]);
                }
                d
            }
        }
        6 | 34887 | 50001 | 50002 | 52546 => {
            crate::chunk_codecs::decode_chunk(layout, prepared, chunk, off, &raw, expected)?
        }
        65000..=65002 => {
            let coding = layout.eer.ok_or_else(|| {
                Error::corrupt_at(
                    FORMAT_ID,
                    off,
                    format!("chunk {idx}: EER page without a coding"),
                )
            })?;
            if chunk_spp != 1 {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    format!("EER page with {chunk_spp} samples per pixel"),
                    "EER pages hold one sample (electron counts).",
                ));
            }
            crate::eer::decode_counts(&raw, cw, rows, coding)
                .map_err(|e| Error::corrupt_at(FORMAT_ID, off, format!("chunk {idx}: {e}")))?
        }
        33003 | 33004 | 33005 | 34712 => {
            crate::jpeg2000::decode_chunk(layout, chunk, off, &raw, expected)?
        }
        other => {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("TIFF compression {other} ({})", compression_name(other)),
                "Supported: none, LZW, deflate, PackBits, JPEG (old and new style), JPEG 2000, JPEG XL, WebP, LERC, zstd, EER.",
            ));
        }
    };
    pad_short_tile(layout, idx, &mut data, expected / rows.max(1), expected);
    if data.len() < expected {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            off,
            format!(
                "chunk {idx} decodes to {} bytes, expected {expected}",
                data.len()
            ),
        ));
    }
    data.truncate(expected);
    if native {
        return Ok(data);
    }
    if layout.byte_order == ByteOrder::Big && bps > 1 && layout.predictor != 3 {
        for s in data.chunks_exact_mut(bps) {
            s.reverse();
        }
    }
    match layout.predictor {
        1 => {}
        2 => undo_horizontal(&mut data, cw * chunk_spp * bps, chunk_spp, bps),
        3 => undo_float(&mut data, cw * chunk_spp * bps, chunk_spp, bps),
        other => {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("predictor {other}"),
                "Supported predictors: 1 (none), 2 (horizontal), 3 (floating point).",
            ));
        }
    }
    Ok(data)
}

/// A tile of the last tile row that holds only the rows inside the image (GDAL writes such
/// tiles; libtiff and tifffile read them): pad it with zero rows to the full tile. Tiles that
/// lack rows inside the image stay short (and are refused).
fn pad_short_tile(
    layout: &PageLayout,
    idx: u64,
    data: &mut Vec<u8>,
    row_bytes: usize,
    full: usize,
) {
    if !layout.tiled || data.len() >= full || row_bytes == 0 {
        return;
    }
    let per_plane = layout.chunks_per_plane().max(1);
    let across = u64::from(layout.chunks_across()).max(1);
    let y0 = (idx % per_plane) / across * u64::from(layout.chunk_height);
    let inside = u64::from(layout.height)
        .saturating_sub(y0)
        .min(u64::from(layout.chunk_height));
    let needed = usize::try_from(inside)
        .unwrap_or(usize::MAX)
        .saturating_mul(row_bytes);
    if data.len() >= needed {
        data.resize(full, 0);
    }
}

/// Decode a chunk whose stored samples differ from the returned ones (packed bits, half and
/// 24-bit floats, complex integers): codec, byte order and predictor on the stored samples,
/// then widening.
fn decode_coded(
    layout: &PageLayout,
    chunk: Chunk,
    off: u64,
    raw: Vec<u8>,
    out_pt: PixelType,
    coding: SampleCoding,
    expected: usize,
) -> Result<Vec<u8>> {
    let Chunk {
        idx,
        cw,
        rows,
        spp: chunk_spp,
        ..
    } = chunk;
    let n = cw * chunk_spp;
    let row_in = coding.row_bytes(out_pt, layout.bits_per_sample, n);
    let stored = row_in * rows;
    let codec_err = |e: openreadout_codecs::CodecError| {
        Error::corrupt_at(FORMAT_ID, off, format!("chunk {idx}: {e}"))
    };
    let mut data = match layout.compression {
        1 => raw,
        5 => lzw_decode(&raw, 0).map_err(codec_err)?,
        8 | 32946 => zlib_decode(&raw, stored).map_err(codec_err)?,
        32773 => packbits_decode(&raw, stored).map_err(codec_err)?,
        50000 => zstd_decode(&raw, 0).map_err(codec_err)?,
        other => {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!(
                    "{}-bit samples (sample format {}) with compression {}",
                    layout.bits_per_sample,
                    layout.sample_format,
                    compression_name(other)
                ),
                "Packed-bit, half-float, 24-bit float and complex-integer samples are decoded with no compression, LZW, deflate, PackBits or zstd.",
            ));
        }
    };
    pad_short_tile(layout, idx, &mut data, row_in, stored);
    if data.len() < stored {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            off,
            format!(
                "chunk {idx} decodes to {} bytes, expected {stored}",
                data.len()
            ),
        ));
    }
    data.truncate(stored);
    let unit = coding.unit_bytes(out_pt);
    if let SampleCoding::Packed { .. } = coding {
        if layout.fill_order == 2 {
            for b in &mut data {
                *b = b.reverse_bits();
            }
        }
    } else if layout.byte_order == ByteOrder::Big && unit > 1 && layout.predictor != 3 {
        for c in data.chunks_exact_mut(unit) {
            c.reverse();
        }
    }
    let parts = if matches!(coding, SampleCoding::ComplexInt(_)) {
        2
    } else {
        1
    };
    match (layout.predictor, coding) {
        (1, _) => {}
        (2, SampleCoding::ComplexInt(_)) => {
            undo_horizontal(&mut data, row_in, chunk_spp * parts, unit);
        }
        (3, SampleCoding::Float16 | SampleCoding::Float24) => {
            undo_float(&mut data, row_in, chunk_spp, unit);
        }
        (p, _) => {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("predictor {p} with {}-bit samples", layout.bits_per_sample),
                "Predictors are undone on whole-byte samples only (horizontal on integers, floating point on floats).",
            ));
        }
    }
    let out = widen_chunk(coding, out_pt, &data, rows, n, row_in);
    if out.len() != expected {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            off,
            format!(
                "chunk {idx}: {} samples where {expected} bytes were expected",
                out.len()
            ),
        ));
    }
    Ok(out)
}

/// Undo predictor 2 (horizontal differencing) on little-endian samples, row by row.
fn undo_horizontal(data: &mut [u8], row_bytes: usize, spp: usize, bps: usize) {
    if row_bytes == 0 {
        return;
    }
    for row in data.chunks_exact_mut(row_bytes) {
        let n = row.len() / bps;
        for i in spp..n {
            let (prev, cur) = (i - spp, i);
            match bps {
                1 => row[cur] = row[cur].wrapping_add(row[prev]),
                2 => {
                    let a = le_u16(row, prev * 2).unwrap_or(0);
                    let b = le_u16(row, cur * 2).unwrap_or(0);
                    row[cur * 2..cur * 2 + 2].copy_from_slice(&b.wrapping_add(a).to_le_bytes());
                }
                4 => {
                    let a = le_u32(row, prev * 4).unwrap_or(0);
                    let b = le_u32(row, cur * 4).unwrap_or(0);
                    row[cur * 4..cur * 4 + 4].copy_from_slice(&b.wrapping_add(a).to_le_bytes());
                }
                8 => {
                    let a = le_u64(row, prev * 8).unwrap_or(0);
                    let b = le_u64(row, cur * 8).unwrap_or(0);
                    row[cur * 8..cur * 8 + 8].copy_from_slice(&b.wrapping_add(a).to_le_bytes());
                }
                _ => {}
            }
        }
    }
}

/// Undo predictor 3 (floating point): byte-wise differencing over the row, then the byte
/// planes (most significant first) are re-interleaved into little-endian samples.
fn undo_float(data: &mut [u8], row_bytes: usize, spp: usize, bps: usize) {
    if row_bytes == 0 {
        return;
    }
    let mut tmp = vec![0u8; row_bytes];
    let nsamples = row_bytes / bps;
    for row in data.chunks_exact_mut(row_bytes) {
        for i in spp..row.len() {
            row[i] = row[i].wrapping_add(row[i - spp]);
        }
        tmp.copy_from_slice(row);
        for j in 0..nsamples {
            for k in 0..bps {
                row[j * bps + k] = tmp[(bps - 1 - k) * nsamples + j];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_bits_unpack_msb_first_with_sign() {
        // 12-bit 107, 123 packed MSB first: 0x06B 0x07B -> bytes 06 b0 7b
        let o = widen_chunk(
            SampleCoding::Packed {
                bits: 12,
                signed: false,
            },
            PixelType::Uint16,
            &[0x06, 0xb0, 0x7b],
            1,
            2,
            3,
        );
        assert_eq!(
            o,
            [107u16, 123]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>()
        );
        // 4-bit signed 0xF = -1, 0x7 = 7; rows padded to a byte (3 samples -> 2 bytes)
        let o = widen_chunk(
            SampleCoding::Packed {
                bits: 4,
                signed: true,
            },
            PixelType::Int8,
            &[0xf7, 0x80, 0x10, 0x00],
            2,
            3,
            2,
        );
        assert_eq!(o, [0xff, 7, 0xf8, 1, 0, 0]);
        // 1-bit: 0b1010_0000 -> 1 0 1
        let o = widen_chunk(
            SampleCoding::Packed {
                bits: 1,
                signed: false,
            },
            PixelType::Uint8,
            &[0b1010_0000],
            1,
            3,
            1,
        );
        assert_eq!(o, [1, 0, 1]);
    }

    #[test]
    #[allow(clippy::float_cmp)] // exact values
    fn half_and_24_bit_floats_widen_exactly() {
        assert_eq!(half_to_f32(0x56b0), 107.0);
        assert_eq!(half_to_f32(0xc000), -2.0);
        assert_eq!(half_to_f32(0x0001), 2f32.powi(-24));
        assert!(half_to_f32(0x7e00).is_nan());
        assert_eq!(float24_to_f32(0x0045_ac00), 107.0);
        assert_eq!(float24_to_f32(0x00bf_0000), -1.0);
        assert_eq!(float24_to_f32(0), 0.0);
        let o = widen_chunk(
            SampleCoding::Float24,
            PixelType::Float,
            &[0x00, 0xac, 0x45],
            1,
            1,
            3,
        );
        assert_eq!(o, 107f32.to_le_bytes());
    }

    #[test]
    fn complex_integers_widen_to_complex_floats() {
        let d: Vec<u8> = [107i16, -3].iter().flat_map(|v| v.to_le_bytes()).collect();
        let o = widen_chunk(
            SampleCoding::ComplexInt(16),
            PixelType::ComplexFloat,
            &d,
            1,
            1,
            4,
        );
        let want: Vec<u8> = [107f32, -3.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert_eq!(o, want);
    }

    #[test]
    fn horizontal_predictor_u16() {
        // samples 10, 12, 15 stored as differences 10, 2, 3
        let mut d: Vec<u8> = [10u16, 2, 3].iter().flat_map(|v| v.to_le_bytes()).collect();
        undo_horizontal(&mut d, 6, 1, 2);
        let v: Vec<u16> = d
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(v, [10, 12, 15]);
    }

    #[test]
    fn float_predictor_roundtrip() {
        // Encode two f32 samples with the predictor-3 scheme and decode them back.
        let vals = [1.5f32, -2.25];
        let n = vals.len();
        let mut planes = vec![0u8; n * 4];
        for (j, v) in vals.iter().enumerate() {
            let be = v.to_be_bytes();
            for k in 0..4 {
                planes[k * n + j] = be[k];
            }
        }
        for i in (1..planes.len()).rev() {
            planes[i] = planes[i].wrapping_sub(planes[i - 1]);
        }
        undo_float(&mut planes, n * 4, 1, 4);
        let back: Vec<f32> = planes
            .chunks(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        assert_eq!(back, vals);
    }
}
