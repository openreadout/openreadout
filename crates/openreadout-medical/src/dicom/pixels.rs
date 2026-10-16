//! Pixel data: the Image Pixel module (PS3.3 C.7.6.3), native frames (PS3.5 §8.1), the frames
//! of encapsulated pixel data (PS3.5 A.4), the decoders of each transfer syntax, and colour
//! conversion to R, G, B.

use openreadout_core::{Error, PixelType, Result};

use super::FORMAT_ID;
use super::dictionary::Compression;
use super::element::Lookup;
use super::file::{DicomFile, PixelLoc};
use super::parse::{self, Fragment, Item};
use super::tags as t;

/// The Image Pixel module of an image.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PixelDesc {
    pub(crate) rows: u32,
    pub(crate) cols: u32,
    pub(crate) spp: u32,
    pub(crate) bits_allocated: u32,
    pub(crate) bits_stored: u32,
    pub(crate) signed: bool,
    pub(crate) photometric: String,
    /// 1: colour-by-plane (RRR…GGG…BBB…); 0: interleaved.
    pub(crate) planar: u32,
    pub(crate) frames: u32,
    /// Float (7FE0,0008) or double (7FE0,0009) pixel data.
    pub(crate) float_bits: u32,
}

impl PixelDesc {
    pub(crate) fn of(ds: &Item, pixel_tag: u32) -> Result<PixelDesc> {
        let rows = ds.uint(t::ROWS).unwrap_or(0);
        let cols = ds.uint(t::COLUMNS).unwrap_or(0);
        if rows == 0 || cols == 0 {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("Rows {rows} x Columns {cols}: the image has no pixels"),
            ));
        }
        let float_bits = match pixel_tag {
            0x7FE0_0008 => 32,
            0x7FE0_0009 => 64,
            _ => 0,
        };
        let bits_allocated = if float_bits > 0 {
            float_bits
        } else {
            ds.uint(t::BITS_ALLOCATED).unwrap_or(16)
        };
        if !matches!(bits_allocated, 1 | 8 | 16 | 32 | 64) {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("Bits Allocated {bits_allocated}"),
                "DICOM pixel data has 1, 8, 16 or 32 bits allocated per sample; check the header with `openreadout check`.",
            ));
        }
        let bits_stored = ds
            .uint(t::BITS_STORED)
            .unwrap_or(bits_allocated)
            .clamp(1, bits_allocated);
        let spp = ds.uint(t::SAMPLES_PER_PIXEL).unwrap_or(1);
        if !matches!(spp, 1 | 3) {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("Samples per Pixel {spp}"),
                "Only grayscale (1) and colour (3) samples per pixel are read.",
            ));
        }
        Ok(PixelDesc {
            rows,
            cols,
            spp,
            bits_allocated,
            bits_stored,
            signed: ds.uint(t::PIXEL_REPRESENTATION) == Some(1),
            photometric: ds
                .text(t::PHOTOMETRIC_INTERPRETATION)
                .unwrap_or_else(|| {
                    if spp == 3 {
                        "RGB".into()
                    } else {
                        "MONOCHROME2".into()
                    }
                })
                .to_ascii_uppercase(),
            planar: ds.uint(t::PLANAR_CONFIGURATION).unwrap_or(0),
            frames: ds.uint(t::NUMBER_OF_FRAMES).unwrap_or(1).max(1),
            float_bits,
        })
    }

    /// The sample type of a decoded frame before any value transform: the stored type, with
    /// 1-bit samples widened to u8.
    pub(crate) fn stored_type(&self) -> PixelType {
        match (self.float_bits, self.bits_allocated, self.signed) {
            (32, _, _) => PixelType::Float,
            (64, _, _) => PixelType::Double,
            (_, 1 | 8, false) => PixelType::Uint8,
            (_, 1 | 8, true) => PixelType::Int8,
            (_, 16, false) => PixelType::Uint16,
            (_, 16, true) => PixelType::Int16,
            (_, _, false) => PixelType::Uint32,
            (_, _, true) => PixelType::Int32,
        }
    }

    /// True when frames are returned as R, G, B (colour photometric interpretations and
    /// palette colour).
    pub(crate) fn is_colour(&self) -> bool {
        self.spp == 3 || self.photometric == "PALETTE COLOR"
    }

    /// Bits of one native frame (YBR_FULL_422 stores two samples per pixel).
    pub(crate) fn native_frame_bits(&self) -> u64 {
        if self.is_native_422() {
            return u64::from(self.rows) * u64::from(self.cols) * 16;
        }
        u64::from(self.rows)
            * u64::from(self.cols)
            * u64::from(self.spp)
            * u64::from(self.bits_allocated)
    }

    /// Native 4:2:2 colour (two luminance samples share one chrominance pair).
    pub(crate) fn is_native_422(&self) -> bool {
        self.photometric == "YBR_FULL_422" && self.spp == 3 && self.bits_allocated == 8
    }

    /// Range of stored values `(min, max)` from Bits Stored and the representation.
    pub(crate) fn stored_range(&self) -> (f64, f64) {
        if self.float_bits > 0 {
            return (f64::MIN, f64::MAX);
        }
        let b = self.bits_stored.min(63);
        if self.signed {
            (-(2f64.powi(b as i32 - 1)), 2f64.powi(b as i32 - 1) - 1.0)
        } else {
            (0.0, 2f64.powi(b as i32) - 1.0)
        }
    }

    fn out_bytes(&self) -> usize {
        self.stored_type().bytes_per_sample()
    }

    /// Expected byte size of a decoded frame in the stored type.
    pub(crate) fn decoded_len(&self) -> Result<usize> {
        let n = u64::from(self.rows)
            .checked_mul(u64::from(self.cols))
            .and_then(|v| v.checked_mul(u64::from(self.spp)))
            .and_then(|v| v.checked_mul(self.out_bytes() as u64))
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "frame size overflows"))?;
        openreadout_core::limits::plane_len(
            FORMAT_ID,
            u64::from(self.cols),
            u64::from(self.rows),
            u64::from(self.spp),
            self.out_bytes() as u64,
            n.max(1 << 30),
        )
    }
}

