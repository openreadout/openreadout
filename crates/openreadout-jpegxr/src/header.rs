//! Image header and image-plane header (T.832 clauses 8.2 and 8.3; jxrlib `ReadWMIHeader`,
//! `ReadImagePlaneHeader`).

use crate::bits::BitReader;
use crate::{Error, Result};

/// Internal and output colour formats (jxrlib `COLORFORMAT`).
pub(crate) mod cf {
    pub(crate) const Y_ONLY: u8 = 0;
    pub(crate) const YUV_420: u8 = 1;
    pub(crate) const YUV_422: u8 = 2;
    pub(crate) const YUV_444: u8 = 3;
    pub(crate) const CMYK: u8 = 4;
    pub(crate) const NCOMPONENT: u8 = 6;
    pub(crate) const RGB: u8 = 7;
    pub(crate) const RGBE: u8 = 8;
}

/// Output bit depths (jxrlib `BITDEPTH_BITS`).
pub(crate) mod bd {
    pub(crate) const B1: u8 = 0;
    pub(crate) const B8: u8 = 1;
    pub(crate) const B16: u8 = 2;
    pub(crate) const B16S: u8 = 3;
    pub(crate) const B16F: u8 = 4;
    pub(crate) const B32: u8 = 5;
    pub(crate) const B32S: u8 = 6;
    pub(crate) const B32F: u8 = 7;
    pub(crate) const B1_ALT: u8 = 0xF;
}

/// Subband selection (jxrlib `SUBBAND`).
pub(crate) mod sb {
    pub(crate) const ALL: u8 = 0;
    pub(crate) const NO_FLEXBITS: u8 = 1;
    pub(crate) const NO_HIGHPASS: u8 = 2;
    pub(crate) const DC_ONLY: u8 = 3;
}

pub(crate) const SUBVERSION_ORIGINAL: u8 = 0;
const SUBVERSION_SOFT_TILES: u8 = 1;
const SUBVERSION_HARD_TILES: u8 = 9;
const LOG_MAX_TILES: u32 = 12;
pub(crate) const MAX_CHANNELS: usize = 16;

/// Image header.
#[derive(Debug, Clone)]
pub(crate) struct ImageHeader {
    pub(crate) subversion: u8,
    pub(crate) hard_tiles: bool,
    pub(crate) frequency_mode: bool,
    pub(crate) index_table: bool,
    pub(crate) overlap: u8,
    pub(crate) trim_flexbits: bool,
    pub(crate) alpha: bool,
    /// Output colour format (the "source" format of the encoder).
    pub(crate) output_cf: u8,
    pub(crate) output_bd: u8,
    /// Displayed size (without the windowing margins).
    pub(crate) width: u64,
    pub(crate) height: u64,
    pub(crate) extra_top: u64,
    pub(crate) extra_left: u64,
    pub(crate) extra_bottom: u64,
    pub(crate) extra_right: u64,
    /// First macroblock column of each vertical slice (tile column).
    pub(crate) tile_x: Vec<u32>,
    /// First macroblock row of each horizontal slice (tile row).
    pub(crate) tile_y: Vec<u32>,
}

/// Image-plane header.
#[derive(Debug, Clone)]
pub(crate) struct PlaneHeader {
    pub(crate) cf: u8,
    pub(crate) channels: usize,
    pub(crate) scaled: bool,
    pub(crate) subband: u8,
    pub(crate) shift: u8,
    pub(crate) exp_bias: i8,
    /// jxrlib `uQPMode` bit field.
    pub(crate) qp_mode: u32,
    pub(crate) qp_dc: [u8; MAX_CHANNELS],
    pub(crate) qp_lp: [u8; MAX_CHANNELS],
    pub(crate) qp_hp: [u8; MAX_CHANNELS],
}

fn fail(msg: &str) -> Error {
    Error::decode(msg)
}

