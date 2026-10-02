//! Macroblock decoding: packets and tiles, adaptive entropy decoding of DC, lowpass and highpass
//! coefficients, prediction, dequantization, and the macroblock schedule that drives the inverse
//! transform and the output stage (T.832 clauses 8.4-9.7; jxrlib `strdec.c`, `segdec.c`,
//! `strPredQuantDec.c`, `strPredQuant.c`, `decode.c`, `image.c`).

use crate::bits::BitReader;
use crate::header::{ImageHeader, MAX_CHANNELS, PlaneHeader, SUBVERSION_ORIGINAL, cf, sb};
use crate::huff::AdaptiveHuffman;
use crate::output::{Output, output_row};
use crate::quant::{self, BandQuant};
use crate::transform::{self, Edges, HardState, Setup, Tiles};
use crate::w::W;
use crate::{Error, Result};

const MAXTOTAL: u32 = 32767;
/// Slack before and after the row buffers (jxrlib's neighbouring allocations).
const BUF_MARGIN: usize = 1024;
const CTDC: usize = 5;
const CONTEXTX: usize = 8;
const NUM_VLC_TABLES: usize = CONTEXTX * 2 + CTDC;
const ALPHABET: [usize; NUM_VLC_TABLES] = [
    5, 4, 8, 7, 7, 12, 6, 6, 12, 6, 6, 7, 7, 12, 6, 6, 12, 6, 6, 7, 7,
];

const ZIGZAG_LOWPASS: [usize; 16] = [0, 1, 4, 5, 2, 8, 6, 9, 3, 12, 10, 7, 13, 11, 14, 15];
const ZIGZAG_V: [usize; 16] = [0, 4, 8, 5, 1, 12, 9, 6, 2, 13, 3, 15, 7, 10, 14, 11];
/// Permutation from coefficient order to a block's buffer layout (jxrlib `dctIndex[0]`).
const DCT_INDEX0: [usize; 16] = [0, 5, 1, 6, 10, 12, 8, 14, 2, 4, 3, 7, 9, 13, 11, 15];
/// Positions of the lowpass coefficients in a macroblock buffer (jxrlib `dctIndex[2]`).
const DCT_INDEX2: [usize; 16] = [
    0, 128, 64, 208, 32, 240, 48, 224, 16, 192, 80, 144, 112, 176, 96, 160,
];
const BLK_OFFSET: [usize; 16] = [
    0, 64, 16, 80, 128, 192, 144, 208, 32, 96, 48, 112, 160, 224, 176, 240,
];
const BLK_OFFSET_UV: [usize; 4] = [0, 32, 16, 48];
const BLK_OFFSET_UV422: [usize; 8] = [0, 64, 16, 80, 32, 96, 48, 112];
const SIG_RUN_BIN: [usize; 15] = [0, 0, 0, 0, 2, 2, 2, 1, 1, 1, 1, 0, 0, 0, 0];
const SIG_RUN_FIXED: [u32; 15] = [0, 0, 1, 1, 3, 0, 0, 1, 1, 2, 0, 0, 0, 0, 1];
const SIG_RUN_REMAP: [i32; 15] = [1, 2, 3, 5, 7, 1, 2, 3, 5, 7, 1, 2, 3, 4, 5];

#[derive(Debug, Clone, Copy, Default)]
struct Scan {
    total: u32,
    pos: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Band {
    #[default]
    Dc,
    Lp,
    Ac,
}

/// Adaptive fixed-length-bit model (jxrlib `CAdaptiveModel`).
#[derive(Debug, Clone, Copy, Default)]
struct Model {
    flc_state: [i32; 2],
    flc_bits: [i32; 2],
    band: Band,
}

#[derive(Debug, Clone, Copy, Default)]
struct CbpModel {
    count0: [i32; 2],
    count1: [i32; 2],
    state: [i32; 2],
}

/// Coding context of one tile column (jxrlib `CCodingContext`).
#[derive(Debug, Clone)]
struct Context {
    /// Readers of the DC, lowpass, highpass and flexbits bands.
    io: [usize; 4],
    cbpcy: AdaptiveHuffman,
    cbpcy1: AdaptiveHuffman,
    ah: Vec<AdaptiveHuffman>,
    scan_lp: [Scan; 16],
    scan_h: [Scan; 16],
    scan_v: [Scan; 16],
    model_ac: Model,
    model_lp: Model,
    model_dc: Model,
    cbp_count_zero: i32,
    cbp_count_max: i32,
    cbp_model: CbpModel,
    trim: i32,
}

impl Context {
    fn new(cbp_symbols: usize) -> Self {
        Context {
            io: [0; 4],
            cbpcy: AdaptiveHuffman::new(cbp_symbols),
            cbpcy1: AdaptiveHuffman::new(5),
            ah: ALPHABET.iter().map(|&n| AdaptiveHuffman::new(n)).collect(),
            scan_lp: [Scan::default(); 16],
            scan_h: [Scan::default(); 16],
            scan_v: [Scan::default(); 16],
            model_ac: Model::default(),
            model_lp: Model::default(),
            model_dc: Model::default(),
            cbp_count_zero: 0,
            cbp_count_max: 0,
            cbp_model: CbpModel::default(),
            trim: 0,
        }
    }

    fn adapt_lowpass(&mut self) {
        for a in &mut self.ah[..CONTEXTX + CTDC] {
            a.adapt();
        }
    }

    fn adapt_highpass(&mut self) {
        self.cbpcy.adapt();
        self.cbpcy1.adapt();
        for a in &mut self.ah[CONTEXTX + CTDC..] {
            a.adapt();
        }
    }