/// The frames of encapsulated pixel data: which fragments each frame is made of.
#[derive(Debug, Clone, Default)]
pub(crate) struct FrameTable {
    pub(crate) fragments: Vec<Fragment>,
    /// Per frame, the range of fragment indices.
    pub(crate) frames: Vec<std::ops::Range<usize>>,
    pub(crate) complete: bool,
}

/// Starts of the codestreams a compressed frame begins with: JPEG and JPEG-LS SOI, a JPEG 2000
/// codestream (SOC SIZ) or JP2 signature, a JPEG XL codestream or container.
fn starts_frame(head: &[u8]) -> bool {
    head.starts_with(&[0xFF, 0xD8, 0xFF])
        || head.starts_with(&[0xFF, 0x4F, 0xFF, 0x51])
        || head.starts_with(&[0x00, 0x00, 0x00, 0x0C])
        || head.starts_with(&[0xFF, 0x0A])
}

impl FrameTable {
    /// Index the fragments of `file` and assign them to `n` frames (PS3.5 A.4): by the
    /// Extended Offset Table, one fragment per frame, the Basic Offset Table, or failing those
    /// by where codestreams start.
    pub(crate) fn build(file: &DicomFile, n: u32) -> Result<FrameTable> {
        let Some(PixelLoc::Encapsulated { offset }) = file.pixels else {
            return Err(Error::corrupt(FORMAT_ID, "pixel data is not encapsulated"));
        };
        let (bot, fragments, complete) = parse::fragments(&file.src, offset)?;
        let n = n as usize;
        let mut table = FrameTable {
            fragments,
            frames: Vec::new(),
            complete,
        };
        let f = &table.fragments;
        if f.is_empty() {
            return Err(Error::corrupt(
                FORMAT_ID,
                "encapsulated pixel data holds no fragments",
            ));
        }
        // item-tag offsets of each fragment, relative to the first fragment's item tag
        let base = f[0].offset - 8;
        let rel: Vec<u64> = f.iter().map(|x| x.offset - 8 - base).collect();
        let by_offsets = |offs: &[u64]| -> Option<Vec<std::ops::Range<usize>>> {
            if offs.len() != n || offs.first() != Some(&0) {
                return None;
            }
            let mut starts = Vec::with_capacity(n);
            for o in offs {
                starts.push(rel.binary_search(o).ok()?);
            }
            let mut out = Vec::with_capacity(n);
            for (i, s) in starts.iter().enumerate() {
                let e = starts.get(i + 1).copied().unwrap_or(f.len());
                if e <= *s {
                    return None;
                }
                out.push(*s..e);
            }
            Some(out)
        };
        let eot: Option<Vec<u64>> = file
            .ds
            .get(t::EXTENDED_OFFSET_TABLE)
            .and_then(|e| file.read_value(e).ok())
            .map(|b| {
                b.as_chunks::<8>()
                    .0
                    .iter()
                    .map(|c| u64::from_le_bytes(*c))
                    .collect()
            });
        if let Some(r) = eot.as_deref().and_then(by_offsets) {
            table.frames = r;
        } else if n == 1 {
            table.frames = vec![0..f.len()];
        } else if f.len() == n {
            table.frames = (0..n).map(|i| i..i + 1).collect();
        } else if let Some(r) = by_offsets(
            &bot.as_chunks::<4>()
                .0
                .iter()
                .map(|c| u64::from(u32::from_le_bytes(*c)))
                .collect::<Vec<_>>(),
        ) {
            table.frames = r;
        } else {
            // no usable offset table: a frame starts at each fragment that begins a codestream
            let mut starts = Vec::new();
            for (i, fr) in f.iter().enumerate() {
                let head = file.src.read(fr.offset, fr.len.min(4))?;
                if i == 0 || starts_frame(&head) {
                    starts.push(i);
                }
            }
            if starts.len() != n {
                return Err(Error::corrupt(
                    FORMAT_ID,
                    format!(
                        "{} fragments of pixel data cannot be divided into {n} frames: no offset table and {} codestream starts",
                        f.len(),
                        starts.len()
                    ),
                ));
            }
            table.frames = starts
                .iter()
                .enumerate()
                .map(|(i, s)| *s..starts.get(i + 1).copied().unwrap_or(f.len()))
                .collect();
        }
        Ok(table)
    }

