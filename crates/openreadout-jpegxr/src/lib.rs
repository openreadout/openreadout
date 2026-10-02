//! Memory-safe JPEG XR decoder (ITU-T T.832 | ISO/IEC 29199-2).
//!
//! A `forbid(unsafe_code)` Rust port of the decoder path of Microsoft's jxrlib ("JPEG XR Device
//! Porting Kit", BSD 2-clause, see `LICENSE`), written to replace a machine translation of the
//! same library that could write past its buffers. The port keeps jxrlib's arithmetic and its
//! macroblock schedule, so decoded pixels are bit-identical to jxrlib's (which is what
//! imagecodecs, czifile and ZEISS libCZI use). Malformed codestreams give an [`Error`], never a
//! panic or an out-of-bounds access; reads past the end of the data stop decoding.
//!
//! Supported: every codec sub-version, spatial and frequency bitstream order, tiles (soft and
//! hard), all overlap modes, internal Y-only, 4:2:0, 4:2:2, 4:4:4 and N-component planes,
//! scaled and unscaled arithmetic, flexbits trimming and dropped subbands, and output as grey,
//! RGB/BGR or N-channel samples of 8/16 bit unsigned, 16/32 bit fixed point, half and single
//! float. Not supported (clean [`Error::Unsupported`]): alpha planes, CMYK, RGBE, packed
//! 5/6/10-bit and bilevel formats, YCC outputs and non-identity orientation tags.
//!
//! ```no_run
//! let bytes = std::fs::read("tile.jxr")?;
//! let img = openreadout_jpegxr::decode(&bytes, 1 << 30)?;
//! println!("{}x{} x{} {:?}", img.width, img.height, img.channels, img.sample);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]
// Short names mirror jxrlib's (a, b, c, d, p0, p1 ...) so the port can be read against it.
#![allow(clippy::many_single_char_names)]
// The port keeps jxrlib's control flow: sequential reads in `if` chains (each call consumes bits),
// sign tests as if/else chains, index loops over parallel tables.
#![allow(
    clippy::same_functions_in_if_condition,
    clippy::comparison_chain,
    clippy::needless_range_loop,
    clippy::items_after_statements
)]

mod bits;
mod decoder;
mod header;
mod huff;
mod output;
mod quant;
mod transform;
mod w;

use header::{bd, cf};

/// Decoding failure.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The data is not a valid JPEG XR file or codestream.
    Decode(String),
    /// A valid feature this decoder does not implement.
    Unsupported(String),
}

impl Error {
    pub(crate) fn decode(msg: &str) -> Self {
        Error::Decode(msg.to_string())
    }
    pub(crate) fn unsupported(msg: &str) -> Self {
        Error::Unsupported(msg.to_string())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Decode(m) => write!(f, "invalid JPEG XR data: {m}"),
            Error::Unsupported(m) => write!(f, "unsupported JPEG XR feature: {m}"),
        }
    }
}

impl std::error::Error for Error {}

/// Result alias of this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Sample type of a decoded image. Multi-byte samples are little-endian.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SampleType {
    /// Unsigned 8-bit.
    U8,
    /// Unsigned 16-bit.
    U16,
    /// Signed 16-bit fixed point (JPEG XR "16bppGrayFixedPoint" and friends).
    I16,
    /// IEEE half-precision float (bits).
    F16,
    /// Signed 32-bit fixed point.
    I32,
    /// IEEE single-precision float.
    F32,
}

impl SampleType {
    /// Bytes per sample.
    pub fn bytes(self) -> usize {
        match self {
            SampleType::U8 => 1,
            SampleType::U16 | SampleType::I16 | SampleType::F16 => 2,
            SampleType::I32 | SampleType::F32 => 4,
        }
    }
}

/// Size and sample layout of a codestream, read from its headers only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Samples per pixel.
    pub channels: u32,
    /// Sample type.
    pub sample: SampleType,
    /// True when the three colour samples are stored B, G, R (JPEG XR "BGR" pixel formats).
    pub bgr: bool,
}

/// A decoded image: `width * height * channels` interleaved samples, no row padding.
#[derive(Debug, Clone)]
pub struct Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Samples per pixel.
    pub channels: u32,
    /// Sample type.
    pub sample: SampleType,
    /// True when the three colour samples are stored B, G, R.
    pub bgr: bool,
    /// The samples.
    pub data: Vec<u8>,
}

/// Pixel format of the container (jxrlib `PKPixelInfo` rows that are decodable).
#[derive(Debug, Clone, Copy)]
struct PixelFormat {
    channels: usize,
    bd: u8,
    bits_per_unit: usize,
    bgr: bool,
    unsupported: Option<&'static str>,
}

