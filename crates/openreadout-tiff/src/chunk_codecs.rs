//! Chunks coded with WebP (50001), JPEG XL (50002, 52546), LERC (34887) and old-style JPEG (6).
//! See `docs/formats/tiff.md` § Other compressions.
//!
//! WebP, JPEG XL and LERC chunks are self-contained images whose decoders return samples, so
//! they skip the byte swap and predictor. Old-style JPEG chunks (TIFF 6.0 section 22, "tables"
//! form) hold only entropy-coded data: the quantization and Huffman tables live elsewhere in the
//! file (one per component, tags 519–521) and the frame and scan headers are implied by the
//! page, so we rebuild a complete JPEG stream per chunk and decode that.

use openreadout_codecs::{
    CodecError, JpegColor, Raster, jpeg_decode_tiff_limited, jpegxl_decode_limited,
    lerc_decode_limited, webp_decode_limited, zlib_decode, zstd_decode,
};
use openreadout_core::{Error, Result};

use crate::FORMAT_ID;
use crate::container::{ByteSource, FieldValue, Ifd};
use crate::decode::{Chunk, PageLayout};

const LERC_PARAMETERS: u16 = 50674;
const JPEG_PROC: u16 = 512;
const JPEG_INTERCHANGE_FORMAT: u16 = 513;
const JPEG_RESTART_INTERVAL: u16 = 515;
const JPEG_Q_TABLES: u16 = 519;
const JPEG_DC_TABLES: u16 = 520;
const JPEG_AC_TABLES: u16 = 521;
const YCBCR_COEFFICIENTS: u16 = 529;
const YCBCR_SUBSAMPLING: u16 = 530;
const REFERENCE_BLACK_WHITE: u16 = 532;

/// Codec-specific tags of a page (only the ones the codecs below need).
#[derive(Debug, Clone, Default)]
pub(crate) struct CodecTags {
    /// LercParameters: LERC version, then the extra compression (0 none, 1 deflate, 2 zstd).
    lerc: Vec<u64>,
    /// Old-style JPEG process (1 baseline, 14 lossless).
    jpeg_proc: Option<u64>,
    jpeg_interchange: Option<u64>,
    restart_interval: Option<u64>,
    q_tables: Vec<u64>,
    dc_tables: Vec<u64>,
    ac_tables: Vec<u64>,
    subsampling: Option<(u64, u64)>,
    /// YCbCrCoefficients (luma red, green, blue).
    luma: Vec<f64>,
    /// ReferenceBlackWhite (Y, Cb, Cr footroom/headroom pairs).
    ref_bw: Vec<f64>,
}

impl CodecTags {
    pub(crate) fn from_ifd(ifd: &Ifd) -> Self {
        let sub = ifd.uints(YCBCR_SUBSAMPLING).unwrap_or_default();
        CodecTags {
            lerc: ifd.uints(LERC_PARAMETERS).unwrap_or_default(),
            jpeg_proc: ifd.uint(JPEG_PROC),
            jpeg_interchange: ifd.uint(JPEG_INTERCHANGE_FORMAT),
            restart_interval: ifd.uint(JPEG_RESTART_INTERVAL),
            q_tables: ifd.uints(JPEG_Q_TABLES).unwrap_or_default(),
            dc_tables: ifd.uints(JPEG_DC_TABLES).unwrap_or_default(),
            ac_tables: ifd.uints(JPEG_AC_TABLES).unwrap_or_default(),
            subsampling: match sub[..] {
                [h, v, ..] => Some((h, v)),
                _ => None,
            },
            luma: floats(ifd, YCBCR_COEFFICIENTS),
            ref_bw: floats(ifd, REFERENCE_BLACK_WHITE),
        }
    }
}

fn floats(ifd: &Ifd, tag: u16) -> Vec<f64> {
    match ifd.field(tag).map(|f| &f.value) {
        Some(FieldValue::Float(v)) => v.clone(),
        Some(FieldValue::Unsigned(v)) => v.iter().map(|&x| x as f64).collect(),
        _ => Vec::new(),
    }
}

/// What a page needs read from the file once before its chunks are decoded.
#[derive(Debug, Default)]
pub(crate) struct Prepared {
    /// Old-style JPEG: DQT, DHT (and DRI) segments rebuilt from tags 515 and 519–521.
    old_jpeg_tables: Option<Vec<u8>>,
}