    /// jxrlib `ResetCodingContextDec`.
    fn reset(&mut self) {
        self.cbpcy.reset();
        self.cbpcy1.reset();
        for a in &mut self.ah {
            a.reset();
        }
        self.adapt_lowpass();
        self.adapt_highpass();
        for i in 0..16 {
            self.scan_lp[i].pos = ZIGZAG_LOWPASS[i];
            self.scan_h[i].pos = DCT_INDEX0[ZIGZAG_LOWPASS[i]];
            self.scan_v[i].pos = DCT_INDEX0[ZIGZAG_V[i]];
        }
        self.model_ac = Model {
            band: Band::Ac,
            ..Model::default()
        };
        self.model_lp = Model {
            band: Band::Lp,
            flc_bits: [4, 4],
            ..Model::default()
        };
        self.model_dc = Model {
            band: Band::Dc,
            flc_bits: [8, 8],
            ..Model::default()
        };
        self.cbp_count_max = 1;
        self.cbp_count_zero = 1;
        self.cbp_model = CbpModel {
            count0: [-4, -4],
            count1: [4, 4],
            state: [0, 0],
        };
    }
}

/// Quantizers of one tile column (jxrlib `CWMITile`).
#[derive(Debug, Clone, Default)]
struct Tile {
    dc: BandQuant,
    lp: BandQuant,
    hp: BandQuant,
    num_qp_lp: usize,
    num_qp_hp: usize,
    bits_lp: u32,
    bits_hp: u32,
}

/// What prediction keeps of a macroblock (jxrlib `CWMIPredInfo`).
#[derive(Debug, Clone, Copy, Default)]
struct PredInfo {
    qp_index: i32,
    cbp: i32,
    dc: i32,
    ad: [i32; 6],
}

/// The macroblock being decoded (jxrlib `CWMIMBInfo`).
#[derive(Debug, Clone)]
struct MbInfo {
    block_dc: [[i32; 16]; MAX_CHANNELS],
    orientation: i32,
    cbp: [i32; MAX_CHANNELS],
    diff_cbp: [i32; MAX_CHANNELS],
    qidx_lp: usize,
    qidx_hp: usize,
}

/// jxrlib `UpdateModelMB`.
fn update_model(cf: u8, channels: usize, lm: [i32; 2], m: &mut Model) {
    const W0: [i32; 3] = [240, 12, 1];
    const W1: [[i32; 16]; 3] = [
        [
            0, 240, 120, 80, 60, 48, 40, 34, 30, 27, 24, 22, 20, 18, 17, 16,
        ],
        [0, 12, 6, 4, 3, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1],
        [0, 16, 8, 5, 4, 3, 3, 2, 2, 2, 2, 1, 1, 1, 1, 1],
    ];
    const W2: [i32; 6] = [120, 37, 2, 120, 18, 1];
    let band = match m.band {
        Band::Dc => 0,
        Band::Lp => 1,
        Band::Ac => 2,
    };
    let mut lm = lm;
    lm[0] = lm[0].wrapping_mul(W0[band]);
    if cf == cf::YUV_420 {
        lm[1] = lm[1].wrapping_mul(W2[band]);
    } else if cf == cf::YUV_422 {
        lm[1] = lm[1].wrapping_mul(W2[3 + band]);
    } else {
        lm[1] = lm[1].wrapping_mul(W1[band][(channels - 1).min(15)]);
        if m.band == Band::Ac {
            lm[1] >>= 4;
        }
    }
    for j in 0..2 {
        let mut ms = m.flc_state[j];
        let mut delta = lm[j].wrapping_sub(70) >> 2;
        if delta <= -8 {
            delta += 4;
            delta = delta.max(-16);
            ms += delta;
            if ms < -8 {
                if m.flc_bits[j] == 0 {
                    ms = -8;
                } else {
                    ms = 0;
                    m.flc_bits[j] -= 1;
                }
            }
        } else if delta >= 8 {
            delta -= 4;
            delta = delta.min(15);
            ms += delta;
            if ms > 8 {
                if m.flc_bits[j] >= 15 {
                    m.flc_bits[j] = 15;
                    ms = 8;
                } else {
                    ms = 0;
                    m.flc_bits[j] += 1;
                }
            }
        }
        m.flc_state[j] = ms;
        if cf == cf::Y_ONLY {
            break;
        }
    }
}

/// jxrlib `DecodeSignificantAbsLevel`.
fn abs_level(ah: &mut AdaptiveHuffman, r: &mut BitReader<'_>) -> Result<i32> {
    const REMAP: [i32; 6] = [2, 3, 4, 6, 10, 14];
    const FIXED: [u32; 6] = [0, 0, 1, 2, 2, 2];
    let idx = ah.decode(r)?;
    ah.update(idx);
    let idx = usize::try_from(idx).map_err(|_| Error::decode("invalid level symbol"))?;
    if idx < 2 {
        Ok(idx as i32 + 2)
    } else if idx < 6 {
        Ok(REMAP[idx] + r.get(FIXED[idx]) as i32)
    } else {
        let mut fixed = r.get(4) + 4;
        if fixed == 19 {
            fixed += r.get(2);
            if fixed == 22 {
                fixed += r.get(3);
            }
        }
        let level = 2i32.wrapping_add(1i32.wrapping_shl(fixed));
        Ok(level.wrapping_add(r.get(fixed) as i32))
    }
}

/// jxrlib `DecodeSignificantRun`.
fn significant_run(max_run: i32, ah: &AdaptiveHuffman, r: &mut BitReader<'_>) -> Result<i32> {
    if max_run < 5 {
        if max_run == 1 || r.get_bool() {
            return Ok(1);
        }
        if max_run == 2 || r.get_bool() {
            return Ok(2);
        }
        if max_run == 3 || r.get_bool() {
            return Ok(3);
        }
        return Ok(4);
    }
    let bin = SIG_RUN_BIN
        .get(max_run as usize)
        .copied()
        .ok_or_else(|| Error::decode("run beyond the block"))?;
    let idx = ah.decode_short(r)? as usize + bin * 5;
    let run = *SIG_RUN_REMAP
        .get(idx)
        .ok_or_else(|| Error::decode("invalid run symbol"))?;
    let flc = SIG_RUN_FIXED[idx];
    Ok(run + r.get(flc) as i32)
}

/// jxrlib `DecodeIndex` for the symbols after the first.
fn decode_index(loc: i32, ah: &mut AdaptiveHuffman, r: &mut BitReader<'_>) -> Result<i32> {
    if loc < 15 {
        let idx = ah.decode_short(r)?;
        ah.update2(idx);
        Ok(idx)
    } else if loc == 15 {
        if !r.get_bool() {
            Ok(0)
        } else if !r.get_bool() {
            Ok(2)
        } else {
            Ok(1 + 2 * i32::from(r.get_bool()))
        }
    } else {
        Ok(r.get(1) as i32)
    }
}

/// jxrlib `DecodeBlock`: run-level pairs of a lowpass block. Returns the number of pairs.
fn decode_block(
    chroma: bool,
    rl: &mut [i32; 32],
    ah: &mut [AdaptiveHuffman],
    offset: usize,
    r: &mut BitReader<'_>,
    mut loc: i32,
) -> Result<usize> {
    let a1 = offset + usize::from(chroma) * 3;
    let idx = ah[a1].decode(r)?;
    ah[a1].update2(idx);
    let mut sr = idx & 1;
    let mut srn = idx >> 2;
    let mut cont = (sr & srn) as usize;
    let sign = r.get_sign();
    rl[1] = if idx & 2 != 0 {
        (abs_level(&mut ah[6 + offset + cont], r)? ^ sign).wrapping_sub(sign)
    } else {
        1 | sign
    };
    rl[0] = 0;
    if sr == 0 {
        rl[0] = significant_run(15 - loc, &ah[0], r)?;
    }
    loc = loc.wrapping_add(rl[0] + 1);
    let mut n = 1usize;
    while srn != 0 {
        if n >= 16 {
            return Err(Error::decode("too many lowpass coefficients in a block"));
        }
        sr = srn & 1;
        rl[n * 2] = 0;
        if sr == 0 {
            rl[n * 2] = significant_run(15 - loc, &ah[0], r)?;
        }
        loc = loc.wrapping_add(rl[n * 2] + 1);
        let idx = decode_index(loc, &mut ah[a1 + cont + 1], r)?;
        srn = idx >> 1;
        if !(0..3).contains(&srn) {
            return Err(Error::decode("invalid lowpass symbol"));
        }
        cont &= srn as usize;
        let sign = r.get_sign();
        rl[n * 2 + 1] = if idx & 1 != 0 {
            (abs_level(&mut ah[6 + offset + cont], r)? ^ sign).wrapping_sub(sign)
        } else {
            1 | sign
        };
        n += 1;
    }
    Ok(n)
}

fn bump_scan(scan: &mut [Scan; 16], loc: usize) {
    scan[loc].total = scan[loc].total.wrapping_add(1);
    if loc > 0 && scan[loc].total > scan[loc - 1].total {
        scan.swap(loc, loc - 1);
    }
}

fn reset_totals(scan: &mut [Scan; 16]) {
    scan[0].total = MAXTOTAL;
    let mut weight = 32;
    for s in scan.iter_mut().skip(1) {
        s.total = weight;
        weight -= 2;
    }
}

/// jxrlib `DecodeBlockHighpass`: highpass coefficients of one 4x4 block, written (already
/// multiplied by the step) into `coef[base + ..]`.
fn decode_block_highpass(
    chroma: bool,
    ah: &mut [AdaptiveHuffman],
    r: &mut BitReader<'_>,
    qp: i32,
    coef: &mut [i32],
    base: usize,
    scan: &mut [Scan; 16],
) -> Result<usize> {
    let offset = CTDC + CONTEXTX;
    let a1 = offset + usize::from(chroma) * 3;
    let mut loc: usize = 1;
    let idx = ah[a1].decode(r)?;
    ah[a1].update2(idx);
    let sr = idx & 1;
    let mut srn = idx >> 2;
    let mut cont = (sr & srn) as usize;
    let sign = r.get_sign();
    let mut level = W((qp ^ sign).wrapping_sub(sign));
    if idx & 2 != 0 {
        level = level * abs_level(&mut ah[6 + offset + cont], r)?;
    }
    if sr == 0 {
        loc += significant_run(15 - loc as i32, &ah[0], r)? as usize;
    }
    loc &= 15;
    coef[base + scan[loc].pos] = level.0;
    bump_scan(scan, loc);
    loc = (loc + 1) & 15;
    let mut n = 1usize;
    while srn != 0 {
        if n > 16 {
            return Err(Error::decode("too many highpass coefficients in a block"));
        }
        let sr = srn & 1;
        if sr == 0 {
            loc += significant_run(15 - loc as i32, &ah[0], r)? as usize;
            if loc >= 16 {
                return Ok(16);
            }
        }
        let idx = decode_index(loc as i32 + 1, &mut ah[a1 + cont + 1], r)?;
        srn = idx >> 1;
        if !(0..3).contains(&srn) {
            return Err(Error::decode("invalid highpass symbol"));
        }
        cont &= srn as usize;
        let sign = r.get_sign();
        let mut level = W((qp ^ sign).wrapping_sub(sign));
        if idx & 1 != 0 {
            level = level * abs_level(&mut ah[6 + offset + cont], r)?;
        }
        coef[base + scan[loc].pos] = level.0;
        bump_scan(scan, loc);
        loc = (loc + 1) & 15;
        n += 1;
    }
    Ok(n)
}

/// jxrlib `DecodeBlockAdaptive` flexbits pass.
fn flexbits(r: &mut BitReader<'_>, coef: &mut [i32], base: usize, flex: u32, qp: i32, trim: i32) {
    if qp.wrapping_add(trim) == 1 {
        for &o in &DCT_INDEX0[1..] {
            let k = &mut coef[base + o];
            if *k < 0 {
                *k = k.wrapping_sub(r.get(flex) as i32);
            } else if *k > 0 {
                *k = k.wrapping_add(r.get(flex) as i32);
            } else {
                *k = r.get_signed(flex);
            }
        }
    } else {
        let qp1 = W(qp) << trim;
        for &o in &DCT_INDEX0[1..] {
            let k = &mut coef[base + o];
            if *k < 0 {
                *k = (W(*k) - qp1 * (r.get(flex) as i32)).0;
            } else if *k > 0 {
                *k = (W(*k) + qp1 * (r.get(flex) as i32)).0;
            } else {
                *k = (qp1 * r.get_signed(flex)).0;
            }
        }
    }
}

fn popcount16(v: i32) -> i32 {
    (v & 0xffff).count_ones() as i32
}

fn saturate32(x: &mut i32) {
    if (x.wrapping_add(16) as u32) >= 32 {
        *x = if *x < 0 { -16 } else { 15 };
    }
}

fn cbp_state(m: &mut CbpModel, c1: usize, n: i32) {
    m.count0[c1] += n - 3;
    saturate32(&mut m.count0[c1]);
    m.count1[c1] += 16 - n - 3;
    saturate32(&mut m.count1[c1]);
    m.state[c1] = if m.count0[c1] < 0 {
        if m.count0[c1] < m.count1[c1] { 1 } else { 2 }
    } else if m.count1[c1] < 0 {
        2
    } else {
        0
    };
}

/// The decoder for one image plane.
pub(crate) struct Decoder<'a> {
    data: &'a [u8],
    img: ImageHeader,
    pl: PlaneHeader,
    cf: u8,
    channels: usize,
    mb_w: usize,
    mb_h: usize,
    csb: usize,
    nv: usize,
    frequency: bool,
    num_bitio: usize,
    index_table: Vec<u64>,
    header_size: u64,
    readers: Vec<BitReader<'a>>,
    ctx: Vec<Context>,
    tiles: Vec<Tile>,
    mb: MbInfo,
    pred_cur: Vec<Vec<PredInfo>>,
    pred_prev: Vec<Vec<PredInfo>>,
    /// All coefficient/pixel rows, laid out as jxrlib lays out its memory: per channel the two
    /// macroblock-row buffers back to back (`a0`, `a1`), channels one after the other, and a
    /// margin before and after. Byte-exactness depends on it: with hard tiles jxrlib's transform
    /// writes past the last macroblock of a row, into whatever buffer follows.
    buf: Vec<i32>,
    /// Start of channel `c`'s first row buffer in `buf`.
    ch_base: Vec<usize>,
    mb_size: Vec<usize>,
    /// Which region holds the current row (jxrlib `a1MBbuffer`); the other is the previous row.
    cur_region: usize,
    col: usize,
    row: usize,
    tile_col: usize,
    tile_row: usize,
    ctx_left: bool,
    ctx_top: bool,
    reset_ctx: bool,
    reset_rgi: bool,
    hard: HardState,
    decode_lp: bool,
    decode_hp: bool,
    skip_flex: bool,
}