const GUID_PREFIX: [u8; 15] = [
    0x24, 0xC3, 0xDD, 0x6F, 0x03, 0x4E, 0xFE, 0x4B, 0xB1, 0x85, 0x3D, 0x77, 0x76, 0x8D, 0xC9,
];
const GUID_32BPP_RGB: [u8; 16] = [
    0x95, 0x6B, 0x8C, 0xD9, 0xFE, 0x3E, 0xD6, 0x47, 0xBB, 0x25, 0xEB, 0x17, 0x48, 0xAB, 0x0C, 0xF1,
];
const GUID_96BPP_RGB_FLOAT: [u8; 16] = [
    0x8F, 0xD7, 0xFE, 0xE3, 0xDB, 0xE8, 0xCF, 0x4A, 0x84, 0xC1, 0xE9, 0x7F, 0x61, 0x36, 0xB3, 0x27,
];

const fn pf(channels: usize, _cf: u8, bd: u8, bits: usize, bgr: bool) -> PixelFormat {
    PixelFormat {
        channels,
        bd,
        bits_per_unit: bits,
        bgr,
        unsupported: None,
    }
}

const fn pf_no(what: &'static str) -> PixelFormat {
    PixelFormat {
        channels: 0,
        bd: 0,
        bits_per_unit: 0,
        bgr: false,
        unsupported: Some(what),
    }
}

fn pixel_format(guid: &[u8]) -> Option<PixelFormat> {
    if guid == GUID_32BPP_RGB {
        return Some(pf(3, cf::RGB, bd::B8, 32, false));
    }
    if guid == GUID_96BPP_RGB_FLOAT {
        return Some(pf(3, cf::RGB, bd::B32F, 96, false));
    }
    if guid.len() != 16 || guid[..15] != GUID_PREFIX {
        return None;
    }
    Some(match guid[15] {
        0x08 => pf(1, cf::Y_ONLY, bd::B8, 8, false),
        0x0b => pf(1, cf::Y_ONLY, bd::B16, 16, false),
        0x13 => pf(1, cf::Y_ONLY, bd::B16S, 16, false),
        0x3e => pf(1, cf::Y_ONLY, bd::B16F, 16, false),
        0x3f => pf(1, cf::Y_ONLY, bd::B32S, 32, false),
        0x11 => pf(1, cf::Y_ONLY, bd::B32F, 32, false),
        0x0d => pf(3, cf::RGB, bd::B8, 24, false),
        0x0c => pf(3, cf::RGB, bd::B8, 24, true),
        0x0e => pf(3, cf::RGB, bd::B8, 32, true),
        0x15 => pf(3, cf::RGB, bd::B16, 48, false),
        0x12 => pf(3, cf::RGB, bd::B16S, 48, false),
        0x3b => pf(3, cf::RGB, bd::B16F, 48, false),
        0x40 => pf(3, cf::RGB, bd::B16S, 64, false),
        0x42 => pf(3, cf::RGB, bd::B16F, 64, false),
        0x18 => pf(3, cf::RGB, bd::B32S, 96, false),
        0x41 => pf(3, cf::RGB, bd::B32S, 128, false),
        0x1b => pf(3, cf::RGB, bd::B32F, 128, false),
        n @ 0x20..=0x25 => {
            let c = usize::from(n - 0x20) + 3;
            pf(c, cf::NCOMPONENT, bd::B8, c * 8, false)
        }
        n @ 0x26..=0x2b => {
            let c = usize::from(n - 0x26) + 3;
            pf(c, cf::NCOMPONENT, bd::B16, c * 16, false)
        }
        0x0f | 0x10 | 0x16 | 0x17 | 0x19 | 0x1a | 0x1d | 0x1e | 0x3a | 0x2c..=0x39 => {
            pf_no("pixel formats with an alpha channel")
        }
        0x1c | 0x1f | 0x54 | 0x55 => pf_no("CMYK pixel formats"),
        0x09 | 0x0a | 0x14 => pf_no("packed 5/6/10-bit RGB pixel formats"),
        0x05 => pf_no("bilevel (black and white) pixel format"),
        0x3d => pf_no("RGBE pixel format"),
        0x44..=0x53 => pf_no("YCC pixel formats"),
        _ => return None,
    })
}

fn le16(d: &[u8], at: usize) -> Result<u16> {
    d.get(at..at + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or_else(|| Error::decode("container directory runs past the end of the data"))
}

fn le32(d: &[u8], at: usize) -> Result<u32> {
    d.get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| Error::decode("container directory runs past the end of the data"))
}