    /// The bytes of frame `i` (its fragments concatenated).
    pub(crate) fn frame_bytes(&self, file: &DicomFile, i: u32) -> Result<Vec<u8>> {
        let r = self.frames.get(i as usize).ok_or_else(|| {
            Error::corrupt(
                FORMAT_ID,
                format!("frame {i} is not in the pixel data ({} frames found)", self.frames.len()),
            )
        })?;
        let mut out = Vec::new();
        for fr in &self.fragments[r.clone()] {
            out.extend(file.src.read(fr.offset, fr.len)?);
        }
        Ok(out)
    }
}

/// A decoded frame in the stored sample type, little-endian; colour frames are interleaved
/// R, G, B (`spp == 3`).
#[derive(Debug, Clone)]
pub(crate) struct Decoded {
    pub(crate) data: Vec<u8>,
    pub(crate) spp: u32,
    pub(crate) pixel_type: PixelType,
}

/// Decode frame `frame` of `file`.
pub(crate) fn decode_frame(
    file: &DicomFile,
    desc: &PixelDesc,
    table: &mut Option<FrameTable>,
    frame: u32,
) -> Result<Decoded> {
    let loc = file.pixels.ok_or_else(|| {
        Error::corrupt(
            FORMAT_ID,
            format!("{} has no pixel data", file.path.display()),
        )
    })?;
    let want = desc.decoded_len()?;
    let raw = match loc {
        PixelLoc::Native { offset, len } => {
            let bits = desc.native_frame_bits();
            let start_bit = bits
                .checked_mul(u64::from(frame))
                .ok_or_else(|| Error::corrupt(FORMAT_ID, "frame offset overflows"))?;
            let start = offset + start_bit / 8;
            let nbytes = (start_bit % 8 + bits).div_ceil(8);
            let avail = (offset + len).min(file.src.len());
            if start + nbytes > avail {
                return Err(Error::corrupt_at(
                    FORMAT_ID,
                    start,
                    format!(
                        "frame {frame} needs {nbytes} bytes at offset {start} but the pixel data ends at {avail} (truncated?)"
                    ),
                ));
            }
            // 8-bit samples in an OW value of a big-endian file are byte-swapped in pairs
            let swap = !file.syntax.little && &file.pixel_vr == b"OW" && desc.bits_allocated == 8;
            let even = (swap && nbytes % 2 == 1 && start + nbytes < avail) as u64;
            let mut b = file.src.read(start, nbytes + even)?;
            if swap {
                for c in b.chunks_exact_mut(2) {
                    c.swap(0, 1);
                }
            }
            native(&b, desc, (start_bit % 8) as u32, file.syntax.little)?
        }
        PixelLoc::Encapsulated { .. } => {
            if table.is_none() {
                *table = Some(FrameTable::build(file, desc.frames)?);
            }
            let tb = table.as_ref().ok_or_else(|| Error::Other("frame table".into()))?;
            let bytes = tb.frame_bytes(file, frame)?;
            compressed(&bytes, desc, file.syntax.compression, want)?
        }
    };
    let mut d = raw;
    if d.data.len() < want {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!(
                "frame {frame} decoded to {} bytes, expected {want}",
                d.data.len()
            ),
        ));
    }
    d.data.truncate(want);
    fix_unused_bits(&mut d, desc);
    let codec_converted = matches!(loc, PixelLoc::Encapsulated { .. })
        && !matches!(
            file.syntax.compression,
            Compression::Rle | Compression::EncapsulatedNative
        );
    if !codec_converted {
        to_rgb(&mut d, desc);
    }
    Ok(d)
}