/// jxrlib `GetVLWordEsc`.
fn vlw(r: &mut BitReader<'_>) -> u64 {
    let s = r.get(8);
    if s == 0xfd || s == 0xfe || s == 0xff {
        0
    } else if s < 0xfb {
        (u64::from(s) << 8) | u64::from(r.get(8))
    } else {
        let mut v = 0u64;
        if s - 0xfb != 0 {
            v = u64::from(r.get(16)) << 16;
            v = (v | u64::from(r.get(16))) << 32;
        }
        v |= u64::from(r.get(16)) << 16;
        v | u64::from(r.get(16))
    }
}

impl<'a> Decoder<'a> {
    /// Set up the decoder: `plane_end` is the offset just past the plane header(s).
    pub(crate) fn new(
        data: &'a [u8],
        img: ImageHeader,
        pl: PlaneHeader,
        plane_end: u64,
    ) -> Result<Self> {
        let cf = pl.cf;
        let channels = pl.channels;
        if pl.subband > sb::DC_ONLY {
            return Err(Error::unsupported("isolated or reserved subband selection"));
        }
        let csb = match pl.subband {
            sb::DC_ONLY => 1,
            sb::NO_HIGHPASS => 2,
            sb::NO_FLEXBITS => 3,
            _ => 4,
        };
        let full_w = img.width + img.extra_left + img.extra_right;
        let full_h = img.height + img.extra_top + img.extra_bottom;
        let mb_w =
            usize::try_from(full_w.div_ceil(16)).map_err(|_| Error::decode("image too wide"))?;
        let mb_h =
            usize::try_from(full_h.div_ceil(16)).map_err(|_| Error::decode("image too tall"))?;
        let nv = img.tile_x.len() - 1;
        let nh = img.tile_y.len() - 1;
        let num_bitio = if !img.index_table {
            0
        } else if img.frequency_mode {
            (nv + 1) * csb
        } else {
            nv + 1
        };
        // Index table and the offset of the first packet.
        let start = usize::try_from(plane_end).unwrap_or(usize::MAX);
        let mut r = BitReader::new(data.get(start..).unwrap_or(&[]));
        let mut index_table = Vec::new();
        if num_bitio > 0 {
            if r.get(16) != 1 {
                return Err(Error::decode("missing index table marker"));
            }
            let entries = num_bitio * (nh + 1);
            // Each entry takes at least one byte.
            if entries > data.len() {
                return Err(Error::decode("index table larger than the codestream"));
            }
            index_table.reserve(entries);
            for _ in 0..entries {
                index_table.push(vlw(&mut r));
            }
        }
        let extra = vlw(&mut r);
        r.align();
        if r.overrun() {
            return Err(Error::decode("index table is truncated"));
        }
        let header_size = extra.saturating_add(plane_end.saturating_add(r.byte_pos()));
        let cbp_symbols = if matches!(cf, cf::Y_ONLY | cf::NCOMPONENT | cf::CMYK) {
            5
        } else {
            9
        };
        let mut ctx: Vec<Context> = (0..=nv).map(|_| Context::new(cbp_symbols)).collect();
        for (k, c) in ctx.iter_mut().enumerate() {
            c.io = if num_bitio == 0 {
                [0; 4]
            } else if !img.frequency_mode {
                [k; 4]
            } else {
                let b = k * csb;
                [
                    b,
                    if csb > 1 { b + 1 } else { b },
                    if csb > 2 { b + 2 } else { b },
                    if csb > 3 { b + 3 } else { b },
                ]
            };
            c.reset();
        }
        let readers = vec![BitReader::new(&[]); num_bitio.max(1)];
        let mut tiles = vec![
            Tile {
                num_qp_lp: 1,
                num_qp_hp: 1,
                ..Tile::default()
            };
            nv + 1
        ];
        // Frame-level (uniform) quantizers (jxrlib `StrDecInit`).
        let mode = pl.qp_mode;
        if mode & 1 == 0 {
            let mut q = quant::alloc(channels, 1);
            for (i, qc) in q.iter_mut().enumerate() {
                qc[0].index = pl.qp_dc[i];
            }
            quant::format(&mut q, (mode >> 3) & 3, 0, true, pl.scaled);
            for t in &mut tiles {
                t.dc = q.clone();
            }
        }
        if pl.subband != sb::DC_ONLY {
            if mode & 2 == 0 {
                let mut q = quant::alloc(channels, 1);
                if mode & 0x200 == 0 {
                    for (i, qc) in q.iter_mut().enumerate() {
                        qc[0] = tiles[0]
                            .dc
                            .get(i)
                            .and_then(|v| v.first())
                            .copied()
                            .ok_or_else(|| Error::decode("lowpass uses a missing DC quantizer"))?;
                    }
                } else {
                    for (i, qc) in q.iter_mut().enumerate() {
                        qc[0].index = pl.qp_lp[i];
                    }
                    quant::format(&mut q, (mode >> 5) & 3, 0, true, pl.scaled);
                }
                for t in &mut tiles {
                    t.lp = q.clone();
                }
            }
            if pl.subband != sb::NO_HIGHPASS && mode & 4 == 0 {
                let mut q = quant::alloc(channels, 1);
                if mode & 0x400 == 0 {
                    for (i, qc) in q.iter_mut().enumerate() {
                        qc[0] = tiles[0]
                            .lp
                            .get(i)
                            .and_then(|v| v.first())
                            .copied()
                            .ok_or_else(|| {
                                Error::decode("highpass uses a missing lowpass quantizer")
                            })?;
                    }
                } else {
                    for (i, qc) in q.iter_mut().enumerate() {
                        qc[0].index = pl.qp_hp[i];
                    }
                    quant::format(&mut q, (mode >> 7) & 3, 0, false, pl.scaled);
                }
                for t in &mut tiles {
                    t.hp = q.clone();
                }
            }
        }
        let chroma_blocks = match cf {
            cf::YUV_420 => 4,
            cf::YUV_422 => 8,
            _ => 16,
        };
        let mb_size: Vec<usize> = (0..channels)
            .map(|c| if c == 0 { 256 } else { chroma_blocks * 16 })
            .collect();
        let mut ch_base = Vec::with_capacity(channels);
        let mut len = BUF_MARGIN;
        for &s in &mb_size {
            ch_base.push(len);
            len = mb_w
                .checked_mul(s)
                .and_then(|n| n.checked_mul(2))
                .and_then(|n| n.checked_add(len))
                .ok_or_else(|| Error::decode("image too wide"))?;
        }
        let buf = vec![0i32; len + BUF_MARGIN];
        let pred = vec![vec![PredInfo::default(); mb_w]; channels];
        Ok(Decoder {
            data,
            cf,
            channels,
            mb_w,
            mb_h,
            csb,
            nv,
            frequency: img.frequency_mode,
            num_bitio,
            index_table,
            header_size,
            readers,
            ctx,
            tiles,
            mb: MbInfo {
                block_dc: [[0; 16]; MAX_CHANNELS],
                orientation: 0,
                cbp: [0; MAX_CHANNELS],
                diff_cbp: [0; MAX_CHANNELS],
                qidx_lp: 0,
                qidx_hp: 0,
            },
            pred_cur: pred.clone(),
            pred_prev: pred,
            buf,
            ch_base,
            mb_size,
            cur_region: 1,
            col: 0,
            row: 0,
            tile_col: 0,
            tile_row: 0,
            ctx_left: false,
            ctx_top: false,
            reset_ctx: false,
            reset_rgi: false,
            hard: HardState::default(),
            decode_lp: pl.subband != sb::DC_ONLY,
            decode_hp: pl.subband == sb::ALL || pl.subband == sb::NO_FLEXBITS,
            skip_flex: pl.subband == sb::NO_FLEXBITS,
            img,
            pl,
        })
    }