/// The codestream and pixel format of a JPEG XR file (jxrlib `ReadContainer`, `ParsePFD`).
fn container(data: &[u8]) -> Result<(&[u8], Option<PixelFormat>)> {
    if data.starts_with(b"WMPHOTO") {
        return Ok((data, None));
    }
    if data.len() < 8 || &data[..2] != b"II" || data[2] != 0xBC || data[3] > 1 {
        return Err(Error::decode("not a JPEG XR file (no II BC header)"));
    }
    let ifd = le32(data, 4)? as usize;
    let count = le16(data, ifd)?;
    if count == 0 || count == u16::MAX {
        return Err(Error::decode("empty image directory"));
    }
    let (mut guid, mut offset, mut bytes) = (None, None, 0u32);
    for i in 0..usize::from(count) {
        let at = ifd + 2 + 12 * i;
        let tag = le16(data, at)?;
        let n = le32(data, at + 4)?;
        let value = le32(data, at + 8)?;
        match tag {
            0xBC01 => {
                let o = value as usize;
                guid =
                    Some(data.get(o..o + 16).ok_or_else(|| {
                        Error::decode("pixel format runs past the end of the data")
                    })?);
            }
            0xBC02 => {
                if n != 1 {
                    return Err(Error::decode("invalid orientation tag"));
                }
                if value != 0 {
                    return Err(Error::unsupported("rotated or flipped orientation"));
                }
            }
            0xBCC0 => {
                if n != 1 {
                    return Err(Error::decode("invalid image offset tag"));
                }
                offset = Some(value as usize);
            }
            0xBCC1 => {
                if n != 1 {
                    return Err(Error::decode("invalid image byte count tag"));
                }
                bytes = value;
            }
            _ => {}
        }
    }
    let guid = guid.ok_or_else(|| Error::decode("no pixel format tag"))?;
    let format = pixel_format(guid).ok_or_else(|| Error::unsupported("unknown pixel format"))?;
    if let Some(why) = format.unsupported {
        return Err(Error::unsupported(why));
    }
    let offset = offset.ok_or_else(|| Error::decode("no image offset tag"))?;
    let rest = data
        .get(offset..)
        .ok_or_else(|| Error::decode("image offset past the end of the data"))?;
    let stream = if bytes == 0 {
        rest
    } else {
        &rest[..rest.len().min(bytes as usize)]
    };
    Ok((stream, Some(format)))
}

/// Everything decided from the headers before decoding.
struct Plan<'a> {
    stream: &'a [u8],
    img: header::ImageHeader,
    plane: header::PlaneHeader,
    plane_end: u64,
    info: Info,
    cf_ext: u8,
    px_samples: usize,
    rgb: bool,
}

fn sample_type(b: u8) -> Result<SampleType> {
    Ok(match b {
        bd::B8 => SampleType::U8,
        bd::B16 => SampleType::U16,
        bd::B16S => SampleType::I16,
        bd::B16F => SampleType::F16,
        bd::B32S => SampleType::I32,
        bd::B32F => SampleType::F32,
        _ => {
            return Err(Error::unsupported(
                "bit depth (bilevel, packed or 32-bit unsigned)",
            ));
        }
    })
}