/// Native samples (PS3.5 §8.1): byte-swapped from big endian, 1-bit samples unpacked
/// (least significant bit first), colour-by-plane interleaved.
fn native(b: &[u8], desc: &PixelDesc, bit0: u32, little: bool) -> Result<Decoded> {
    let ty = desc.stored_type();
    let n = desc.rows as usize * desc.cols as usize * desc.spp as usize;
    if desc.is_native_422() {
        return Ok(Decoded {
            data: expand_422(b, n / 3),
            spp: 3,
            pixel_type: ty,
        });
    }
    let mut data = if desc.bits_allocated == 1 {
        (0..n)
            .map(|i| {
                let bit = i + bit0 as usize;
                (b.get(bit / 8).copied().unwrap_or(0) >> (bit % 8)) & 1
            })
            .collect()
    } else {
        b.to_vec()
    };
    let w = ty.bytes_per_sample();
    if !little && w > 1 {
        for c in data.chunks_exact_mut(w) {
            c.reverse();
        }
    }
    if desc.spp == 3 && desc.planar == 1 {
        data = interleave(&data, n / 3, w);
    }
    Ok(Decoded {
        data,
        spp: desc.spp,
        pixel_type: ty,
    })
}

/// RRR…GGG…BBB… → RGBRGB… for `px` pixels of `w`-byte samples.
fn interleave(planar: &[u8], px: usize, w: usize) -> Vec<u8> {
    let mut out = vec![0u8; planar.len()];
    for p in 0..px {
        for c in 0..3 {
            let s = (c * px + p) * w;
            let d = (p * 3 + c) * w;
            if let (Some(src), Some(dst)) = (planar.get(s..s + w), out.get_mut(d..d + w)) {
                dst.copy_from_slice(src);
            }
        }
    }
    out
}

fn codec_err(e: openreadout_codecs::CodecError) -> Error {
    match e {
        openreadout_codecs::CodecError::Unsupported { codec, detail } => Error::unsupported(
            FORMAT_ID,
            format!("{codec}: {detail}"),
            "This compressed frame uses a codec feature OpenReadout does not decode; the header is still readable with `openreadout info`.",
        ),
        other => Error::corrupt(FORMAT_ID, other.to_string()),
    }
}