    pub(crate) fn mb_width(&self) -> usize {
        self.mb_w
    }

    /// Start of `region`'s macroblock 0 in channel `ch`'s buffer.
    fn base(&self, ch: usize, region: usize) -> usize {
        self.ch_base[ch] + region * self.mb_w * self.mb_size[ch]
    }

    /// Current macroblock in the current row of channel `ch` (jxrlib `p1MBbuffer`).
    fn p1(&self, ch: usize) -> usize {
        self.base(ch, self.cur_region) + self.col * self.mb_size[ch]
    }

    /// jxrlib `getTilePos`.
    fn tile_pos(&mut self) {
        let (x, y) = (self.col, self.row);
        let tx = &self.img.tile_x;
        let ty = &self.img.tile_y;
        let at = |t: &[u32], k: usize| t.get(k).map_or(usize::MAX, |&v| v as usize);
        if x == 0 {
            self.tile_col = 0;
        } else if self.tile_col < self.nv && x == at(tx, self.tile_col + 1) {
            self.tile_col += 1;
        }
        let nh = ty.len() - 1;
        if y == 0 {
            self.tile_row = 0;
        } else if self.tile_row < nh && y == at(ty, self.tile_row + 1) {
            self.tile_row += 1;
        }
        let x0 = at(tx, self.tile_col);
        self.ctx_left = x == x0;
        self.ctx_top = y == at(ty, self.tile_row);
        self.reset_ctx = (x.wrapping_sub(x0) & 0xf) == 0;
        self.reset_rgi = self.reset_ctx;
        if self.tile_col == self.nv {
            if x + 1 == self.mb_w {
                self.reset_ctx = true;
            }
        } else if x + 1 == at(tx, self.tile_col + 1) {
            self.reset_ctx = true;
        }
    }

    fn attach(&mut self, k: usize, pos: u64) {
        let pos = usize::try_from(pos).unwrap_or(usize::MAX);
        self.readers[k] = BitReader::new(self.data.get(pos..).unwrap_or(&[]));
    }

    /// jxrlib `readPacketHeader`; `strict` = the error is reported.
    fn packet_header(&mut self, k: usize, strict: bool) -> Result<()> {
        let r = &mut self.readers[k];
        let ok = r.get(8) == 0 && r.get(8) == 0 && r.get(8) == 1;
        if ok {
            r.get(8);
        } else if strict {
            return Err(Error::decode("missing packet start code"));
        }
        Ok(())
    }

    /// jxrlib `readPackets` (tile-row start) and the tile headers of the tile's first macroblock.
    fn read_packets(&mut self) -> Result<()> {
        let ty = self
            .img
            .tile_y
            .get(self.tile_row)
            .map_or(usize::MAX, |&v| v as usize);
        if self.col == 0 && self.row == ty {
            for k in 0..self.num_bitio {
                let entry = self
                    .index_table
                    .get(self.num_bitio * self.tile_row + k)
                    .copied()
                    .unwrap_or(u64::MAX);
                self.attach(k, entry.saturating_add(self.header_size));
            }
            if self.num_bitio == 0 {
                self.attach(0, self.header_size);
            }
            for k in 0..=self.nv {
                if self.frequency {
                    let b = k * self.csb;
                    self.packet_header(b, true)?;
                    if self.csb > 1 {
                        self.packet_header(b + 1, true)?;
                    }
                    if self.csb > 2 {
                        self.packet_header(b + 2, true)?;
                    }
                    if self.csb > 3 {
                        self.packet_header(b + 3, false)?;
                        self.ctx[k].trim = if self.img.trim_flexbits {
                            self.readers[b + 3].get(4) as i32
                        } else {
                            0
                        };
                    }
                } else {
                    let io = if self.num_bitio == 0 { 0 } else { k };
                    self.packet_header(io, true)?;
                    self.ctx[k].trim = if self.img.trim_flexbits {
                        self.readers[io].get(4) as i32
                    } else {
                        0
                    };
                }
                self.ctx[k].reset();
            }
        }
        if self.ctx_left && self.ctx_top {
            self.tile_header_dc();
            if self.csb > 1 {
                self.tile_header_lp()?;
            }
            if self.csb > 2 {
                self.tile_header_hp()?;
            }
        }
        Ok(())
    }

    /// jxrlib `readQuantizer`.
    fn read_quantizer(&mut self, io: usize, q: &mut BandQuant, pos: usize) -> u32 {
        let r = &mut self.readers[io];
        let mode = if self.channels > 1 { r.get(2) } else { 0 };
        q[0][pos].index = r.get(8) as u8;
        if mode == 1 {
            if let Some(c) = q.get_mut(1) {
                c[pos].index = r.get(8) as u8;
            }
        } else if mode > 0 {
            for c in q.iter_mut().skip(1) {
                c[pos].index = r.get(8) as u8;
            }
        }
        mode
    }

    fn tile_header_dc(&mut self) {
        if self.pl.qp_mode & 1 != 0 {
            let io = self.ctx[self.tile_col].io[0];
            let mut q = quant::alloc(self.channels, 1);
            let mode = self.read_quantizer(io, &mut q, 0);
            quant::format(&mut q, mode, 0, true, self.pl.scaled);
            self.tiles[self.tile_col].dc = q;
        }
    }

    fn tile_header_lp(&mut self) -> Result<()> {
        if self.pl.subband != sb::DC_ONLY && self.pl.qp_mode & 2 != 0 {
            let io = self.ctx[self.tile_col].io[1];
            let use_dc = self.readers[io].get(1) == 1;
            let t = self.tile_col;
            self.tiles[t].bits_lp = 0;
            self.tiles[t].num_qp_lp = 1;
            if use_dc {
                let mut q = quant::alloc(self.channels, 1);
                for (i, qc) in q.iter_mut().enumerate() {
                    qc[0] = self.tiles[t]
                        .dc
                        .get(i)
                        .and_then(|v| v.first())
                        .copied()
                        .ok_or_else(|| Error::decode("lowpass uses a missing DC quantizer"))?;
                }
                self.tiles[t].lp = q;
            } else {
                let num = self.readers[io].get(4) as usize + 1;
                self.tiles[t].num_qp_lp = num;
                self.tiles[t].bits_lp = quant::dquant_bits(num);
                let mut q = quant::alloc(self.channels, num);
                for i in 0..num {
                    let mode = self.read_quantizer(io, &mut q, i);
                    quant::format(&mut q, mode, i, true, self.pl.scaled);
                }
                self.tiles[t].lp = q;
            }
        }
        Ok(())
    }

    fn tile_header_hp(&mut self) -> Result<()> {
        if self.pl.subband != sb::DC_ONLY
            && self.pl.subband != sb::NO_HIGHPASS
            && self.pl.qp_mode & 4 != 0
        {
            let io = self.ctx[self.tile_col].io[2];
            let use_lp = self.readers[io].get(1) == 1;
            let t = self.tile_col;
            self.tiles[t].bits_hp = 0;
            self.tiles[t].num_qp_hp = 1;
            if use_lp {
                let num = self.tiles[t].num_qp_lp;
                self.tiles[t].num_qp_hp = num;
                let mut q = quant::alloc(self.channels, num);
                for (i, qc) in q.iter_mut().enumerate() {
                    for (k, v) in qc.iter_mut().enumerate() {
                        *v = self.tiles[t]
                            .lp
                            .get(i)
                            .and_then(|l| l.get(k))
                            .copied()
                            .ok_or_else(|| {
                                Error::decode("highpass uses a missing lowpass quantizer")
                            })?;
                    }
                }
                self.tiles[t].hp = q;
            } else {
                let num = self.readers[io].get(4) as usize + 1;
                self.tiles[t].num_qp_hp = num;
                self.tiles[t].bits_hp = quant::dquant_bits(num);
                let mut q = quant::alloc(self.channels, num);
                for i in 0..num {
                    let mode = self.read_quantizer(io, &mut q, i);
                    quant::format(&mut q, mode, i, false, self.pl.scaled);
                }
                self.tiles[t].hp = q;
            }
        }
        Ok(())
    }

