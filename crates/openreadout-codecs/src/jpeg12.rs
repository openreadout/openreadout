//! Sequential DCT JPEG with 12-bit samples (ITU-T T.81 extended process, `SOF1`, Huffman
//! coding), which `jpeg-decoder` does not implement. Written from the T.81 recommendation
//! (Annex A: the DCT and level shift; Annex B: the syntax; Annex F: sequential Huffman
//! decoding), not from another decoder.
//!
//! Scope: one or three components without chroma subsampling, interleaved or not,
//! restart intervals. Progressive, arithmetic-coded and hierarchical 12-bit streams and
//! subsampled colour are refused with [`CodecError::Unsupported`]. The inverse DCT is the
//! exact separable transform of T.81 A.3.3 in double precision, rounded to the nearest
//! integer; libjpeg's integer transform can differ from it by about one grey level.

use crate::{CodecError, JpegColor, Raster, Result};

const CODEC: &str = "jpeg";

/// Zig-zag index → natural (row-major) index of an 8×8 block (T.81 Figure A.6).
const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

fn bad(detail: impl Into<String>) -> CodecError {
    CodecError::Decode {
        codec: CODEC,
        detail: detail.into(),
    }
}

fn unsupported(detail: impl Into<String>) -> CodecError {
    CodecError::Unsupported {
        codec: CODEC,
        detail: detail.into(),
    }
}

/// The sample precision of a stream's frame header (`SOF`), read without decoding.
/// `None` when no frame header precedes the first scan.
pub(crate) fn frame_precision(data: &[u8]) -> Option<u8> {
    let mut i = 2usize;
    if data.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    loop {
        if *data.get(i)? != 0xFF {
            return None;
        }
        let mk = *data.get(i + 1)?;
        if mk == 0xFF {
            i += 1;
            continue;
        }
        if mk == 0xD8 || mk == 0x01 || (0xD0..=0xD7).contains(&mk) {
            i += 2;
            continue;
        }
        if mk == 0xDA || mk == 0xD9 {
            return None;
        }
        let len = usize::from(u16::from_be_bytes([*data.get(i + 2)?, *data.get(i + 3)?]));
        if (0xC0..=0xCF).contains(&mk) && !matches!(mk, 0xC4 | 0xC8 | 0xCC) {
            return data.get(i + 4).copied();
        }
        i = i.checked_add(2 + len)?;
    }
}

/// A Huffman table in the form of T.81 F.2.2.3 (`MINCODE`, `MAXCODE`, `VALPTR`), with a
/// 9-bit lookup for the short codes.
#[derive(Clone)]
struct Huffman {
    mincode: [i32; 17],
    maxcode: [i32; 18],
    valptr: [i32; 17],
    values: Vec<u8>,
    /// For each 9-bit prefix: (code length, value), length 0 when the code is longer.
    fast: Vec<(u8, u8)>,
}

impl Huffman {
    fn new(counts: &[u8; 16], values: &[u8]) -> Result<Self> {
        let mut mincode = [0i32; 17];
        let mut maxcode = [-1i32; 18];
        let mut valptr = [0i32; 17];
        let mut code = 0i32;
        let mut k = 0i32;
        let mut fast = vec![(0u8, 0u8); 512];
        for l in 1..=16usize {
            let n = i32::from(counts[l - 1]);
            if n > 0 {
                valptr[l] = k;
                mincode[l] = code;
                for j in 0..n {
                    let c = code + j;
                    if l <= 9 {
                        let v = *values
                            .get((k + j) as usize)
                            .ok_or_else(|| bad("Huffman table has fewer values than codes"))?;
                        let shift = 9 - l;
                        let base = (c as usize) << shift;
                        for e in fast
                            .get_mut(base..base + (1 << shift))
                            .into_iter()
                            .flatten()
                        {
                            *e = (l as u8, v);
                        }
                    }
                }
                code += n;
                k += n;
                maxcode[l] = code - 1;
            }
            if code > (1 << l) {
                return Err(bad("Huffman table code lengths overflow"));
            }
            code <<= 1;
        }
        if (k as usize) > values.len() {
            return Err(bad("Huffman table has fewer values than codes"));
        }
        maxcode[17] = i32::MAX;
        Ok(Self {
            mincode,
            maxcode,
            valptr,
            values: values.to_vec(),
            fast,
        })
    }
}