/// Decode a compressed frame to the stored type.
fn compressed(
    bytes: &[u8],
    desc: &PixelDesc,
    compression: Compression,
    want: usize,
) -> Result<Decoded> {
    let ty = desc.stored_type();
    let limit = want.saturating_mul(2).max(1 << 16);
    let raster = match compression {
        Compression::EncapsulatedNative => {
            return native(bytes, desc, 0, true);
        }
        Compression::Rle => return rle(bytes, desc),
        Compression::DeflateFrame => {
            let need = usize::try_from(desc.native_frame_bits().div_ceil(8)).unwrap_or(usize::MAX);
            let raw = miniz_oxide::inflate::decompress_to_vec_with_limit(bytes, need.saturating_add(1))
                .or_else(|_| miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(bytes, need.saturating_add(1)))
                .map_err(|e| Error::corrupt(FORMAT_ID, format!("deflated frame does not inflate ({:?})", e.status)))?;
            return native(&raw, desc, 0, true);
        }
        Compression::Jpeg => jpeg(bytes, desc, limit),
        Compression::JpegLs => openreadout_codecs::jpegls_decode_limited(bytes, limit),
        Compression::Jpeg2000 => openreadout_codecs::jpeg2000_decode_limited(bytes, limit),
        Compression::JpegXl => openreadout_codecs::jpegxl_decode_limited(bytes, limit),
        Compression::Native | Compression::Other => {
            return Err(Error::unsupported(
                FORMAT_ID,
                "this transfer syntax's pixel compression",
                "Video (MPEG-2, MPEG-4, HEVC) and unknown transfer syntaxes are not decoded; `openreadout info` still reads the header.",
            ));
        }
    }
    .map_err(codec_err)?;
    if (raster.width, raster.height) != (desc.cols, desc.rows) {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!(
                "the codestream is {} x {} but the header says {} x {}",
                raster.width, raster.height, desc.cols, desc.rows
            ),
        ));
    }
    let spp = raster.channels;
    if spp != desc.spp {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!(
                "the codestream has {spp} components but Samples per Pixel is {}",
                desc.spp
            ),
        ));
    }
    // widen or narrow to the stored container
    let w = ty.bytes_per_sample();
    let data = match (raster.bits_per_sample, w) {
        (8, 1) | (16, 2) => raster.data,
        (8, 2) => raster
            .data
            .iter()
            .flat_map(|&v| u16::from(v).to_le_bytes())
            .collect(),
        (16, 1) => raster
            .data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c).min(255) as u8)
            .collect(),
        (b, 4) if b <= 16 => {
            let src = raster.data;
            if b == 8 {
                src.iter()
                    .flat_map(|&v| u32::from(v).to_le_bytes())
                    .collect()
            } else {
                src.as_chunks::<2>()
                    .0
                    .iter()
                    .flat_map(|c| u32::from(u16::from_le_bytes(*c)).to_le_bytes())
                    .collect()
            }
        }
        (b, _) => {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("{b}-bit decoded samples in a {}-bit container", w * 8),
                "The codestream's precision does not fit the header's Bits Allocated.",
            ));
        }
    };
    Ok(Decoded {
        data,
        spp,
        pixel_type: ty,
    })
}

/// A JPEG frame with the colour coding DICOM specifies (PS3.5 §8.2.1): the stream's own
/// markers (JFIF or an Adobe APP14 segment) when it has them, otherwise R, G, B as coded for
/// Photometric Interpretation RGB and Y, Cb, Cr converted to R, G, B for the YBR ones.
/// Lossless streams have no colour transform.
fn jpeg(
    bytes: &[u8],
    desc: &PixelDesc,
    limit: usize,
) -> std::result::Result<openreadout_codecs::Raster, openreadout_codecs::CodecError> {
    use openreadout_codecs::JpegColor;
    let Ok(m) = openreadout_codecs::jpeg_markers(bytes, None) else {
        return openreadout_codecs::jpeg_decode_limited(bytes, limit);
    };
    if m.component_ids.len() != 3 {
        return openreadout_codecs::jpeg_decode_limited(bytes, limit);
    }
    let color = if m.process == 3 {
        JpegColor::AsCoded
    } else if m.jfif {
        JpegColor::ToRgb
    } else if let Some(t) = m.adobe_transform {
        if t == 0 {
            JpegColor::AsCoded
        } else {
            JpegColor::ToRgb
        }
    } else if desc.photometric == "RGB" {
        JpegColor::AsCoded
    } else {
        JpegColor::ToRgb
    };
    openreadout_codecs::jpeg_decode_tiff_limited(bytes, None, color, limit)
}