    fn qp_index(r: &mut BitReader<'_>, bits: u32) -> usize {
        if r.get(1) == 0 {
            0
        } else {
            r.get(bits) as usize + 1
        }
    }

    /// jxrlib `DecodeMacroblockDC`.
    fn decode_dc(&mut self) -> Result<()> {
        let t = self.tile_col;
        let (bits_lp, bits_hp) = (self.tiles[t].bits_lp, self.tiles[t].bits_hp);
        let (num_lp, num_hp) = (self.tiles[t].num_qp_lp, self.tiles[t].num_qp_hp);
        let cfm = self.cf;
        let channels = self.channels;
        for i in 0..channels {
            self.mb.block_dc[i] = [0; 16];
        }
        let ctx = &mut self.ctx[t];
        let r = &mut self.readers[ctx.io[0]];
        self.mb.qidx_lp = 0;
        self.mb.qidx_hp = 0;
        if !self.frequency && self.pl.subband != sb::DC_ONLY {
            if bits_lp > 0 {
                self.mb.qidx_lp = Self::qp_index(r, bits_lp);
            }
            if self.pl.subband != sb::NO_HIGHPASS && bits_hp > 0 {
                self.mb.qidx_hp = Self::qp_index(r, bits_hp);
            }
        }
        if bits_hp == 0 && num_hp > 1 {
            self.mb.qidx_hp = self.mb.qidx_lp;
        }
        if self.mb.qidx_lp >= num_lp || self.mb.qidx_hp >= num_hp {
            return Err(Error::decode("macroblock quantizer index out of range"));
        }
        let mut lm = [0i32; 2];
        let mut plm = 0;
        let mut model_bits = ctx.model_dc.flc_bits[0] as u32;
        if matches!(cfm, cf::Y_ONLY | cf::CMYK | cf::NCOMPONENT) {
            for i in 0..channels {
                let mut q = W(0);
                if r.get_bool() {
                    q = W(abs_level(&mut ctx.ah[3], r)? - 1);
                    lm[plm] += 1;
                }
                if model_bits != 0 {
                    q = (q << model_bits as i32) | W(r.get(model_bits) as i32);
                }
                if q.0 != 0 && r.get_bool() {
                    q = -q;
                }
                self.mb.block_dc[i][0] = q.0;
                plm = 1;
                model_bits = ctx.model_dc.flc_bits[1] as u32;
            }
        } else {
            let idx = ctx.ah[2].decode(r)?;
            let (qy, qu, qv) = (idx >> 2, (idx >> 1) & 1, idx & 1);
            let mut vals = [W(0); 3];
            for (c, present) in [qy, qu, qv].into_iter().enumerate() {
                let table = if c == 0 { 3 } else { 4 };
                let mut q = W(0);
                if present != 0 {
                    q = W(abs_level(&mut ctx.ah[table], r)? - 1);
                    lm[usize::from(c > 0)] += 1;
                }
                if model_bits != 0 {
                    q = (q << model_bits as i32) | W(r.get(model_bits) as i32);
                }
                if q.0 != 0 && r.get_bool() {
                    q = -q;
                }
                vals[c] = q;
                if c == 0 {
                    model_bits = ctx.model_dc.flc_bits[1] as u32;
                }
            }
            for (c, v) in vals.iter().enumerate() {
                self.mb.block_dc[c][0] = v.0;
            }
        }
        update_model(cfm, channels, lm, &mut ctx.model_dc);
        if self.pl.subband == sb::DC_ONLY && self.reset_ctx {
            for a in &mut ctx.ah[2..5] {
                a.adapt();
            }
        }
        Ok(())
    }

    /// jxrlib `DecodeMacroblockLowpass`.
    fn decode_lowpass(&mut self) -> Result<()> {
        let t = self.tile_col;
        let cfm = self.cf;
        let channels = self.channels;
        let sub = cfm == cf::YUV_420 || cfm == cf::YUV_422;
        let full_planes = if sub { 2 } else { channels };
        let ctx = &mut self.ctx[t];
        let r = &mut self.readers[ctx.io[1]];
        if self.frequency && self.tiles[t].bits_lp > 0 {
            self.mb.qidx_lp = Self::qp_index(r, self.tiles[t].bits_lp);
        }
        if self.reset_rgi {
            reset_totals(&mut ctx.scan_lp);
        }
        let mut cbp: i32 = 0;
        if matches!(cfm, cf::YUV_420 | cf::YUV_422 | cf::YUV_444) {
            let mut count_m = ctx.cbp_count_max;
            let mut count_z = ctx.cbp_count_zero;
            let max = (full_planes * 4 - 5) as i32;
            if count_z <= 0 || count_m < 0 {
                if r.get_bool() {
                    cbp = 1;
                    let k = r.get(full_planes as u32 - 1) as i32;
                    if k != 0 {
                        cbp = k * 2 + r.get(1) as i32;
                    }
                }
                if count_m < count_z {
                    cbp = max - cbp;
                }
            } else {
                cbp = r.get(full_planes as u32) as i32;
            }
            count_m += 1 - 4 * i32::from(cbp == max);
            count_z += 1 - 4 * i32::from(cbp == 0);
            ctx.cbp_count_max = count_m.clamp(-8, 7);
            ctx.cbp_count_zero = count_z.clamp(-8, 7);
        } else {
            for ch in 0..channels.min(31) {
                cbp |= (r.get(1) as i32) << ch;
            }
        }
        let mut model_bits = ctx.model_lp.flc_bits[0] as u32;
        let mut lm = [0i32; 2];
        let mut plm = 0usize;
        let mut rl = [0i32; 32];
        for ch in 0..full_planes {
            if cbp & 1 != 0 {
                let loc = 1
                    + 9 * i32::from(cfm == cf::YUV_420 && ch == 1)
                    + i32::from(cfm == cf::YUV_422 && ch == 1);
                let n = decode_block(ch > 0, &mut rl, &mut ctx.ah, CTDC, r, loc)?;
                lm[plm] += n as i32;
                if sub && ch > 0 {
                    let mut tmp = [0i32; 16];
                    let mut idx: i32 = 0;
                    for k in 0..n {
                        idx = idx.wrapping_add(rl[k * 2]);
                        tmp[(idx & 0xf) as usize] = rl[k * 2 + 1];
                        idx = idx.wrapping_add(1);
                    }
                    const REMAP: [usize; 7] = [4, 1, 2, 3, 5, 6, 7];
                    let remap = &REMAP[usize::from(cfm == cf::YUV_420)..];
                    let count = if cfm == cf::YUV_420 { 6 } else { 14 };
                    for (k, &v) in tmp.iter().enumerate().take(count) {
                        self.mb.block_dc[(k & 1) + 1][remap[k >> 1]] = v;
                    }
                } else {
                    let scan = &mut ctx.scan_lp;
                    let mut idx: usize = 1;
                    for k in 0..n {
                        idx = idx.wrapping_add(rl[k * 2] as usize);
                        if idx > 15 {
                            return Err(Error::decode("lowpass run past the end of the block"));
                        }
                        self.mb.block_dc[ch][scan[idx].pos] = rl[k * 2 + 1];
                        bump_scan(scan, idx);
                        idx += 1;
                    }
                }
            }
            if model_bits != 0 {
                let mb = model_bits as i32;
                if sub && ch > 0 {
                    for k in 1..(if cfm == cf::YUV_420 { 4 } else { 8 }) {
                        for c in 1..3 {
                            let v = W(self.mb.block_dc[c][k]);
                            let nv = if v.0 > 0 {
                                (v << mb) + r.get(model_bits) as i32
                            } else if v.0 < 0 {
                                (v << mb) - r.get(model_bits) as i32
                            } else {
                                let x = W(r.get(model_bits) as i32);
                                if x.0 != 0 && r.get_bool() { -x } else { x }
                            };
                            self.mb.block_dc[c][k] = nv.0;
                        }
                    }
                } else {
                    let coeffs = &mut self.mb.block_dc[ch];
                    for c in coeffs.iter_mut().skip(1) {
                        let v = W(*c);
                        *c = if v.0 > 0 {
                            ((v << mb) + r.get(model_bits) as i32).0
                        } else if v.0 < 0 {
                            ((v << mb) - r.get(model_bits) as i32).0
                        } else {
                            r.get_signed(model_bits)
                        };
                    }
                }
            }
            plm = 1;
            model_bits = ctx.model_lp.flc_bits[1] as u32;
            cbp >>= 1;
        }
        update_model(cfm, channels, lm, &mut ctx.model_lp);
        if self.reset_ctx {
            ctx.adapt_lowpass();
        }
        Ok(())
    }