/// Entropy-coded data with byte stuffing (`FF 00`) removed on the fly. Past a marker or
/// the end of the segment, zeros are supplied; the MCU count bounds the work.
struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
    acc: u64,
    n: u32,
}

impl<'a> Bits<'a> {
    fn new(d: &'a [u8]) -> Self {
        Self {
            d,
            pos: 0,
            acc: 0,
            n: 0,
        }
    }

    fn fill(&mut self) {
        while self.n <= 56 {
            let mut b = 0u8;
            if let Some(&x) = self.d.get(self.pos) {
                if x == 0xFF {
                    if self.d.get(self.pos + 1) == Some(&0) {
                        self.pos += 2;
                        b = 0xFF;
                    }
                    // Otherwise a marker: leave `pos` on it and feed zeros.
                } else {
                    self.pos += 1;
                    b = x;
                }
            }
            self.acc |= u64::from(b) << (56 - self.n);
            self.n += 8;
        }
    }

    fn peek(&mut self, k: u32) -> u32 {
        if self.n < k {
            self.fill();
        }
        (self.acc >> (64 - k)) as u32
    }

    fn skip(&mut self, k: u32) {
        self.acc <<= k;
        self.n -= k;
    }

    fn bits(&mut self, k: u32) -> u32 {
        if k == 0 {
            return 0;
        }
        let v = self.peek(k);
        self.skip(k);
        v
    }

    fn decode(&mut self, table: &Huffman) -> Result<u8> {
        let peeked = self.peek(16);
        let (len, value) = table.fast[(peeked >> 7) as usize];
        if len > 0 {
            self.skip(u32::from(len));
            return Ok(value);
        }
        for len in 10..=16usize {
            let code = (peeked >> (16 - len)) as i32;
            if code <= table.maxcode[len] {
                self.skip(len as u32);
                let at = table.valptr[len] + code - table.mincode[len];
                return table
                    .values
                    .get(at as usize)
                    .copied()
                    .ok_or_else(|| bad("Huffman code outside its table"));
            }
        }
        Err(bad("invalid Huffman code"))
    }

    /// `RECEIVE` and `EXTEND` of T.81 F.2.2.1.
    fn receive_extend(&mut self, s: u8) -> Result<i32> {
        if s == 0 {
            return Ok(0);
        }
        if s > 16 {
            return Err(bad(format!("magnitude category {s}")));
        }
        let s = u32::from(s);
        let v = self.bits(s) as i32;
        Ok(if v < (1 << (s - 1)) {
            v - (1 << s) + 1
        } else {
            v
        })
    }

    /// Discard buffered bits and step over the next `RST` marker.
    fn restart(&mut self) -> Result<()> {
        self.acc = 0;
        self.n = 0;
        while self.pos + 1 < self.d.len() {
            if self.d[self.pos] == 0xFF && (0xD0..=0xD7).contains(&self.d[self.pos + 1]) {
                self.pos += 2;
                return Ok(());
            }
            self.pos += 1;
        }
        Err(bad("restart marker missing"))
    }
}

struct Component {
    id: u8,
    h: usize,
    v: usize,
    tq: usize,
    /// Blocks per line and per column, padded to whole MCUs.
    bw: usize,
    bh: usize,
    samples: Vec<i32>,
    dc_pred: i32,
}

/// Cosine table of the inverse DCT: `C(u)/2 · cos((2x+1)uπ/16)` at `[x][u]`.
fn idct_table() -> [[f64; 8]; 8] {
    let mut t = [[0.0; 8]; 8];
    for (x, row) in t.iter_mut().enumerate() {
        for (u, c) in row.iter_mut().enumerate() {
            let cu = if u == 0 {
                std::f64::consts::FRAC_1_SQRT_2
            } else {
                1.0
            };
            *c = cu / 2.0 * ((2.0 * x as f64 + 1.0) * u as f64 * std::f64::consts::PI / 16.0).cos();
        }
    }
    t
}