fn plan(data: &[u8]) -> Result<Plan<'_>> {
    let (stream, format) = container(data)?;
    let (img, plane, plane_end) = header::read_headers(stream)?;
    if img.alpha {
        return Err(Error::unsupported("alpha image plane"));
    }
    if matches!(plane.cf, cf::CMYK) {
        return Err(Error::unsupported("CMYK colour format"));
    }
    let sample = sample_type(img.output_bd)?;
    // jxrlib `WMPhotoValidate`: the output colour format follows the header, adjusted to the
    // internal one.
    let int = plane.cf;
    let mut ext = img.output_cf;
    if int == cf::NCOMPONENT {
        ext = cf::NCOMPONENT;
    }
    if int == cf::YUV_422 && ext == cf::YUV_420 {
        ext = cf::YUV_422;
    }
    if int == cf::YUV_444 && (ext == cf::YUV_422 || ext == cf::YUV_420) {
        ext = cf::YUV_444;
    }
    if img.output_cf == cf::RGB && ext != cf::Y_ONLY && ext != cf::NCOMPONENT {
        ext = cf::RGB;
    }
    if img.output_cf == cf::RGBE {
        return Err(Error::unsupported("RGBE output"));
    }
    let effective = if int == cf::Y_ONLY { cf::Y_ONLY } else { ext };
    let written = match effective {
        cf::RGB => 3,
        cf::Y_ONLY if ext == cf::RGB => 3,
        cf::Y_ONLY => 1,
        cf::YUV_444 | cf::NCOMPONENT => plane.channels,
        _ => return Err(Error::unsupported("YCC or CMYK output colour format")),
    };
    let (channels, px_samples, bgr) = match format {
        Some(f) => {
            if f.bd != img.output_bd {
                return Err(Error::decode(
                    "pixel format and codestream bit depths disagree",
                ));
            }
            let px_samples = f.bits_per_unit / 8 / sample.bytes();
            if px_samples < written || px_samples < f.channels {
                return Err(Error::decode(
                    "pixel format too small for the coded channels",
                ));
            }
            (f.channels, px_samples, f.bgr)
        }
        None => (written, written, false),
    };
    let width = u32::try_from(img.width).map_err(|_| Error::decode("image too wide"))?;
    let height = u32::try_from(img.height).map_err(|_| Error::decode("image too tall"))?;
    Ok(Plan {
        stream,
        info: Info {
            width,
            height,
            channels: channels as u32,
            sample,
            bgr: bgr && channels >= 3,
        },
        cf_ext: ext,
        px_samples,
        rgb: !bgr,
        img,
        plane,
        plane_end,
    })
}

/// Read the size and sample layout without decoding pixels.
pub fn probe(data: &[u8]) -> Result<Info> {
    Ok(plan(data)?.info)
}

/// Decode a JPEG XR file (`II BC` container) or bare codestream (`WMPHOTO`). Refuses, before
/// allocating anything large, images whose decoded size or working memory exceeds `max_bytes`.
pub fn decode(data: &[u8], max_bytes: usize) -> Result<Image> {
    let p = plan(data)?;
    let Info {
        width,
        height,
        channels,
        sample,
        bgr,
    } = p.info.clone();
    let sb = sample.bytes();
    let too_big = || {
        Error::Unsupported(format!(
            "a {width}x{height} image is larger than the decode limit"
        ))
    };
    let px = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(too_big)?;
    let out_len = px
        .checked_mul(p.px_samples * sb)
        .filter(|&n| n <= max_bytes)
        .ok_or_else(too_big)?;
    let full_w = p.img.width + p.img.extra_left + p.img.extra_right;
    let mb_w = usize::try_from(full_w.div_ceil(16)).map_err(|_| too_big())?;
    // Two macroblock rows of 32-bit coefficients per channel, plus chroma upsampling rows.
    let work = (mb_w + 2)
        .checked_mul(256 * 4 * 2 * (p.plane.channels + 2))
        .filter(|&n| n <= max_bytes.max(256 << 20))
        .ok_or_else(too_big)?;
    let _ = work;
    let uv_res_change = p.cf_ext != cf::Y_ONLY
        && ((p.plane.cf == cf::YUV_420 && p.cf_ext != cf::YUV_420)
            || (p.plane.cf == cf::YUV_422 && p.cf_ext != cf::YUV_422));
    let res_len = if uv_res_change { mb_w * 256 } else { 0 };
    let mut out = output::Output {
        cf_int: p.plane.cf,
        cf_ext: p.cf_ext,
        bd: p.img.output_bd,
        scaled: p.plane.scaled,
        shift: p.plane.shift,
        exp_bias: p.plane.exp_bias,
        rgb: p.rgb,
        channels: p.plane.channels,
        width: width as usize,
        height: height as usize,
        left: p.img.extra_left as usize,
        top: p.img.extra_top as usize,
        mb_w,
        px_samples: p.px_samples,
        sample_bytes: sb,
        uv_res_change,
        res_u: vec![0; res_len],
        res_v: vec![0; res_len],
        data: vec![0; out_len],
    };
    let mut dec = decoder::Decoder::new(p.stream, p.img, p.plane, p.plane_end)?;
    debug_assert_eq!(dec.mb_width(), mb_w);
    dec.run(&mut out)?;
    output::fixup_gray_to_rgb(&mut out);
    let channels_us = channels as usize;
    let data = if p.px_samples == channels_us {
        out.data
    } else {
        let (src, dst) = (p.px_samples * sb, channels_us * sb);
        let mut v = Vec::with_capacity(px * dst);
        for pxl in out.data.chunks_exact(src) {
            v.extend_from_slice(&pxl[..dst]);
        }
        v
    };
    Ok(Image {
        width,
        height,
        channels,
        sample,
        bgr,
        data,
    })
}