    /// jxrlib `getDCACPredMode`.
    fn dcac_pred_mode(&self) -> (i32, i32) {
        let x = self.col;
        let dc_mode = if self.ctx_left && self.ctx_top {
            3
        } else if self.ctx_left {
            1
        } else if self.ctx_top {
            0
        } else {
            let cur = &self.pred_cur;
            let prev = &self.pred_prev;
            let (l, t, tl) = (W(cur[0][x - 1].dc), W(prev[0][x].dc), W(prev[0][x - 1].dc));
            let (h, v) = if self.cf == cf::Y_ONLY || self.cf == cf::NCOMPONENT {
                ((tl - l).0.wrapping_abs(), (tl - t).0.wrapping_abs())
            } else {
                let scale = match self.cf {
                    cf::YUV_420 => 8,
                    cf::YUV_422 => 4,
                    _ => 2,
                };
                let d = |a: i32, b: i32| a.wrapping_sub(b).wrapping_abs();
                let h = (tl - l)
                    .0
                    .wrapping_abs()
                    .wrapping_mul(scale)
                    .wrapping_add(d(prev[1][x - 1].dc, cur[1][x - 1].dc))
                    .wrapping_add(d(prev[2][x - 1].dc, cur[2][x - 1].dc));
                let v = (tl - t)
                    .0
                    .wrapping_abs()
                    .wrapping_mul(scale)
                    .wrapping_add(d(prev[1][x - 1].dc, prev[1][x].dc))
                    .wrapping_add(d(prev[2][x - 1].dc, prev[2][x].dc));
                (h, v)
            };
            if h.wrapping_mul(4) < v {
                1
            } else if v.wrapping_mul(4) < h {
                0
            } else {
                2
            }
        };
        let mut ad_mode = 2;
        let q = self.mb.qidx_lp as i32;
        if dc_mode == 1 && q == self.pred_prev[0][x].qp_index {
            ad_mode = 1;
        }
        if dc_mode == 0 && q == self.pred_cur[0][x - 1].qp_index {
            ad_mode = 0;
        }
        (dc_mode, ad_mode)
    }

    /// jxrlib `getACPredMode`.
    fn ac_pred_mode(&self) -> i32 {
        let a = |v: i32| v.wrapping_abs();
        let c = &self.mb.block_dc[0];
        let mut h = a(c[1]).wrapping_add(a(c[2])).wrapping_add(a(c[3]));
        let mut v = a(c[4]).wrapping_add(a(c[8])).wrapping_add(a(c[12]));
        if self.cf != cf::Y_ONLY && self.cf != cf::NCOMPONENT {
            let u = &self.mb.block_dc[1];
            let w = &self.mb.block_dc[2];
            h = h.wrapping_add(a(u[1])).wrapping_add(a(w[1]));
            match self.cf {
                cf::YUV_420 => v = v.wrapping_add(a(u[2])).wrapping_add(a(w[2])),
                cf::YUV_422 => {
                    v = v
                        .wrapping_add(a(u[2]))
                        .wrapping_add(a(w[2]))
                        .wrapping_add(a(u[6]))
                        .wrapping_add(a(w[6]));
                    h = h.wrapping_add(a(u[5])).wrapping_add(a(w[5]));
                }
                _ => v = v.wrapping_add(a(u[4])).wrapping_add(a(w[4])),
            }
        }
        if h.wrapping_mul(4) < v {
            1
        } else if v.wrapping_mul(4) < h {
            0
        } else {
            2
        }
    }

    /// jxrlib `predDCACDec`.
    fn pred_dcac(&mut self) {
        let x = self.col;
        let sub = self.cf == cf::YUV_420 || self.cf == cf::YUV_422;
        let luma_like = if sub { 1 } else { self.channels };
        let (dcm, adm) = self.dcac_pred_mode();
        let add = |a: &mut i32, b: i32| *a = a.wrapping_add(b);
        for ii in 0..luma_like {
            let org = &mut self.mb.block_dc[ii];
            match dcm {
                1 => add(&mut org[0], self.pred_prev[ii][x].dc),
                0 => add(&mut org[0], self.pred_cur[ii][x - 1].dc),
                2 => add(
                    &mut org[0],
                    ((W(self.pred_cur[ii][x - 1].dc) + W(self.pred_prev[ii][x].dc)) >> 1).0,
                ),
                _ => {}
            }
            if adm == 1 {
                let r = self.pred_prev[ii][x].ad;
                add(&mut org[4], r[3]);
                add(&mut org[8], r[4]);
                add(&mut org[12], r[5]);
            } else if adm == 0 {
                let r = self.pred_cur[ii][x - 1].ad;
                add(&mut org[1], r[0]);
                add(&mut org[2], r[1]);
                add(&mut org[3], r[2]);
            }
        }
        if sub {
            for ii in 1..3 {
                let org = &mut self.mb.block_dc[ii];
                match dcm {
                    1 => add(&mut org[0], self.pred_prev[ii][x].dc),
                    0 => add(&mut org[0], self.pred_cur[ii][x - 1].dc),
                    2 => add(
                        &mut org[0],
                        ((W(self.pred_cur[ii][x - 1].dc) + W(self.pred_prev[ii][x].dc) + 1) >> 1).0,
                    ),
                    _ => {}
                }
                if self.cf == cf::YUV_420 {
                    if adm == 1 {
                        add(&mut org[2], self.pred_prev[ii][x].ad[1]);
                    } else if adm == 0 {
                        add(&mut org[1], self.pred_cur[ii][x - 1].ad[0]);
                    }
                } else if adm == 1 {
                    add(&mut org[4], self.pred_prev[ii][x].ad[4]);
                    add(&mut org[2], self.pred_prev[ii][x].ad[3]);
                    let v = org[2];
                    add(&mut org[6], v);
                } else if adm == 0 {
                    add(&mut org[4], self.pred_cur[ii][x - 1].ad[4]);
                    add(&mut org[1], self.pred_cur[ii][x - 1].ad[0]);
                    add(&mut org[5], self.pred_cur[ii][x - 1].ad[2]);
                } else if dcm == 1 {
                    let v = org[2];
                    add(&mut org[6], v);
                }
            }
        }
        self.mb.orientation = 2 - self.ac_pred_mode();
    }

    /// jxrlib `dequantizeMacroblock`.
    fn dequantize(&mut self) -> Result<()> {
        let t = &self.tiles[self.tile_col];
        let missing = || Error::decode("macroblock uses a missing quantizer");
        for i in 0..self.channels {
            let p = self.p1(i);
            let dcq = t.dc.get(i).and_then(|v| v.first()).ok_or_else(missing)?.qp;
            let org = self.mb.block_dc[i];
            let d = &mut self.buf;
            d[p] = org[0].wrapping_mul(dcq);
            if self.pl.subband != sb::DC_ONLY {
                let lpq =
                    t.lp.get(i)
                        .and_then(|v| v.get(self.mb.qidx_lp))
                        .ok_or_else(missing)?
                        .qp;
                if i == 0 || (self.cf != cf::YUV_422 && self.cf != cf::YUV_420) {
                    for k in 1..16 {
                        d[p + DCT_INDEX2[k]] = org[k].wrapping_mul(lpq);
                    }
                } else if self.cf == cf::YUV_422 {
                    for (k, off) in [
                        (1, 64),
                        (2, 16),
                        (3, 80),
                        (4, 32),
                        (5, 96),
                        (6, 48),
                        (7, 112),
                    ] {
                        d[p + off] = org[k].wrapping_mul(lpq);
                    }
                } else {
                    for (k, off) in [(1, 32), (2, 16), (3, 48)] {
                        d[p + off] = org[k].wrapping_mul(lpq);
                    }
                }
            }
        }
        Ok(())
    }