/// Read what [`decode_chunk`] needs from outside the chunks (old-style JPEG tables).
pub(crate) fn prepare(src: &mut ByteSource, layout: &PageLayout) -> Result<Prepared> {
    if layout.compression != 6 {
        return Ok(Prepared::default());
    }
    let t = &layout.codec_tags;
    if t.jpeg_proc.is_some_and(|p| p != 1) {
        return Err(Error::unsupported(
            FORMAT_ID,
            format!(
                "old-style JPEG process {} (lossless)",
                t.jpeg_proc.unwrap_or(0)
            ),
            "Only baseline (process 1) old-style JPEG is decoded.",
        ));
    }
    let comps = if layout.planar == 2 {
        1
    } else {
        usize::from(layout.samples_per_pixel)
    };
    if t.q_tables.is_empty() || t.dc_tables.is_empty() || t.ac_tables.is_empty() {
        let hint = if t.jpeg_interchange.is_some() {
            "This old-style JPEG page stores one JPEGInterchangeFormat stream instead of per-component tables; that layout is not decoded. Re-save the file with new-style JPEG (compression 7)."
        } else {
            "Re-save the file with new-style JPEG (compression 7) or a lossless codec."
        };
        return Err(Error::unsupported(
            FORMAT_ID,
            "old-style JPEG (compression 6) without JPEGQTables/JPEGDCTables/JPEGACTables",
            hint,
        ));
    }
    if !(1..=4).contains(&comps)
        || [&t.q_tables, &t.dc_tables, &t.ac_tables]
            .iter()
            .any(|v| v.len() < comps)
    {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!(
                "old-style JPEG page with {comps} components lists {} quantization, {} DC and {} AC tables",
                t.q_tables.len(),
                t.dc_tables.len(),
                t.ac_tables.len()
            ),
        ));
    }
    let mut seg = Vec::new();
    for (i, &off) in t.q_tables.iter().take(comps).enumerate() {
        let q = read_table(src, off, 64)?;
        seg.extend_from_slice(&[0xFF, 0xDB, 0, 67, i as u8]);
        seg.extend_from_slice(&q);
    }
    for (class, offs) in [(0u8, &t.dc_tables), (1, &t.ac_tables)] {
        for (i, &off) in offs.iter().take(comps).enumerate() {
            let counts = read_table(src, off, 16)?;
            let n: usize = counts.iter().map(|&c| usize::from(c)).sum();
            if n == 0 || n > 256 {
                return Err(Error::corrupt_at(
                    FORMAT_ID,
                    off,
                    format!("old-style JPEG Huffman table with {n} codes"),
                ));
            }
            let values = read_table(src, off + 16, n as u64)?;
            let len = 2 + 1 + 16 + n;
            seg.extend_from_slice(&[
                0xFF,
                0xC4,
                (len >> 8) as u8,
                len as u8,
                (class << 4) | i as u8,
            ]);
            seg.extend_from_slice(&counts);
            seg.extend_from_slice(&values);
        }
    }
    if let Some(ri) = t.restart_interval.filter(|&r| r > 0) {
        let ri = u16::try_from(ri).unwrap_or(u16::MAX);
        seg.extend_from_slice(&[0xFF, 0xDD, 0, 4]);
        seg.extend_from_slice(&ri.to_be_bytes());
    }
    Ok(Prepared {
        old_jpeg_tables: Some(seg),
    })
}

fn read_table(src: &mut ByteSource, off: u64, len: u64) -> Result<Vec<u8>> {
    if off == 0 || off.checked_add(len).is_none_or(|e| e > src.len) {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            off,
            format!(
                "old-style JPEG table ({len} bytes at offset {off}) lies outside the file ({} bytes)",
                src.len
            ),
        ));
    }
    src.read_at(off, len)
}

/// True for the compressions this module decodes.
pub(crate) fn handles(compression: u16) -> bool {
    matches!(compression, 6 | 34887 | 50001 | 50002 | 52546)
}

