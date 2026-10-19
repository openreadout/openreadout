//! The adaptive arithmetic (range) coder of CRAM 3.1 (block method 6) and the frequency models
//! it shares with fqzcomp. Written from the CRAM codecs specification (`CRAMcodecs.tex` § 4,
//! hts-specs).

use super::io::{Cursor, DecodeResult};
use super::rans::{MAX_OUT, unpack};

/// The range decoder.
pub(crate) struct RangeDecoder<'a> {
    r: Cursor<'a>,
    range: u32,
    code: u32,
}

impl<'a> RangeDecoder<'a> {
    pub(crate) fn new(mut r: Cursor<'a>) -> DecodeResult<Self> {
        let mut code: u64 = 0;
        for _ in 0..5 {
            code = (code << 8) | u64::from(r.u8()?);
        }
        Ok(RangeDecoder {
            r,
            range: u32::MAX,
            code: code as u32,
        })
    }

    fn get_freq(&mut self, tot: u32) -> DecodeResult<u32> {
        self.range /= tot.max(1);
        if self.range == 0 {
            return Err("range coder state collapsed".into());
        }
        Ok(self.code / self.range)
    }

    fn decode(&mut self, low: u32, freq: u32) -> DecodeResult<()> {
        self.code = self.code.wrapping_sub(low.wrapping_mul(self.range));
        self.range = self.range.wrapping_mul(freq);
        while self.range < (1 << 24) {
            if self.range == 0 {
                return Err("range coder state collapsed".into());
            }
            self.range <<= 8;
            // past the end the stream reads as zeros, as the encoder flushed its state
            let b = if self.r.is_empty() { 0 } else { self.r.u8()? };
            self.code = (self.code << 8) | u32::from(b);
        }
        Ok(())
    }
}

/// An adaptive frequency model of `n` symbols (CRAMcodecs `ModelCreate`, `ModelDecode`).
#[derive(Clone)]
pub(crate) struct Model {
    s: Vec<u16>,
    f: Vec<u32>,
    total: u32,
}

impl Model {
    pub(crate) fn new(n: usize) -> Self {
        Model {
            s: (0..n).map(|i| i as u16).collect(),
            f: vec![1; n],
            total: n as u32,
        }
    }

    pub(crate) fn decode(&mut self, rc: &mut RangeDecoder) -> DecodeResult<u16> {
        let freq = rc.get_freq(self.total)?;
        let mut acc = 0u32;
        let mut x = 0usize;
        while x < self.f.len() && acc + self.f[x] <= freq {
            acc += self.f[x];
            x += 1;
        }
        if x >= self.f.len() {
            return Err("arithmetic code outside the model".into());
        }
        rc.decode(acc, self.f[x])?;
        self.f[x] += 16;
        self.total += 16;
        if self.total > (1 << 16) - 17 {
            self.total = 0;
            for v in &mut self.f {
                *v -= *v / 2;
                self.total += *v;
            }
        }
        let sym = self.s[x];
        if x > 0 && self.f[x] > self.f[x - 1] {
            self.f.swap(x, x - 1);
            self.s.swap(x, x - 1);
        }
        Ok(sym)
    }
}

fn max_sym(r: &mut Cursor) -> DecodeResult<usize> {
    let m = r.u8()? as usize;
    Ok(if m == 0 { 256 } else { m })
}

fn order0(r: &mut Cursor, len: usize) -> DecodeResult<Vec<u8>> {
    let m = max_sym(r)?;
    let mut lit = Model::new(m);
    let mut rc = RangeDecoder::new(r.clone())?;
    let mut out = Vec::with_capacity(len.min(16 << 20));
    for _ in 0..len {
        out.push(lit.decode(&mut rc)? as u8);
    }
    Ok(out)
}

fn order1(r: &mut Cursor, len: usize) -> DecodeResult<Vec<u8>> {
    let m = max_sym(r)?;
    let mut lit = vec![Model::new(m); m];
    let mut rc = RangeDecoder::new(r.clone())?;
    let mut out = Vec::with_capacity(len.min(16 << 20));
    let mut last = 0usize;
    for _ in 0..len {
        let model = lit
            .get_mut(last)
            .ok_or_else(|| "arithmetic order-1 context outside the alphabet".to_string())?;
        let s = model.decode(&mut rc)? as u8;
        out.push(s);
        last = s as usize;
    }
    Ok(out)
}