    /// jxrlib `DecodeCBP`.
    fn decode_cbp(&mut self) -> Result<()> {
        const TAB: [i32; 4] = [6, 9, 10, 12];
        const FLC0: [u32; 6] = [0, 2, 1, 2, 2, 0];
        const OFF0: [usize; 6] = [0, 4, 2, 8, 12, 1];
        const OUT0: [i32; 16] = [0, 15, 3, 12, 1, 2, 4, 8, 5, 6, 9, 10, 7, 11, 13, 14];
        let cfm = self.cf;
        let n_ch = if cfm == cf::NCOMPONENT || cfm == cf::CMYK {
            self.channels
        } else {
            1
        };
        let ctx = &mut self.ctx[self.tile_col];
        let r = &mut self.readers[ctx.io[2]];
        let pattern = |r: &mut BitReader<'_>, code: i32| -> i32 {
            match code {
                2 => match r.get(2) {
                    0 => 3,
                    1 => 5,
                    n => TAB[(n as usize) * 2 + usize::from(r.get_bool()) - 4],
                },
                1 => 1 << r.get(2),
                3 => 0xf ^ (1 << r.get(2)),
                4 => 0xf,
                _ => 0,
            }
        };
        for i in 0..n_ch {
            let (mut cy, mut cu, mut cv) = (0i32, 0i32, 0i32);
            let num = ctx.cbpcy1.decode_short(r)?;
            ctx.cbpcy1.update(num);
            let num = pattern(r, num);
            for block in 0..4 {
                if num & (1 << block) == 0 {
                    continue;
                }
                let nb = ctx.cbpcy.decode(r)?;
                ctx.cbpcy.update(nb);
                let mut val =
                    usize::try_from(nb + 1).map_err(|_| Error::decode("invalid CBP symbol"))?;
                let mut code = 0i32;
                if val >= 6 {
                    code = if r.get_bool() {
                        0x10
                    } else if r.get_bool() {
                        0x20
                    } else {
                        0x30
                    };
                    if val == 9 {
                        if r.get_bool() {
                        } else if r.get_bool() {
                            val = 10;
                        } else {
                            val = 11;
                        }
                    }
                    val -= 6;
                }
                let flc = *FLC0
                    .get(val)
                    .ok_or_else(|| Error::decode("invalid CBP symbol"))?;
                let code1 = OFF0[val] + r.get(flc) as usize;
                code += OUT0[code1 & 15];
                match cfm {
                    cf::YUV_444 => {
                        cy |= (code & 0xf) << (block * 4);
                        for k in 0..2 {
                            if (code >> (k + 4)) & 1 != 0 {
                                let c = ctx.ah[1].decode_short(r)?;
                                let c = match c {
                                    1 => match r.get(2) {
                                        0 => 3,
                                        1 => 5,
                                        n => TAB[(n as usize) * 2 + usize::from(r.get_bool()) - 4],
                                    },
                                    0 => 1 << r.get(2),
                                    2 => 0xf ^ (1 << r.get(2)),
                                    3 => 0xf,
                                    _ => c,
                                };
                                if k == 0 {
                                    cu |= c << (block * 4);
                                } else {
                                    cv |= c << (block * 4);
                                }
                            }
                        }
                    }
                    cf::YUV_420 => {
                        cy |= (code & 0xf) << (block * 4);
                        cu |= ((code >> 4) & 1) << block;
                        cv |= ((code >> 5) & 1) << block;
                    }
                    cf::YUV_422 => {
                        cy |= (code & 0xf) << (block * 4);
                        const SHIFT: [i32; 4] = [0, 1, 4, 5];
                        for k in 0..2 {
                            if (code >> (k + 4)) & 1 != 0 {
                                let mut c = 5;
                                if r.get_bool() {
                                    c = 1;
                                } else if r.get_bool() {
                                    c = 4;
                                }
                                c <<= SHIFT[block];
                                if k == 0 {
                                    cu |= c;
                                } else {
                                    cv |= c;
                                }
                            }
                        }
                    }
                    _ => cy |= code << (block * 4),
                }
            }
            self.mb.diff_cbp[i] = cy;
            if matches!(cfm, cf::YUV_420 | cf::YUV_444 | cf::YUV_422) {
                self.mb.diff_cbp[1] = cu;
                self.mb.diff_cbp[2] = cv;
            }
        }
        Ok(())
    }

    /// jxrlib `predCBPDec` (with `predCBPCDec`, `predCBPC420Dec`, `predCBPC422Dec`).
    fn pred_cbp(&mut self) {
        let x = self.col;
        let cfm = self.cf;
        let sub = cfm == cf::YUV_420 || cfm == cf::YUV_422;
        let luma_like = if sub { 1 } else { self.channels };
        let (left, top) = (self.ctx_left, self.ctx_top);
        let model = &mut self.ctx[self.tile_col].cbp_model;
        for c in 0..luma_like {
            let c1 = usize::from(c > 0);
            let mut cbp = self.mb.diff_cbp[c];
            if model.state[c1] == 0 {
                if left {
                    if top {
                        cbp ^= 1;
                    } else {
                        cbp ^= (self.pred_prev[c][x].cbp >> 10) & 1;
                    }
                } else {
                    cbp ^= (self.pred_cur[c][x - 1].cbp >> 5) & 1;
                }
                cbp ^= 0x02 & (cbp << 1);
                cbp ^= 0x10 & (cbp << 3);
                cbp ^= 0x20 & (cbp << 1);
                cbp ^= (cbp & 0x33) << 2;
                cbp ^= (cbp & 0xcc) << 6;
                cbp ^= (cbp & 0x3300) << 2;
            } else if model.state[c1] == 2 {
                cbp ^= 0xffff;
            }
            cbp_state(model, c1, popcount16(cbp));
            self.mb.cbp[c] = cbp;
            self.pred_cur[c][x].cbp = cbp;
        }
        if sub {
            for c in 1..3 {
                let mut cbp = self.mb.diff_cbp[c];
                if model.state[1] == 0 {
                    let (top_bit, left_bit) = if cfm == cf::YUV_420 { (2, 1) } else { (6, 1) };
                    if left {
                        if top {
                            cbp ^= 1;
                        } else {
                            cbp ^= (self.pred_prev[c][x].cbp >> top_bit) & 1;
                        }
                    } else {
                        cbp ^= (self.pred_cur[c][x - 1].cbp >> left_bit) & 1;
                    }
                    if cfm == cf::YUV_420 {
                        cbp ^= 0x02 & (cbp << 1);
                        cbp ^= (cbp & 0x3) << 2;
                    } else {
                        cbp ^= (cbp & 0x1) << 1;
                        cbp ^= (cbp & 0x3) << 2;
                        cbp ^= (cbp & 0xc) << 2;
                        cbp ^= (cbp & 0x30) << 2;
                    }
                } else if model.state[1] == 2 {
                    cbp ^= if cfm == cf::YUV_420 { 0xf } else { 0xff };
                }
                let n = popcount16(cbp) * if cfm == cf::YUV_420 { 4 } else { 2 };
                cbp_state(model, 1, n);
                self.mb.cbp[c] = cbp;
                self.pred_cur[c][x].cbp = cbp;
            }
        }
    }

    /// jxrlib `DecodeCoeffs`.
    fn decode_coeffs(&mut self) -> Result<()> {
        let t = self.tile_col;
        let cfm = self.cf;
        let channels = self.channels;
        let planes = if cfm == cf::YUV_420 || cfm == cf::YUV_422 {
            1
        } else {
            channels
        };
        let p1: Vec<usize> = (0..channels).map(|c| self.p1(c)).collect();
        let ctx = &mut self.ctx[t];
        let tile = &self.tiles[t];
        let mut model_bits = ctx.model_ac.flc_bits[0];
        let mut lm = [0i32; 2];
        let mut plm = 0usize;
        let vertical = self.mb.orientation == 1;
        let mut chroma = false;
        let mut cbp = self.mb.cbp[0];
        let nblocks = match cfm {
            cf::YUV_420 => {
                cbp = cbp
                    .wrapping_add(self.mb.cbp[1] << 16)
                    .wrapping_add(self.mb.cbp[2] << 20);
                6
            }
            cf::YUV_422 => {
                cbp = cbp
                    .wrapping_add(self.mb.cbp[1] << 16)
                    .wrapping_add(self.mb.cbp[2] << 24);
                8
            }
            _ => 4,
        };
        let (io, fl) = (ctx.io[2], ctx.io[3]);
        let trim = ctx.trim;
        for i in 0..planes {
            let mut index = 0usize;
            for block in 0..nblocks {
                let qch = if planes > 1 {
                    i
                } else if block > 3 {
                    if cfm == cf::YUV_420 {
                        block - 3
                    } else {
                        block / 2 - 1
                    }
                } else {
                    0
                };
                let qp = tile
                    .hp
                    .get(qch)
                    .and_then(|v| v.get(self.mb.qidx_hp))
                    .ok_or_else(|| Error::decode("macroblock uses a missing highpass quantizer"))?
                    .qp;
                for sub in 0..4 {
                    let base = if block >= 4 {
                        if cfm == cf::YUV_420 {
                            p1[block - 3] + BLK_OFFSET_UV[sub]
                        } else {
                            let c = 1 + (1 & (block >> 1));
                            p1[c] + (block & 1) * 32 + BLK_OFFSET_UV422[sub]
                        }
                    } else {
                        p1[i] + BLK_OFFSET[index & 0xf]
                    };
                    let coef = &mut self.buf[..];
                    let scan = if vertical {
                        &mut ctx.scan_v
                    } else {
                        &mut ctx.scan_h
                    };
                    let mut flex = model_bits - trim;
                    if flex < 0 || self.skip_flex {
                        flex = 0;
                    }
                    let mut n = 0;
                    if cbp & 1 != 0 {
                        let qp1 = (W(qp) << model_bits).0;
                        n = decode_block_highpass(
                            chroma,
                            &mut ctx.ah,
                            &mut self.readers[io],
                            qp1,
                            coef,
                            base,
                            scan,
                        )?;
                    }
                    if flex != 0 {
                        flexbits(&mut self.readers[fl], coef, base, flex as u32, qp, trim);
                    }
                    if n > 16 {
                        return Err(Error::decode("too many highpass coefficients"));
                    }
                    lm[plm] += n as i32;
                    index += 1;
                    cbp >>= 1;
                }
                if block == 3 {
                    model_bits = ctx.model_ac.flc_bits[1];
                    plm = 1;
                    chroma = true;
                }
            }
            cbp = self.mb.cbp[(i + 1) & 15];
        }
        update_model(cfm, channels, lm, &mut ctx.model_ac);
        Ok(())
    }

    /// jxrlib `DecodeMacroblockHighpass`.
    fn decode_highpass(&mut self) -> Result<()> {
        let t = self.tile_col;
        if self.reset_rgi {
            let ctx = &mut self.ctx[t];
            reset_totals(&mut ctx.scan_h);
            reset_totals(&mut ctx.scan_v);
        }
        let (bits_hp, num_hp) = (self.tiles[t].bits_hp, self.tiles[t].num_qp_hp);
        if self.frequency && bits_hp > 0 {
            let io = self.ctx[t].io[2];
            self.mb.qidx_hp = Self::qp_index(&mut self.readers[io], bits_hp);
            if self.mb.qidx_hp >= num_hp {
                return Err(Error::decode(
                    "macroblock highpass quantizer index out of range",
                ));
            }
        } else if bits_hp == 0 && num_hp > 1 {
            self.mb.qidx_hp = self.mb.qidx_lp;
        }
        self.decode_cbp()?;
        self.pred_cbp();
        self.decode_coeffs()?;
        if self.reset_ctx {
            self.ctx[t].adapt_highpass();
        }
        Ok(())
    }

    /// jxrlib `predACDec`.
    fn pred_ac(&mut self) {
        let mode = 2 - self.mb.orientation;
        let cfm = self.cf;
        let sub = cfm == cf::YUV_420 || cfm == cf::YUV_422;
        let luma_like = if sub { 1 } else { self.channels };
        let add = |d: &mut [i32], a: usize, b: usize| d[a] = d[a].wrapping_add(d[b]);
        for i in 0..luma_like {
            let s = self.p1(i);
            let d = &mut self.buf;
            match mode {
                1 => {
                    for b in [1usize, 2, 3, 5, 6, 7, 9, 10, 11, 13, 14, 15] {
                        let o = s + 16 * b;
                        for k in [2, 10, 9] {
                            add(d, o + k, o + k - 16);
                        }
                    }
                }
                0 => {
                    for j in (64..256).step_by(16) {
                        let o = s + j;
                        for k in [1, 5, 6] {
                            add(d, o + k, o + k - 64);
                        }
                    }
                }
                _ => {}
            }
        }
        if cfm == cf::YUV_420 {
            for c in 1..3 {
                let s = self.p1(c);
                let d = &mut self.buf;
                match mode {
                    1 => {
                        for j in [1usize, 3] {
                            let o = s + 16 * j;
                            for k in [2, 10, 9] {
                                add(d, o + k, o + k - 16);
                            }
                        }
                    }
                    0 => {
                        for j in [2usize, 3] {
                            let o = s + 16 * j;
                            for k in [1, 5, 6] {
                                add(d, o + k, o + k - 32);
                            }
                        }
                    }
                    _ => {}
                }
            }
        } else if cfm == cf::YUV_422 {
            for c in 1..3 {
                let s = self.p1(c);
                let d = &mut self.buf;
                match mode {
                    1 => {
                        for &off in &BLK_OFFSET_UV422[2..8] {
                            let o = s + off;
                            for k in [10, 2, 9] {
                                add(d, o + k, o + k - 16);
                            }
                        }
                    }
                    0 => {
                        for j in [1usize, 3, 5, 7] {
                            let o = s + BLK_OFFSET_UV422[j];
                            for k in [1, 5, 6] {
                                add(d, o + k, o + k - 64);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// jxrlib `updatePredInfo`.
    fn update_pred_info(&mut self) {
        let x = self.col;
        let q = self.mb.qidx_lp as i32;
        let sub = self.cf == cf::YUV_420 || self.cf == cf::YUV_422;
        let luma_like = if sub { 1 } else { self.channels };
        for i in 0..luma_like {
            let p = &self.mb.block_dc[i];
            let pi = &mut self.pred_cur[i][x];
            pi.dc = p[0];
            pi.qp_index = q;
            pi.ad = [p[1], p[2], p[3], p[4], p[8], p[12]];
        }
        if self.cf == cf::YUV_420 {
            for i in 1..3 {
                let p = self.mb.block_dc[i];
                let pi = &mut self.pred_cur[i][x];
                pi.dc = p[0];
                pi.qp_index = q;
                pi.ad[0] = p[1];
                pi.ad[1] = p[2];
            }
        } else if self.cf == cf::YUV_422 {
            for i in 1..3 {
                let p = self.mb.block_dc[i];
                let pi = &mut self.pred_cur[i][x];
                pi.qp_index = q;
                pi.dc = p[0];
                pi.ad[0] = p[1];
                pi.ad[1] = p[2];
                pi.ad[2] = p[5];
                pi.ad[3] = p[6];
                pi.ad[4] = p[4];
            }
        }
    }

    /// Entropy-decode, predict and dequantize the macroblock at (`col`, `row`).
    fn decode_macroblock(&mut self) -> Result<()> {
        self.tile_pos();
        self.read_packets()?;
        self.decode_dc()?;
        if self.decode_lp {
            self.decode_lowpass()?;
        }
        self.pred_dcac();
        self.dequantize()?;
        if self.decode_hp {
            self.decode_highpass()?;
            self.pred_ac();
        }
        self.update_pred_info();
        if self.readers.iter().any(BitReader::overrun) {
            return Err(Error::decode("codestream is truncated"));
        }
        Ok(())
    }

    fn transform(&mut self) {
        let prev = 1 - self.cur_region;
        let p0: Vec<isize> = (0..self.channels)
            .map(|c| (self.base(c, prev) + self.col * self.mb_size[c]) as isize)
            .collect();
        let p1: Vec<isize> = (0..self.channels).map(|c| self.p1(c) as isize).collect();
        let setup = Setup {
            cf: self.cf,
            channels: self.channels,
            overlap: self.img.overlap,
            scaled: self.pl.scaled,
            hp_absent: self.pl.subband == sb::NO_HIGHPASS || self.pl.subband == sb::DC_ONLY,
        };
        let edges = Edges {
            left: self.col == 0,
            right: self.col == self.mb_w,
            top: self.row == 0,
            bottom: self.row == self.mb_h,
            column: self.col,
            row: self.row,
            mb_width: self.mb_w,
        };
        if self.img.subversion == SUBVERSION_ORIGINAL {
            let mut hpqp = [255i32; MAX_CHANNELS];
            if !setup.hp_absent {
                let t = &self.tiles[self.tile_col];
                for (i, q) in hpqp.iter_mut().enumerate().take(self.channels) {
                    if let Some(v) = t.hp.get(i).and_then(|v| v.get(self.mb.qidx_hp)) {
                        *q = v.qp;
                    }
                }
            }
            transform::inv_transform_original(&mut self.buf, &p0, &p1, setup, edges, &hpqp);
        } else {
            let tiles = Tiles {
                hard: self.img.hard_tiles,
                tile_x: &self.img.tile_x,
                tile_y: &self.img.tile_y,
            };
            transform::inv_transform_altered(
                &mut self.buf,
                &p0,
                &p1,
                setup,
                edges,
                &mut self.hard,
                tiles,
            );
        }
    }

    /// Decode every macroblock row and hand each finished row to the output stage.
    pub(crate) fn run(&mut self, out: &mut Output) -> Result<()> {
        for row in 0..=self.mb_h {
            self.row = row;
            let cur = self.cur_region;
            for c in 0..self.channels {
                let start = self.base(c, cur);
                let len = self.mb_w * self.mb_size[c];
                self.buf[start..start + len].fill(0);
            }
            for col in 0..=self.mb_w {
                self.col = col;
                if col < self.mb_w && row < self.mb_h {
                    self.decode_macroblock()?;
                }
                self.transform();
            }
            if row > 0 {
                let prev = 1 - self.cur_region;
                let prev_base: Vec<usize> =
                    (0..self.channels).map(|c| self.base(c, prev)).collect();
                let cur_base: Vec<usize> = (0..self.channels)
                    .map(|c| self.base(c, self.cur_region))
                    .collect();
                output_row(
                    out,
                    &self.buf,
                    &prev_base,
                    &cur_base,
                    row - 1,
                    row == self.mb_h,
                )?;
            }
            std::mem::swap(&mut self.pred_cur, &mut self.pred_prev);
            self.cur_region = 1 - self.cur_region;
        }
        Ok(())
    }
}