/// Decode one chunk to `cw × rows × chunk_spp` little-endian samples (`expected` bytes).
pub(crate) fn decode_chunk(
    layout: &PageLayout,
    prepared: &Prepared,
    chunk: Chunk,
    off: u64,
    raw: &[u8],
    expected: usize,
) -> Result<Vec<u8>> {
    let Chunk {
        idx,
        spp: chunk_spp,
        ..
    } = chunk;
    let name = crate::decode::compression_name(layout.compression);
    let codec_err = |e: CodecError| match e {
        CodecError::Unsupported { detail, .. } => Error::unsupported(
            FORMAT_ID,
            format!("{name} chunk {idx}: {detail}"),
            "This codestream uses a feature the decoder does not implement.",
        ),
        CodecError::NotCompiled { .. } => Error::unsupported(
            FORMAT_ID,
            format!("{name} chunks"),
            "LERC decoding is off in this build: the available pure-Rust LERC decoder is not robust against malformed files. Convert the file with GDAL (gdal_translate -co COMPRESS=DEFLATE) to read it.",
        ),
        e => Error::corrupt_at(FORMAT_ID, off, format!("chunk {idx}: {e}")),
    };
    if layout.predictor != 1 && layout.compression != 6 {
        return Err(Error::unsupported(
            FORMAT_ID,
            format!("predictor {} with {name}", layout.predictor),
            "WebP, JPEG XL and LERC chunks are decoded without a predictor.",
        ));
    }
    // A chunk may be coded a little larger than its geometry (and is cropped); anything much
    // larger is refused before decoding.
    let cap = expected.saturating_mul(4).max(1 << 20);
    let r = match layout.compression {
        50001 => {
            let mut r = webp_decode_limited(raw, cap).map_err(codec_err)?;
            // A lossless WebP may carry an (opaque) alpha channel the page does not have.
            if r.channels == 3 && chunk_spp == 4 {
                // ... or none, when it is fully opaque, though the page has four samples.
                r.data = r
                    .data
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .flat_map(|p| [p[0], p[1], p[2], 255])
                    .collect();
                r.channels = 4;
            }
            if r.channels == 4 && chunk_spp == 3 {
                r.data = r
                    .data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|p| [p[0], p[1], p[2]])
                    .collect();
                r.channels = 3;
            }
            r
        }
        50002 | 52546 => jpegxl_decode_limited(raw, cap).map_err(codec_err)?,
        34887 => {
            let blob = match layout.codec_tags.lerc.get(1).copied().unwrap_or(0) {
                0 => std::borrow::Cow::Borrowed(raw),
                1 => zlib_decode(raw, 0).map_err(codec_err)?.into(),
                2 => zstd_decode(raw, 0).map_err(codec_err)?.into(),
                other => {
                    return Err(Error::unsupported(
                        FORMAT_ID,
                        format!("LERC chunk {idx}: additional compression {other}"),
                        "LERC chunks wrapped in deflate (1) or zstd (2), or not at all (0), are decoded.",
                    ));
                }
            };
            lerc_decode_limited(&blob, cap).map_err(codec_err)?
        }
        6 => old_jpeg(layout, prepared, raw, chunk, cap).map_err(codec_err)?,
        other => {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("TIFF compression {other}"),
                "Not handled here.",
            ));
        }
    };
    fit(layout, chunk, off, r, expected)
}

#[allow(clippy::many_single_char_names)] // frame header fields (w, h, n; h, v sampling factors) as in ITU T.81
/// Rebuild SOI, tables, SOF1, SOS around an old-style JPEG chunk's entropy-coded data. A chunk
/// that is already a whole JPEG stream (starts with SOI) is decoded as is.
fn old_jpeg(
    layout: &PageLayout,
    prepared: &Prepared,
    raw: &[u8],
    chunk: Chunk,
    cap: usize,
) -> std::result::Result<Raster, CodecError> {
    let Chunk {
        cw,
        rows,
        spp: chunk_spp,
        ..
    } = chunk;
    let color = if layout.photometric == 6 {
        JpegColor::ToRgb
    } else {
        JpegColor::AsCoded
    };
    if raw.starts_with(&[0xFF, 0xD8]) {
        return jpeg_decode_tiff_limited(raw, None, color, cap);
    }
    let bad = |d: &str| CodecError::Decode {
        codec: "old-jpeg",
        detail: d.into(),
    };
    let tables = prepared
        .old_jpeg_tables
        .as_deref()
        .ok_or_else(|| bad("missing tables"))?;
    let (w, h) = (
        u16::try_from(cw).map_err(|_| bad("chunk wider than 65535"))?,
        u16::try_from(rows).map_err(|_| bad("chunk taller than 65535"))?,
    );
    let n = u8::try_from(chunk_spp).map_err(|_| bad("too many components"))?;
    // Y is sampled at YCbCrSubSampling (default 2×2) relative to Cb, Cr; others 1×1.
    let (sh, sv) = if layout.photometric == 6 && chunk_spp == 3 {
        let (h, v) = layout.codec_tags.subsampling.unwrap_or((2, 2));
        (h.clamp(1, 4) as u8, v.clamp(1, 4) as u8)
    } else {
        (1, 1)
    };
    let mut s = Vec::with_capacity(raw.len() + tables.len() + 64);
    s.extend_from_slice(&[0xFF, 0xD8]);
    s.extend_from_slice(tables);
    let sof_len = 8 + 3 * u16::from(n);
    s.extend_from_slice(&[0xFF, 0xC1]);
    s.extend_from_slice(&sof_len.to_be_bytes());
    s.push(8);
    s.extend_from_slice(&h.to_be_bytes());
    s.extend_from_slice(&w.to_be_bytes());
    s.push(n);
    for c in 0..n {
        let samp = if c == 0 { (sh << 4) | sv } else { 0x11 };
        s.extend_from_slice(&[c + 1, samp, c]);
    }
    let sos_len = 6 + 2 * u16::from(n);
    s.extend_from_slice(&[0xFF, 0xDA]);
    s.extend_from_slice(&sos_len.to_be_bytes());
    s.push(n);
    for c in 0..n {
        s.extend_from_slice(&[c + 1, (c << 4) | c]);
    }
    s.extend_from_slice(&[0, 63, 0]);
    s.extend_from_slice(raw);
    if !raw.ends_with(&[0xFF, 0xD9]) {
        s.extend_from_slice(&[0xFF, 0xD9]);
    }
    if color == JpegColor::AsCoded || chunk_spp != 3 {
        return jpeg_decode_tiff_limited(&s, None, color, cap);
    }
    // The page's YCbCr coding (TIFF 6.0 section 21): ReferenceBlackWhite and
    // YCbCrCoefficients apply, as libtiff applies them, rather than JFIF's full-range BT.601.
    let mut r = jpeg_decode_tiff_limited(&s, None, JpegColor::AsCoded, cap)?;
    if r.channels == 3 && r.bits_per_sample == 8 {
        tiff_ycbcr_to_rgb(&mut r.data, &layout.codec_tags);
    }
    Ok(r)
}

