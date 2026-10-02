//! JPEG 2000 tiles and strips (compression 33003, 33004, 33005 and 34712). See
//! `docs/formats/tiff.md` § JPEG 2000.
//!
//! Each chunk is a raw JPEG 2000 codestream. Aperio writes two flavours: 33005 (Kakadu) codes
//! R, G, B with the codestream's own irreversible colour transform, which the decoder undoes;
//! 33003 (Matrox) codes Y, Cb, Cr without one (chroma usually at half horizontal resolution),
//! which we convert here the way OpenJPEG's `sycc*_to_rgb` does (chroma replicated, JFIF
//! coefficients, truncation toward zero), so the samples match tifffile + imagecodecs.

use openreadout_codecs::{CodecError, jpeg2000_decode_limited};
use openreadout_core::{Error, Result};

use crate::FORMAT_ID;
use crate::decode::{Chunk, PageLayout};

/// Decode one JPEG 2000 chunk to `cw × rows × chunk_spp` little-endian samples (`expected`
/// bytes).
pub(crate) fn decode_chunk(
    layout: &PageLayout,
    chunk: Chunk,
    off: u64,
    raw: &[u8],
    expected: usize,
) -> Result<Vec<u8>> {
    let Chunk {
        idx,
        cw,
        rows,
        spp: chunk_spp,
        bps,
    } = chunk;
    let codec_err = |e: CodecError| match e {
        CodecError::Unsupported { detail, .. } => Error::unsupported(
            FORMAT_ID,
            format!("JPEG 2000 chunk {idx}: {detail}"),
            "Unsigned 8/16-bit JPEG 2000 codestreams with up to 16 components are decoded; HTJ2K and Part 2 extensions are not.",
        ),
        e => Error::corrupt_at(FORMAT_ID, off, format!("chunk {idx}: {e}")),
    };
    // A codestream a little larger than the chunk is tolerated (and cropped); anything much
    // larger is refused before decoding (2 bytes per sample in the limit).
    let cap = expected.saturating_mul(4).max(1 << 20);
    let mut r = jpeg2000_decode_limited(raw, cap).map_err(codec_err)?;
    if r.channels as usize != chunk_spp {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            off,
            format!(
                "chunk {idx}: JPEG 2000 codestream has {} components, the page declares {chunk_spp} samples",
                r.channels
            ),
        ));
    }
    let sample_bytes = (r.bits_per_sample as usize).div_ceil(8);
    if sample_bytes != bps || !matches!(layout.sample_format, 1 | 4) {
        return Err(Error::unsupported(
            FORMAT_ID,
            format!(
                "chunk {idx}: {}-bit JPEG 2000 samples in a page of {}-bit samples (sample format {})",
                r.bits_per_sample, layout.bits_per_sample, layout.sample_format
            ),
            "The JPEG 2000 precision must match the page's BitsPerSample and be unsigned.",
        ));
    }
    if layout.compression == 33003 && r.channels == 3 {
        ycbcr_to_rgb(&mut r.data, sample_bytes);
    }
    let (jw, jh) = (r.width as usize, r.height as usize);
    let px = chunk_spp * bps;
    if jw == cw && jh >= rows {
        let mut d = r.data;
        d.truncate(expected);
        return Ok(d);
    }
    // Re-grid into the chunk's geometry (a codestream narrower, shorter or wider than it).
    let mut d = vec![0u8; expected];
    let (row_out, row_in) = (cw * px, jw * px);
    let n = row_out.min(row_in);
    for y in 0..rows.min(jh) {
        d[y * row_out..y * row_out + n].copy_from_slice(&r.data[y * row_in..y * row_in + n]);
    }
    Ok(d)
}

/// OpenJPEG `sycc_to_rgb` on interleaved Y, Cb, Cr samples (8-bit or little-endian 16-bit).
fn ycbcr_to_rgb(data: &mut [u8], sample_bytes: usize) {
    let (offset, upb) = if sample_bytes == 1 {
        (128i32, 255i32)
    } else {
        (32768, 65535)
    };
    let conv = |y: i32, cb: i32, cr: i32| -> [i32; 3] {
        let (cb, cr) = (f64::from(cb - offset), f64::from(cr - offset));
        let r = y + (1.402 * cr) as i32;
        let g = y - (0.344 * cb + 0.714 * cr) as i32;
        let b = y + (1.772 * cb) as i32;
        [r.clamp(0, upb), g.clamp(0, upb), b.clamp(0, upb)]
    };
    if sample_bytes == 1 {
        for px in data.as_chunks_mut::<3>().0 {
            let rgb = conv(i32::from(px[0]), i32::from(px[1]), i32::from(px[2]));
            for (d, v) in px.iter_mut().zip(rgb) {
                *d = v as u8;
            }
        }
    } else {
        for px in data.as_chunks_mut::<6>().0 {
            let s = |i: usize| i32::from(u16::from_le_bytes([px[2 * i], px[2 * i + 1]]));
            let rgb = conv(s(0), s(1), s(2));
            for (i, v) in rgb.into_iter().enumerate() {
                px[2 * i..2 * i + 2].copy_from_slice(&(v as u16).to_le_bytes());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ycbcr_conversion_follows_openjpeg() {
        // Neutral grey stays grey; pure chroma moves R and B the JFIF way (truncating).
        let mut px = [200u8, 128, 128, 100, 228, 128, 100, 128, 228];
        ycbcr_to_rgb(&mut px, 1);
        assert_eq!(px[..3], [200, 200, 200]);
        // cb = +100: r = 100, g = 100 - 34 = 66, b = 100 + 177 = 255 (clipped)
        assert_eq!(px[3..6], [100, 66, 255]);
        // cr = +100: r = 100 + 140 = 240, g = 100 - 71 = 29, b = 100
        assert_eq!(px[6..], [240, 29, 100]);
    }
}