/// Decode a 12-bit sequential DCT stream. `color` as in `jpeg_decode_tiff` (`None`: the
/// stream's markers decide, as libjpeg does). Samples come back as little-endian u16.
pub(crate) fn decode(data: &[u8], color: Option<JpegColor>, max_bytes: usize) -> Result<Raster> {
    if data.get(..2) != Some(&[0xFF, 0xD8]) {
        return Err(bad("stream lacks an SOI marker"));
    }
    let markers = crate::jpeg_markers(data, None)?;
    let mut qt = [[0i32; 64]; 4];
    let mut dc: [Option<Huffman>; 4] = [None, None, None, None];
    let mut ac: [Option<Huffman>; 4] = [None, None, None, None];
    let mut comps: Vec<Component> = Vec::new();
    let (mut width, mut height, mut precision) = (0usize, 0usize, 0u8);
    let (mut hmax, mut vmax) = (1usize, 1usize);
    let mut restart = 0usize;
    let mut scans = 0usize;
    let cos = idct_table();
    let mut i = 2usize;
    loop {
        let Some(&byte) = data.get(i) else {
            if scans > 0 {
                break; // a missing EOI after the last scan is tolerated
            }
            return Err(bad("stream ends before its first scan"));
        };
        if byte != 0xFF {
            return Err(bad(format!("expected a marker at byte {i}")));
        }
        let mk = *data.get(i + 1).ok_or_else(|| bad("truncated marker"))?;
        if mk == 0xFF {
            i += 1;
            continue;
        }
        if mk == 0xD9 {
            break;
        }
        if mk == 0xD8 || mk == 0x01 || (0xD0..=0xD7).contains(&mk) {
            i += 2;
            continue;
        }
        let len = data
            .get(i + 2..i + 4)
            .map(|l| usize::from(u16::from_be_bytes([l[0], l[1]])))
            .ok_or_else(|| bad("truncated segment length"))?;
        let seg = len
            .checked_sub(2)
            .and_then(|n| data.get(i + 4..i + 4 + n))
            .ok_or_else(|| bad("segment runs past the end of the stream"))?;
        match mk {
            0xDB => {
                let mut p = 0usize;
                while p < seg.len() {
                    let pq = seg[p] >> 4;
                    let tq = usize::from(seg[p] & 15);
                    if tq > 3 || pq > 1 {
                        return Err(bad("invalid quantization table header"));
                    }
                    p += 1;
                    let w = if pq == 1 { 2 } else { 1 };
                    let body = seg
                        .get(p..p + 64 * w)
                        .ok_or_else(|| bad("truncated quantization table"))?;
                    for (k, &nat) in ZIGZAG.iter().enumerate() {
                        qt[tq][nat] = if pq == 1 {
                            i32::from(u16::from_be_bytes([body[2 * k], body[2 * k + 1]]))
                        } else {
                            i32::from(body[k])
                        };
                    }
                    p += 64 * w;
                }
            }
            0xC4 => {
                let mut p = 0usize;
                while p < seg.len() {
                    let tc = seg[p] >> 4;
                    let th = usize::from(seg[p] & 15);
                    if tc > 1 || th > 3 {
                        return Err(bad("invalid Huffman table header"));
                    }
                    let counts: [u8; 16] = seg
                        .get(p + 1..p + 17)
                        .and_then(|c| c.try_into().ok())
                        .ok_or_else(|| bad("truncated Huffman table"))?;
                    let n: usize = counts.iter().map(|&c| usize::from(c)).sum();
                    let vals = seg
                        .get(p + 17..p + 17 + n)
                        .ok_or_else(|| bad("truncated Huffman table"))?;
                    let t = Huffman::new(&counts, vals)?;
                    if tc == 0 {
                        dc[th] = Some(t);
                    } else {
                        ac[th] = Some(t);
                    }
                    p += 17 + n;
                }
            }
            0xDD => {
                restart = usize::from(u16::from_be_bytes([
                    *seg.first().ok_or_else(|| bad("truncated DRI"))?,
                    *seg.get(1).ok_or_else(|| bad("truncated DRI"))?,
                ]));
            }
            0xC0 | 0xC1 => {
                if !comps.is_empty() {
                    return Err(bad("more than one frame header"));
                }
                if seg.len() < 6 {
                    return Err(bad("truncated frame header"));
                }
                precision = seg[0];
                if precision != 12 && precision != 8 {
                    return Err(unsupported(format!("{precision}-bit DCT samples")));
                }
                height = usize::from(u16::from_be_bytes([seg[1], seg[2]]));
                width = usize::from(u16::from_be_bytes([seg[3], seg[4]]));
                if height == 0 {
                    return Err(unsupported("a frame height set by a DNL marker"));
                }
                if width == 0 {
                    return Err(bad("frame width 0"));
                }
                let nf = usize::from(seg[5]);
                if nf != 1 && nf != 3 {
                    return Err(unsupported(format!("a {nf}-component 12-bit stream")));
                }
                let spec = seg
                    .get(6..6 + 3 * nf)
                    .ok_or_else(|| bad("truncated frame header"))?;
                let bytes = (width as u64) * (height as u64) * (nf as u64) * 2;
                if bytes > max_bytes as u64 {
                    return Err(bad(format!(
                        "frame header declares {width} x {height} pixels ({bytes} bytes decoded)"
                    )));
                }
                for c in spec.as_chunks::<3>().0 {
                    let (h, v) = (usize::from(c[1] >> 4), usize::from(c[1] & 15));
                    if !(1..=4).contains(&h) || !(1..=4).contains(&v) || c[2] > 3 {
                        return Err(bad("invalid component specification"));
                    }
                    hmax = hmax.max(h);
                    vmax = vmax.max(v);
                    comps.push(Component {
                        id: c[0],
                        h,
                        v,
                        tq: usize::from(c[2]),
                        bw: 0,
                        bh: 0,
                        samples: Vec::new(),
                        dc_pred: 0,
                    });
                }
                if comps.iter().any(|c| c.h != hmax || c.v != vmax) {
                    return Err(unsupported("12-bit JPEG with chroma subsampling"));
                }
                let mcux = width.div_ceil(8 * hmax);
                let mcuy = height.div_ceil(8 * vmax);
                for c in &mut comps {
                    c.bw = mcux * c.h;
                    c.bh = mcuy * c.v;
                    c.samples = vec![0; c.bw * 8 * c.bh * 8];
                }
            }
            0xC2 | 0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                return Err(unsupported(format!(
                    "12-bit JPEG frame type SOF{} (only sequential Huffman frames are decoded)",
                    mk - 0xC0
                )));
            }
            0xDA => {
                if comps.is_empty() {
                    return Err(bad("scan before the frame header"));
                }
                let ns = usize::from(*seg.first().ok_or_else(|| bad("truncated scan header"))?);
                let spec = seg
                    .get(1..1 + 2 * ns)
                    .ok_or_else(|| bad("truncated scan header"))?;
                let tail = seg
                    .get(1 + 2 * ns..1 + 2 * ns + 3)
                    .ok_or_else(|| bad("truncated scan header"))?;
                if tail[0] != 0 || tail[1] != 63 || tail[2] != 0 {
                    return Err(bad("sequential scan with spectral selection"));
                }
                let mut members = Vec::with_capacity(ns);
                for s in spec.as_chunks::<2>().0 {
                    let ci = comps
                        .iter()
                        .position(|c| c.id == s[0])
                        .ok_or_else(|| bad("scan names an unknown component"))?;
                    let (td, ta) = (usize::from(s[1] >> 4), usize::from(s[1] & 15));
                    if td > 3 || ta > 3 {
                        return Err(bad("invalid Huffman table selector"));
                    }
                    members.push((ci, td, ta));
                }
                if members.is_empty() {
                    return Err(bad("scan without components"));
                }
                // The entropy-coded segment runs to the first marker that is not RSTn.
                let start = i + 2 + len;
                let mut end = start;
                while end + 1 < data.len() {
                    if data[end] == 0xFF
                        && data[end + 1] != 0
                        && !(0xD0..=0xD7).contains(&data[end + 1])
                    {
                        break;
                    }
                    end += 1;
                }
                if end + 1 >= data.len() {
                    end = data.len();
                }
                scan(
                    &data[start..end],
                    &mut comps,
                    &members,
                    &dc,
                    &ac,
                    &qt,
                    restart,
                    width,
                    height,
                    (hmax, vmax),
                    precision,
                    &cos,
                )?;
                scans += 1;
                i = end;
                continue;
            }
            _ => {}
        }
        i += 2 + len;
    }
    if scans == 0 {
        return Err(bad("no scan"));
    }
    let channels = comps.len();
    let max = (1i32 << precision) - 1;
    let to_rgb = channels == 3
        && match color {
            Some(JpegColor::ToRgb) => true,
            Some(JpegColor::AsCoded) => false,
            None => markers.coded_color() == Some(JpegColor::ToRgb),
        };
    let mut out = Vec::with_capacity(width * height * channels * 2);
    let half = f64::from(1i32 << (precision - 1));
    for y in 0..height {
        for x in 0..width {
            if to_rgb {
                let at = |c: &Component| f64::from(c.samples[y * c.bw * 8 + x]);
                let (yy, cb, cr) = (at(&comps[0]), at(&comps[1]) - half, at(&comps[2]) - half);
                // T.81 does not define colour; JFIF (ITU-T T.871) does.
                let red = yy + (1.402 * cr + 0.5).floor();
                let green = yy + (-0.344_136_286 * cb - 0.714_136_286 * cr + 0.5).floor();
                let blue = yy + (1.772 * cb + 0.5).floor();
                for v in [red, green, blue] {
                    let s = (v as i32).clamp(0, max) as u16;
                    out.extend_from_slice(&s.to_le_bytes());
                }
            } else {
                for c in &comps {
                    let s = c.samples[y * c.bw * 8 + x].clamp(0, max) as u16;
                    out.extend_from_slice(&s.to_le_bytes());
                }
            }
        }
    }
    Ok(Raster {
        width: width as u32,
        height: height as u32,
        channels: channels as u32,
        bits_per_sample: if precision > 8 { 16 } else { 8 },
        float: false,
        bgr: false,
        data: if precision > 8 {
            out
        } else {
            out.as_chunks::<2>().0.iter().map(|c| c[0]).collect()
        },
    })
}