/// Parse the image header and the first image-plane header. Returns them and the byte offset
/// that follows (where the index table starts).
pub(crate) fn read_headers(data: &[u8]) -> Result<(ImageHeader, PlaneHeader, u64)> {
    if data.len() < 8 || &data[..7] != b"WMPHOTO" {
        return Err(fail("missing WMPHOTO image header signature"));
    }
    let mut r = BitReader::new(&data[8..]);
    if r.get(4) != 1 {
        return Err(fail("unsupported codec version"));
    }
    let subversion = r.get(4) as u8;
    if ![
        SUBVERSION_ORIGINAL,
        SUBVERSION_SOFT_TILES,
        SUBVERSION_HARD_TILES,
    ]
    .contains(&subversion)
    {
        return Err(fail("unsupported codec sub-version"));
    }
    let tiling = r.get_bool();
    let frequency_mode = r.get_bool();
    let _orientation = r.get(3); // jxrlib ignores it (the container tag decides)
    let index_table = r.get_bool();
    let overlap = r.get(2) as u8;
    if overlap == 3 {
        return Err(fail("invalid overlap mode 3"));
    }
    let short_header = r.get_bool();
    let _long_word = r.get_bool();
    let windowing = r.get_bool();
    let trim_flexbits = r.get_bool();
    let tile_stretch = r.get_bool();
    let _rb_swapped = r.get_bool();
    let _reserved = r.get_bool();
    let alpha = r.get_bool();
    let output_cf = r.get(4) as u8;
    let mut output_bd = r.get(4) as u8;
    if output_bd == bd::B1_ALT {
        output_bd = bd::B1;
    }
    let size_bits = if short_header { 16 } else { 32 };
    let mut width = u64::from(r.get(size_bits)) + 1;
    let mut height = u64::from(r.get(size_bits)) + 1;
    let (mut extra_top, mut extra_left, mut extra_bottom, mut extra_right) = (0, 0, 0, 0);
    if !windowing && width & 15 != 0 {
        extra_right = 16 - (width & 15);
    }
    if !windowing && height & 15 != 0 {
        extra_bottom = 16 - (height & 15);
    }
    let (mut nv, mut nh) = (0usize, 0usize);
    if tiling {
        nv = r.get(LOG_MAX_TILES) as usize;
        nh = r.get(LOG_MAX_TILES) as usize;
    }
    if !index_table && (frequency_mode || nv + nh > 0) {
        return Err(fail(
            "tiled or frequency-mode codestream without an index table",
        ));
    }
    let tile_bits = if short_header { 8 } else { 16 };
    let mut tile_x = vec![0u32; nv + 1];
    for i in 0..nv {
        tile_x[i + 1] = tile_x[i].saturating_add(r.get(tile_bits));
    }
    let mut tile_y = vec![0u32; nh + 1];
    for i in 0..nh {
        tile_y[i + 1] = tile_y[i].saturating_add(r.get(tile_bits));
    }
    if tile_stretch {
        for _ in 0..(nv + 1) * (nh + 1) {
            r.get(8);
        }
    }
    if windowing {
        extra_top = u64::from(r.get(6));
        extra_left = u64::from(r.get(6));
        extra_bottom = u64::from(r.get(6));
        extra_right = u64::from(r.get(6));
    }
    if ((width + extra_left + extra_right) & 15) + ((height + extra_top + extra_bottom) & 15) != 0 {
        if (width & 15) + (height & 15) + extra_left + extra_top != 0 {
            return Err(fail("inconsistent windowing margins"));
        }
        if width <= extra_right || height <= extra_bottom {
            return Err(fail("windowing margins larger than the image"));
        }
        width -= extra_right;
        height -= extra_bottom;
    }
    r.align();
    if r.overrun() {
        return Err(fail("image header is truncated"));
    }
    let image = ImageHeader {
        subversion,
        hard_tiles: subversion == SUBVERSION_HARD_TILES,
        frequency_mode,
        index_table,
        overlap,
        trim_flexbits,
        alpha,
        output_cf,
        output_bd,
        width,
        height,
        extra_top,
        extra_left,
        extra_bottom,
        extra_right,
        tile_x,
        tile_y,
    };
    let plane = read_plane_header(&mut r, &image)?;
    if r.overrun() {
        return Err(fail("image plane header is truncated"));
    }
    Ok((image, plane, 8 + r.byte_pos()))
}

