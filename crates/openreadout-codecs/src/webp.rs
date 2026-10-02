//! WebP (lossy VP8 and lossless VP8L), as TIFF compression 50001 stores it (one WebP file per
//! chunk). Backed by `image-webp` (image-rs; MIT OR Apache-2.0, pure Rust).

use crate::{CodecError, MAX_UNSIZED_OUTPUT, Raster, Result};

fn err(detail: impl Into<String>) -> CodecError {
    CodecError::Decode {
        codec: "webp",
        detail: detail.into(),
    }
}

/// Decode a WebP file to 8-bit R,G,B (or R,G,B,A when it has alpha), refusing before decoding
/// an image of more than `max_bytes` decoded bytes.
pub(crate) fn decode(data: &[u8], max_bytes: usize) -> Result<Raster> {
    let mut dec =
        image_webp::WebPDecoder::new(std::io::Cursor::new(data)).map_err(|e| err(e.to_string()))?;
    let (w, h) = dec.dimensions();
    let channels: u32 = if dec.has_alpha() { 4 } else { 3 };
    let bytes = u64::from(w) * u64::from(h) * u64::from(channels);
    if bytes > max_bytes.min(MAX_UNSIZED_OUTPUT) as u64 {
        return Err(err(format!("header declares an implausible {w}x{h} image")));
    }
    let len = dec
        .output_buffer_size()
        .ok_or_else(|| err("image too large"))?;
    if len as u64 != bytes {
        return Err(err("unexpected output size"));
    }
    let mut data = vec![0u8; len];
    dec.read_image(&mut data).map_err(|e| err(e.to_string()))?;
    Ok(Raster {
        width: w,
        height: h,
        channels,
        bits_per_sample: 8,
        float: false,
        bgr: false,
        data,
    })
}