/// RLE Lossless (PS3.5 Annex G): a 64-byte header of up to 15 segment offsets, one PackBits
/// segment per byte plane, most significant byte first, sample by sample.
fn rle(b: &[u8], desc: &PixelDesc) -> Result<Decoded> {
    if b.len() < 64 {
        return Err(Error::corrupt(FORMAT_ID, "RLE frame shorter than its 64-byte header"));
    }
    let word = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
    let nseg = word(0);
    let bytes = (desc.bits_allocated as usize).div_ceil(8);
    let spp = desc.spp as usize;
    if nseg != bytes * spp || nseg > 15 {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!(
                "RLE header lists {nseg} segments; {spp} sample(s) of {bytes} byte(s) need {}",
                bytes * spp
            ),
        ));
    }
    let px = desc.rows as usize * desc.cols as usize;
    if desc.bits_allocated == 1 {
        // one segment with a byte (0 or 1) per pixel, as the reference readers decode it
        let start = word(4);
        let seg = b.get(start..).unwrap_or(&[]);
        let data = openreadout_codecs::packbits_decode(seg, px).map_err(codec_err)?;
        return Ok(Decoded {
            data,
            spp: 1,
            pixel_type: PixelType::Uint8,
        });
    }
    let mut out = vec![0u8; px * spp * bytes];
    for s in 0..nseg {
        let start = word(4 + 4 * s);
        let end = if s + 1 < nseg {
            word(8 + 4 * s)
        } else {
            b.len()
        };
        let seg = b.get(start..end.max(start).min(b.len())).ok_or_else(|| {
            Error::corrupt(FORMAT_ID, format!("RLE segment {s} offset {start} is outside the frame"))
        })?;
        let plane = openreadout_codecs::packbits_decode(seg, px).map_err(codec_err)?;
        let (sample, byte) = (s / bytes, s % bytes);
        // byte 0 is the most significant: in little-endian output it goes last
        let pos = bytes - 1 - byte;
        for (p, v) in plane.iter().enumerate() {
            if let Some(o) = out.get_mut((p * spp + sample) * bytes + pos) {
                *o = *v;
            }
        }
    }
    Ok(Decoded {
        data: out,
        spp: desc.spp,
        pixel_type: desc.stored_type(),
    })
}

/// Clear the bits above Bits Stored, sign-extending signed samples (PS3.5 §8.1.1), as the
/// reference readers do.
fn fix_unused_bits(d: &mut Decoded, desc: &PixelDesc) {
    let bits = desc.bits_stored;
    let w = d.pixel_type.bytes_per_sample() as u32;
    if desc.float_bits > 0 || bits >= w * 8 || desc.bits_allocated == 1 {
        return;
    }
    let mask: u64 = (1u64 << bits) - 1;
    let sign = 1u64 << (bits - 1);
    for c in d.data.chunks_exact_mut(w as usize) {
        let mut v: u64 = 0;
        for (i, b) in c.iter().enumerate() {
            v |= u64::from(*b) << (8 * i);
        }
        v &= mask;
        if desc.signed && v & sign != 0 {
            v |= !mask;
        }
        for (i, b) in c.iter_mut().enumerate() {
            *b = (v >> (8 * i)) as u8;
        }
    }
}

/// YBR_FULL and YBR_FULL_422 frames of native and RLE pixel data to R, G, B (PS3.3
/// C.7.6.3.1.2, full-range ITU-R BT.601); the JPEG and JPEG 2000 decoders convert their own.
fn to_rgb(d: &mut Decoded, desc: &PixelDesc) {
    if d.spp != 3 || d.pixel_type != PixelType::Uint8 {
        return;
    }
    if !matches!(desc.photometric.as_str(), "YBR_FULL" | "YBR_FULL_422") {
        return;
    }
    for px in d.data.chunks_exact_mut(3) {
        let (y, cb, cr) = (
            f64::from(px[0]),
            f64::from(px[1]) - 128.0,
            f64::from(px[2]) - 128.0,
        );
        let r = y + 1.402 * cr;
        let g = y - 0.344_136 * cb - 0.714_136 * cr;
        let b = y + 1.772 * cb;
        px[0] = r.round().clamp(0.0, 255.0) as u8;
        px[1] = g.round().clamp(0.0, 255.0) as u8;
        px[2] = b.round().clamp(0.0, 255.0) as u8;
    }
}

/// Native YBR_FULL_422 (PS3.3 C.7.6.3.1.2): each pair of pixels is stored as Y1 Y2 Cb Cr;
/// expanded to Y Cb Cr per pixel.
fn expand_422(b: &[u8], px: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(px * 3);
    for p in b.chunks(4) {
        let (y1, y2, cb, cr) = (
            p.first().copied().unwrap_or(0),
            p.get(1).copied().unwrap_or(0),
            p.get(2).copied().unwrap_or(128),
            p.get(3).copied().unwrap_or(128),
        );
        out.extend_from_slice(&[y1, cb, cr]);
        if out.len() < px * 3 {
            out.extend_from_slice(&[y2, cb, cr]);
        }
    }
    out.resize(px * 3, 0);
    out
}