/// jxrlib `readQuantizerSB`: channel mode and the QP indexes of one band.
fn read_quantizer(r: &mut BitReader<'_>, qp: &mut [u8; MAX_CHANNELS], channels: usize) -> u32 {
    if channels >= MAX_CHANNELS {
        return 0;
    }
    let mode = if channels > 1 { r.get(2) } else { 0 };
    qp[0] = r.get(8) as u8;
    if mode == 1 {
        qp[1] = r.get(8) as u8;
    } else if mode > 0 {
        for q in qp.iter_mut().take(channels).skip(1) {
            *q = r.get(8) as u8;
        }
    }
    mode
}

pub(crate) fn read_plane_header(r: &mut BitReader<'_>, image: &ImageHeader) -> Result<PlaneHeader> {
    let cf = r.get(3) as u8;
    let scaled = r.get_bool();
    let subband = r.get(4) as u8;
    let channels = match cf {
        cf::Y_ONLY => 1,
        cf::YUV_420 => {
            r.get(1);
            r.get(3);
            r.get(1);
            r.get(3);
            3
        }
        cf::YUV_422 => {
            r.get(1);
            r.get(3);
            r.get(4);
            3
        }
        cf::YUV_444 => {
            r.get(4);
            r.get(4);
            3
        }
        cf::NCOMPONENT => {
            let n = r.get(4) as usize + 1;
            r.get(4);
            n
        }
        cf::CMYK => 4,
        _ => return Err(fail("invalid internal colour format")),
    };
    let (mut shift, mut exp_bias) = (0u8, 0i8);
    match image.output_bd {
        bd::B16 | bd::B16S | bd::B32 | bd::B32S => shift = r.get(8) as u8,
        bd::B32F => {
            shift = r.get(8) as u8;
            exp_bias = r.get(8) as u8 as i8;
        }
        _ => {}
    }
    let mut qp_dc = [0u8; MAX_CHANNELS];
    let mut qp_lp = [0u8; MAX_CHANNELS];
    let mut qp_hp = [0u8; MAX_CHANNELS];
    let mut mode: u32 = 0;
    if r.get_bool() {
        mode += read_quantizer(r, &mut qp_dc, channels) << 3;
    } else {
        mode += 1;
    }
    if subband != sb::DC_ONLY {
        if r.get_bool() {
            mode += ((mode & 1) << 1) + ((mode & 0x18) << 2);
        } else {
            mode += 0x200;
            if r.get_bool() {
                mode += read_quantizer(r, &mut qp_lp, channels) << 5;
            } else {
                mode += 2;
            }
        }
        if subband != sb::NO_HIGHPASS {
            if r.get_bool() {
                mode += ((mode & 2) << 1) + ((mode & 0x60) << 2);
            } else {
                mode += 0x400;
                if r.get_bool() {
                    mode += read_quantizer(r, &mut qp_hp, channels) << 7;
                } else {
                    mode += 4;
                }
            }
        }
    }
    if subband == sb::DC_ONLY {
        mode |= 0x200;
    } else if subband == sb::NO_HIGHPASS {
        mode |= 0x400;
    }
    if mode & 0x600 == 0 {
        return Err(fail("frame-level quantizers are not specified"));
    }
    r.align();
    Ok(PlaneHeader {
        cf,
        channels,
        scaled,
        subband,
        shift,
        exp_bias,
        qp_mode: mode,
        qp_dc,
        qp_lp,
        qp_hp,
    })
}