/// The inverse DCT of one dequantized block (natural order): rows, then columns,
/// `s(y,x) = Σv Σu c[y][v] c[x][u] S(v,u)` with `c` from [`idct_table`].
fn idct(cos: &[[f64; 8]; 8], block: &[f64; 64]) -> [f64; 64] {
    let mut tmp = [0f64; 64];
    for (row_in, row_out) in block
        .as_chunks::<8>()
        .0
        .iter()
        .zip(tmp.as_chunks_mut::<8>().0.iter_mut())
    {
        for (out, basis) in row_out.iter_mut().zip(cos) {
            *out = basis.iter().zip(row_in).map(|(c, s)| c * s).sum();
        }
    }
    let mut out = [0f64; 64];
    for (y, basis) in cos.iter().enumerate() {
        for x in 0..8 {
            out[y * 8 + x] = basis
                .iter()
                .enumerate()
                .map(|(v, c)| c * tmp[v * 8 + x])
                .sum();
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn scan(
    seg: &[u8],
    comps: &mut [Component],
    members: &[(usize, usize, usize)],
    dc: &[Option<Huffman>; 4],
    ac: &[Option<Huffman>; 4],
    qt: &[[i32; 64]; 4],
    restart: usize,
    width: usize,
    height: usize,
    (hmax, vmax): (usize, usize),
    precision: u8,
    cos: &[[f64; 8]; 8],
) -> Result<()> {
    let mut bits = Bits::new(seg);
    for &(ci, _, _) in members {
        comps[ci].dc_pred = 0;
    }
    // A non-interleaved scan codes one component's blocks in raster order, covering only
    // the blocks the image needs (T.81 A.2.2); an interleaved scan codes whole MCUs.
    let (units_x, units_y) = if members.len() == 1 {
        let c = &comps[members[0].0];
        let cw = (width * c.h).div_ceil(hmax);
        let ch = (height * c.v).div_ceil(vmax);
        (cw.div_ceil(8), ch.div_ceil(8))
    } else {
        (width.div_ceil(8 * hmax), height.div_ceil(8 * vmax))
    };
    let shift = 1i32 << (precision - 1);
    let max = (1i32 << precision) - 1;
    let mut zz = [0i32; 64];
    let mut block = [0f64; 64];
    let total = units_x * units_y;
    for unit in 0..total {
        if restart > 0 && unit > 0 && unit % restart == 0 {
            bits.restart()?;
            for &(ci, _, _) in members {
                comps[ci].dc_pred = 0;
            }
        }
        let (ux, uy) = (unit % units_x, unit / units_x);
        for &(ci, td, ta) in members {
            let (nh, nv) = if members.len() == 1 {
                (1, 1)
            } else {
                (comps[ci].h, comps[ci].v)
            };
            for by in 0..nv {
                for bx in 0..nh {
                    let dct = dc[td]
                        .as_ref()
                        .ok_or_else(|| bad("scan uses an undefined DC table"))?;
                    let act = ac[ta]
                        .as_ref()
                        .ok_or_else(|| bad("scan uses an undefined AC table"))?;
                    zz.fill(0);
                    let category = bits.decode(dct)?;
                    let diff = bits.receive_extend(category)?;
                    let comp = &mut comps[ci];
                    comp.dc_pred = comp.dc_pred.wrapping_add(diff);
                    zz[0] = comp.dc_pred;
                    let mut k = 1usize;
                    while k < 64 {
                        let rs = bits.decode(act)?;
                        let (run, size) = (usize::from(rs >> 4), rs & 15);
                        if size == 0 {
                            if run == 15 {
                                k += 16;
                                continue;
                            }
                            break;
                        }
                        k += run;
                        if k > 63 {
                            return Err(bad("AC coefficient index past 63"));
                        }
                        zz[k] = bits.receive_extend(size)?;
                        k += 1;
                    }
                    let quant = &qt[comp.tq];
                    for (k, &nat) in ZIGZAG.iter().enumerate() {
                        block[nat] = f64::from(zz[k].saturating_mul(quant[nat]));
                    }
                    let spatial = idct(cos, &block);
                    let (gx, gy) = if members.len() == 1 {
                        (ux, uy)
                    } else {
                        (ux * comp.h + bx, uy * comp.v + by)
                    };
                    let stride = comp.bw * 8;
                    for (y, line) in spatial.as_chunks::<8>().0.iter().enumerate() {
                        let row = (gy * 8 + y) * stride + gx * 8;
                        for (x, value) in line.iter().enumerate() {
                            let sample = (value.round() as i32).saturating_add(shift).clamp(0, max);
                            if let Some(d) = comp.samples.get_mut(row + x) {
                                *d = sample;
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idct_of_a_dc_block_is_flat() {
        let t = idct_table();
        // DC coefficient 8·d gives d everywhere: C(0)²/4 · 8d = d.
        let mut s = 0.0;
        for v in 0..8 {
            for u in 0..8 {
                s += t[3][u] * t[5][v] * if u == 0 && v == 0 { 80.0 } else { 0.0 };
            }
        }
        assert!((s - 10.0).abs() < 1e-9);
    }

    #[test]
    fn precision_is_read_from_the_frame_header() {
        let s = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0, 0, 0xFF, 0xC1, 0x00, 0x0B, 12, 0, 8, 0, 8, 1, 1,
            0x11, 0,
        ];
        assert_eq!(frame_precision(&s), Some(12));
        assert_eq!(frame_precision(&s[..10]), None);
        assert_eq!(frame_precision(b"not a jpeg"), None);
    }

    #[test]
    fn garbage_is_refused_cleanly() {
        for s in [
            &[0xFF, 0xD8][..],
            &[
                0xFF, 0xD8, 0xFF, 0xC1, 0x00, 0x0B, 12, 0, 8, 0, 8, 1, 1, 0x11, 0,
            ][..],
            &[
                0xFF, 0xD8, 0xFF, 0xC1, 0x00, 0x0B, 12, 0xFF, 0xFF, 0xFF, 0xFF, 1, 1, 0x11, 0,
            ][..],
            &[0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x02][..],
        ] {
            assert!(decode(s, None, 1 << 20).is_err());
        }
    }
}