fn rle(r: &mut Cursor, len: usize, order1: bool) -> DecodeResult<Vec<u8>> {
    let m = max_sym(r)?;
    let mut lit = vec![Model::new(m); if order1 { m } else { 1 }];
    let mut runs = vec![Model::new(4); 258];
    let mut rc = RangeDecoder::new(r.clone())?;
    let mut out = Vec::with_capacity(len.min(16 << 20));
    let mut last = 0usize;
    while out.len() < len {
        let ctx = if order1 { last } else { 0 };
        let model = lit
            .get_mut(ctx)
            .ok_or_else(|| "arithmetic order-1 context outside the alphabet".to_string())?;
        let s = model.decode(&mut rc)? as u8;
        last = s as usize;
        let mut part = runs[s as usize].decode(&mut rc)? as usize;
        let mut run = part;
        let mut rctx = 256;
        while part == 3 {
            part = runs[rctx].decode(&mut rc)? as usize;
            rctx = 257;
            run += part;
            if run > len {
                return Err("arithmetic run longer than the block".into());
            }
        }
        if out.len() + run + 1 > len {
            return Err("arithmetic run longer than the block".into());
        }
        out.extend(std::iter::repeat_n(s, run + 1));
    }
    Ok(out)
}

const ORDER: u8 = 1;
const EXT: u8 = 4;
const STRIPE: u8 = 8;
const NOSIZE: u8 = 16;
const CAT: u8 = 32;
const RLE: u8 = 64;
const PACK: u8 = 128;

/// The adaptive arithmetic codec (block method 6).
pub(crate) fn decode(data: &[u8], len: Option<usize>) -> DecodeResult<Vec<u8>> {
    decode_at(&mut Cursor::new(data), len, 0)
}

fn decode_at(r: &mut Cursor, len: Option<usize>, depth: u32) -> DecodeResult<Vec<u8>> {
    if depth > 4 {
        return Err("arithmetic stripes nested too deep".into());
    }
    let flags = r.u8()?;
    let mut len = if flags & NOSIZE == 0 {
        r.uint7_len(MAX_OUT, "arithmetic block")?
    } else {
        len.ok_or_else(|| "arithmetic stream without a size".to_string())?
    };
    if flags & STRIPE != 0 {
        let n = r.u8()? as usize;
        if n == 0 {
            return Err("arithmetic stripe of 0 streams".into());
        }
        let mut stripe_lens = Vec::with_capacity(n);
        for _ in 0..n {
            stripe_lens.push(r.uint7_len(MAX_OUT, "stripe")?);
        }
        let mut out = vec![0u8; len];
        for (j, cl) in stripe_lens.iter().enumerate() {
            let ulen = len / n + usize::from(len % n > j);
            let sub = r.take(*cl)?;
            let t = decode_at(&mut Cursor::new(sub), Some(ulen), depth + 1)?;
            for (i, &b) in t.iter().enumerate() {
                if let Some(o) = out.get_mut(i * n + j) {
                    *o = b;
                }
            }
        }
        return Ok(out);
    }
    let mut pack = None;
    if flags & PACK != 0 {
        let nsym = r.u8()? as usize;
        let p = r.take(nsym)?.to_vec();
        let plen = r.uint7_len(MAX_OUT, "packed data")?;
        pack = Some((p, len));
        len = plen;
    }
    let mut data = if flags & CAT != 0 {
        r.take(len)?.to_vec()
    } else if flags & EXT != 0 {
        let rest = r.rest();
        if !rest.starts_with(b"BZh") {
            return Err("arithmetic codec's external stream is not bzip2".into());
        }
        super::block::bunzip(rest, len)?
    } else if flags & RLE != 0 {
        rle(r, len, flags & ORDER != 0)?
    } else if flags & ORDER != 0 {
        order1(r, len)?
    } else {
        order0(r, len)?
    };
    if let Some((p, out_len)) = pack {
        data = unpack(&data, &p, out_len)?;
    }
    Ok(data)
}
