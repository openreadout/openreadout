//! JPEG XL, as TIFF compression 50002 (and 52546) stores it: one JPEG XL codestream or
//! container per chunk. Backed by `jxl-oxide` (MIT OR Apache-2.0, pure Rust, conformance-tested
//! against libjxl), built without its thread pool (TIFF chunks are already decoded in
//! parallel) and with an allocation tracker, so a hostile header cannot make it allocate more
//! than the caller's limit.

use jxl_oxide::image::BitDepth;
use jxl_oxide::{AllocTracker, JxlImage};

use crate::{CodecError, MAX_UNSIZED_OUTPUT, Raster, Result};

fn err(detail: impl Into<String>) -> CodecError {
    CodecError::Decode {
        codec: "jpeg-xl",
        detail: detail.into(),
    }
}

/// Decode the first frame, every channel (colour, then extra channels such as alpha), to
/// interleaved samples: u8 for up to 8-bit images, little-endian u16 up to 16-bit, f32 for
/// floating-point images. Integer samples are rounded as libjxl does (`v × max + 0.5`).
pub(crate) fn decode(data: &[u8], max_bytes: usize) -> Result<Raster> {
    let limit = max_bytes.min(MAX_UNSIZED_OUTPUT);
    // The decoder's own working buffers (f32 or i32 grids per channel, plus transforms) are a
    // few times the output; the tracker makes it fail cleanly instead of allocating further.
    let image = JxlImage::builder()
        .alloc_tracker(AllocTracker::with_limit(limit.saturating_mul(8)))
        .read(std::io::Cursor::new(data))
        .map_err(|e| err(e.to_string()))?;
    let (w, h) = (image.width(), image.height());
    let meta = &image.image_header().metadata;
    let colour = if meta.grayscale() { 1u32 } else { 3 };
    let channels = colour + u32::try_from(meta.ec_info.len()).unwrap_or(u32::MAX);
    let (bits, float) = match meta.bit_depth {
        BitDepth::IntegerSample { bits_per_sample } if bits_per_sample <= 8 => (8u32, false),
        BitDepth::IntegerSample { bits_per_sample } if bits_per_sample <= 16 => (16, false),
        BitDepth::FloatSample {
            bits_per_sample, ..
        } if bits_per_sample <= 32 => (32, true),
        other => {
            return Err(CodecError::Unsupported {
                codec: "jpeg-xl",
                detail: format!("{} bits per sample", other.bits_per_sample()),
            });
        }
    };
    let bytes = u64::from(w) * u64::from(h) * u64::from(channels) * u64::from(bits / 8);
    if bytes > limit as u64 {
        return Err(err(format!(
            "header declares an implausible {w}x{h}x{channels} image"
        )));
    }
    let render = image.render_frame(0).map_err(|e| err(e.to_string()))?;
    let fb = render.image_all_channels();
    if fb.width() != w as usize || fb.height() != h as usize || fb.channels() != channels as usize {
        return Err(err(format!(
            "rendered {}x{}x{} samples, header declares {w}x{h}x{channels}",
            fb.width(),
            fb.height(),
            fb.channels()
        )));
    }
    let samples = fb.buf();
    let data: Vec<u8> = match (bits, float) {
        (8, false) => samples
            .iter()
            .map(|&v| (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8)
            .collect(),
        (16, false) => samples
            .iter()
            .flat_map(|&v| ((v * 65535.0 + 0.5).clamp(0.0, 65535.0) as u16).to_le_bytes())
            .collect(),
        _ => samples.iter().flat_map(|v| v.to_le_bytes()).collect(),
    };
    Ok(Raster {
        width: w,
        height: h,
        channels,
        bits_per_sample: bits,
        float,
        bgr: false,
        data,
    })
}