/// TIFF 6.0 section 21 YCbCr to RGB on 8-bit interleaved samples: each component is first
/// scaled by its ReferenceBlackWhite pair (default 0/255 for Y, 128/255 for Cb and Cr), then
/// converted with the YCbCrCoefficients (default 0.299, 0.587, 0.114).
#[allow(clippy::many_single_char_names)] // r, g, b, y, cb, cr as in the TIFF 6.0 equations
fn tiff_ycbcr_to_rgb(data: &mut [u8], t: &CodecTags) {
    let (lr, lg, lb) = match t.luma[..] {
        [red, green, blue, ..] if green > 0.0 => (red, green, blue),
        _ => (0.299, 0.587, 0.114),
    };
    let distinct = |lo: f64, hi: f64| (hi - lo).abs() > f64::EPSILON;
    let rbw = match t.ref_bw[..] {
        [y0, y1, cb0, cb1, cr0, cr1, ..]
            if distinct(y0, y1) && distinct(cb0, cb1) && distinct(cr0, cr1) =>
        {
            [y0, y1, cb0, cb1, cr0, cr1]
        }
        _ => [0.0, 255.0, 128.0, 255.0, 128.0, 255.0],
    };
    for px in data.as_chunks_mut::<3>().0 {
        let y = (f64::from(px[0]) - rbw[0]) * 255.0 / (rbw[1] - rbw[0]);
        let cb = (f64::from(px[1]) - rbw[2]) * 127.0 / (rbw[3] - rbw[2]);
        let cr = (f64::from(px[2]) - rbw[4]) * 127.0 / (rbw[5] - rbw[4]);
        let r = cr * (2.0 - 2.0 * lr) + y;
        let b = cb * (2.0 - 2.0 * lb) + y;
        let g = (y - lb * b - lr * r) / lg;
        for (d, v) in px.iter_mut().zip([r, g, b]) {
            *d = v.round().clamp(0.0, 255.0) as u8;
        }
    }
}

/// Check a decoded raster against the page and re-grid it into the chunk's geometry.
fn fit(layout: &PageLayout, chunk: Chunk, off: u64, r: Raster, expected: usize) -> Result<Vec<u8>> {
    let Chunk {
        idx,
        cw,
        rows,
        spp: chunk_spp,
        bps,
    } = chunk;
    let sample_bytes = (r.bits_per_sample as usize).div_ceil(8);
    let float_page = layout.sample_format == 3;
    if r.channels as usize != chunk_spp || sample_bytes != bps || r.float != float_page {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            off,
            format!(
                "chunk {idx}: decodes to {} {}-bit {} samples per pixel, the page declares {chunk_spp} {}-bit {}",
                r.channels,
                r.bits_per_sample,
                if r.float { "float" } else { "integer" },
                layout.bits_per_sample,
                if float_page { "float" } else { "integer" },
            ),
        ));
    }
    let (jw, jh) = (r.width as usize, r.height as usize);
    if jw == cw && jh >= rows {
        let mut d = r.data;
        d.truncate(expected);
        return Ok(d);
    }
    let px = chunk_spp * bps;
    let mut d = vec![0u8; expected];
    let (row_out, row_in) = (cw * px, jw * px);
    let n = row_out.min(row_in);
    for y in 0..rows.min(jh) {
        d[y * row_out..y * row_out + n].copy_from_slice(&r.data[y * row_in..y * row_in + n]);
    }
    Ok(d)
}
