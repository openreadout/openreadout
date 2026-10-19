//! rANS entropy decoders: rANS 4x8 (CRAM 3.0, block method 4) and rANS Nx16 with its stripe,
//! run-length and bit-packing transforms (CRAM 3.1, block method 5). Written from the CRAM
//! codecs specification (`CRAMcodecs.tex` §§ 2-3, hts-specs).

use super::io::{Cursor, DecodeResult};

/// Largest decoded block accepted (bytes). Writers keep slices to some millions of bases, so
/// real blocks stay far below this.
pub(crate) const MAX_OUT: usize = 256 << 20;

/// Frequencies and cumulative frequencies of one context, with a lookup from a cumulative
/// frequency to its symbol.
struct Table {
    f: [u32; 256],
    c: [u32; 257],
    lookup: Vec<u8>,
}

impl Table {
    fn new(f: &[u32; 256], total_bits: u32) -> DecodeResult<Self> {
        let mut c = [0u32; 257];
        for s in 0..256 {
            c[s + 1] = c[s]
                .checked_add(f[s])
                .ok_or_else(|| "rANS frequencies overflow".to_string())?;
        }
        let total = 1u32 << total_bits;
        if c[256] > total {
            return Err(format!(
                "rANS frequencies sum to {}, more than {total}",
                c[256]
            ));
        }
        let mut lookup = vec![0u8; total as usize];
        for s in 0..256usize {
            for x in c[s]..c[s + 1] {
                lookup[x as usize] = s as u8;
            }
        }
        Ok(Table { f: *f, c, lookup })
    }

    /// The symbol whose range holds cumulative frequency `x`, and its frequency and start.
    fn symbol(&self, x: u32) -> DecodeResult<(u8, u32, u32)> {
        if x >= self.c[256] {
            return Err("rANS state points past the frequency table".into());
        }
        let s = self.lookup[x as usize];
        Ok((s, self.f[s as usize], self.c[s as usize]))
    }
}

/// Symbol list with run-length (CRAMcodecs `ReadAlphabet`, also the 4x8 table's symbol order).
fn read_alphabet(r: &mut Cursor) -> DecodeResult<Vec<u8>> {
    let mut a = Vec::new();
    let mut s = r.u8()?;
    let mut last = s;
    let mut rle = 0u32;
    loop {
        a.push(s);
        if a.len() > 256 {
            return Err("rANS alphabet longer than 256 symbols".into());
        }
        if rle > 0 {
            rle -= 1;
            s = s.wrapping_add(1);
        } else {
            s = r.u8()?;
            if last != 255 && s == last + 1 {
                rle = u32::from(r.u8()?);
            }
        }
        last = s;
        if s == 0 {
            break;
        }
    }
    Ok(a)
}

/// rANS 4x8 order-0 frequency table: symbols with RLE, each followed by an ITF8 frequency.
fn freqs_4x8(r: &mut Cursor) -> DecodeResult<[u32; 256]> {
    let mut f = [0u32; 256];
    let mut s = r.u8()?;
    let mut last = s;
    let mut rle = 0u32;
    let mut n = 0;
    loop {
        f[s as usize] = u32::try_from(r.itf8()?).map_err(|_| "negative rANS frequency")?;
        n += 1;
        if n > 256 {
            return Err("rANS frequency table longer than 256 symbols".into());
        }
        if rle > 0 {
            rle -= 1;
            s = s.wrapping_add(1);
        } else {
            s = r.u8()?;
            if last != 255 && s == last + 1 {
                rle = u32::from(r.u8()?);
            }
        }
        last = s;
        if s == 0 {
            break;
        }
    }
    Ok(f)
}

/// rANS 4x8 (block method 4).
pub(crate) fn decode_4x8(data: &[u8]) -> DecodeResult<Vec<u8>> {
    let mut r = Cursor::new(data);
    let order = r.u8()?;
    let _n_in = r.u32_le()?;
    let n_out = r.u32_le()? as usize;
    if n_out > MAX_OUT {
        return Err(format!("rANS block of {n_out} bytes is too large"));
    }
    let mut out = vec![0u8; n_out];
    if n_out == 0 {
        return Ok(out);
    }
    let renorm = |r: &mut Cursor, mut x: u32| -> DecodeResult<u32> {
        while x < (1 << 23) {
            x = (x << 8) | u32::from(r.u8()?);
        }
        Ok(x)
    };
    let step = |t: &Table, x: u32| -> DecodeResult<(u8, u32)> {
        let (s, f, c) = t.symbol(x & 0xfff)?;
        Ok((
            s,
            f.wrapping_mul(x >> 12)
                .wrapping_add(x & 0xfff)
                .wrapping_sub(c),
        ))
    };
    if order == 0 {
        let t = Table::new(&freqs_4x8(&mut r)?, 12)?;
        let mut st = [0u32; 4];
        for x in &mut st {
            *x = r.u32_le()?;
        }
        for i in 0..n_out {
            let j = i % 4;
            let (s, x) = step(&t, st[j])?;
            out[i] = s;
            st[j] = renorm(&mut r, x)?;
        }
    } else {
        let mut tables: Vec<Option<Table>> = (0..256).map(|_| None).collect();
        let mut ctx = r.u8()?;
        let mut last = ctx;
        let mut rle = 0u32;
        let mut n = 0;
        loop {
            tables[ctx as usize] = Some(Table::new(&freqs_4x8(&mut r)?, 12)?);
            n += 1;
            if n > 256 {
                return Err("rANS order-1 table longer than 256 contexts".into());
            }
            if rle > 0 {
                rle -= 1;
                ctx = ctx.wrapping_add(1);
            } else {
                ctx = r.u8()?;
                if last != 255 && ctx == last + 1 {
                    rle = u32::from(r.u8()?);
                }
            }
            last = ctx;
            if ctx == 0 {
                break;
            }
        }
        let mut st = [0u32; 4];
        for x in &mut st {
            *x = r.u32_le()?;
        }
        let mut prev = [0u8; 4];
        let quarter = n_out / 4;
        let mut decode = |j: usize, st: &mut [u32; 4], prev: &mut [u8; 4]| -> DecodeResult<u8> {
            let t = tables[prev[j] as usize]
                .as_ref()
                .ok_or_else(|| "rANS order-1 context without a table".to_string())?;
            let (s, x) = step(t, st[j])?;
            st[j] = renorm(&mut r, x)?;
            prev[j] = s;
            Ok(s)
        };
        for i in 0..quarter {
            for j in 0..4 {
                out[i + j * quarter] = decode(j, &mut st, &mut prev)?;
            }
        }
        for o in out.iter_mut().skip(quarter * 4) {
            *o = decode(3, &mut st, &mut prev)?;
        }
    }
    Ok(out)
}

// ---- rANS Nx16 ----

const ORDER: u8 = 1;
const N32: u8 = 4;
const STRIPE: u8 = 8;
const NOSIZE: u8 = 16;
const CAT: u8 = 32;
const RLE: u8 = 64;
const PACK: u8 = 128;

/// Scale frequencies up to sum to `1 << bits` (CRAMcodecs `NormaliseFrequenciesNx16_0`).
fn normalise(f: &mut [u32; 256], bits: u32) -> DecodeResult<()> {
    let mut tot: u64 = f.iter().map(|v| u64::from(*v)).sum();
    let want = 1u64 << bits;
    if tot == 0 || tot == want {
        return Ok(());
    }
    if tot > want {
        return Err(format!(
            "rANS Nx16 frequencies sum to {tot}, more than {want}"
        ));
    }
    let mut shift = 0;
    while tot < want {
        tot *= 2;
        shift += 1;
    }
    for v in f.iter_mut() {
        *v <<= shift;
    }
    Ok(())
}

fn freqs_nx16_0(r: &mut Cursor) -> DecodeResult<Table> {
    let mut f = [0u32; 256];
    for s in read_alphabet(r)? {
        f[s as usize] = u32::try_from(r.uint7()?).map_err(|_| "rANS frequency too large")?;
    }
    normalise(&mut f, 12)?;
    Table::new(&f, 12)
}

fn renorm16(r: &mut Cursor, x: u32) -> DecodeResult<u32> {
    if x < (1 << 15) {
        Ok((x << 16) | u32::from(r.u16_le()?))
    } else {
        Ok(x)
    }
}

/// Order-0 rANS Nx16 entropy decoding of `len` bytes with `n` states.
fn nx16_order0(r: &mut Cursor, len: usize, n: usize) -> DecodeResult<Vec<u8>> {
    let t = freqs_nx16_0(r)?;
    let mut st = vec![0u32; n];
    for x in &mut st {
        *x = r.u32_le()?;
    }
    let mut out = vec![0u8; len];
    for (i, o) in out.iter_mut().enumerate() {
        let j = i % n;
        let x = st[j];
        let (s, f, c) = t.symbol(x & 0xfff)?;
        *o = s;
        st[j] = renorm16(
            r,
            f.wrapping_mul(x >> 12)
                .wrapping_add(x & 0xfff)
                .wrapping_sub(c),
        )?;
    }
    Ok(out)
}

/// Order-1 rANS Nx16 entropy decoding of `len` bytes with `n` states.
fn nx16_order1(r: &mut Cursor, len: usize, n: usize) -> DecodeResult<Vec<u8>> {
    let comp = r.u8()?;
    let bits = u32::from(comp >> 4);
    if bits != 10 && bits != 12 {
        return Err(format!("rANS Nx16 order-1 table of {bits}-bit frequencies"));
    }
    let owned;
    let mut src: Cursor = if comp & 1 != 0 {
        let u_size = r.uint7_len(MAX_OUT, "rANS order-1 table")?;
        let c_size = r.uint7_len(MAX_OUT, "rANS order-1 table")?;
        let c_data = r.take(c_size)?;
        owned = nx16_order0(&mut Cursor::new(c_data), u_size, 4)?;
        Cursor::new(&owned)
    } else {
        r.clone()
    };
    let alphabet = read_alphabet(&mut src)?;
    let mut tables: Vec<Option<Table>> = (0..256).map(|_| None).collect();
    for &i in &alphabet {
        let mut f = [0u32; 256];
        let mut run = 0u32;
        for &j in &alphabet {
            if run > 0 {
                run -= 1;
            } else {
                f[j as usize] =
                    u32::try_from(src.uint7()?).map_err(|_| "rANS frequency too large")?;
                if f[j as usize] == 0 {
                    run = u32::from(src.u8()?);
                }
            }
        }
        normalise(&mut f, bits)?;
        tables[i as usize] = Some(Table::new(&f, bits)?);
    }
    if comp & 1 == 0 {
        // the table was read from the main stream: continue after it
        let used = src.pos();
        r.skip(used - r.pos())?;
    }
    let mut st = vec![0u32; n];
    for x in &mut st {
        *x = r.u32_le()?;
    }
    let mut prev = vec![0u8; n];
    let mut out = vec![0u8; len];
    let mask = (1u32 << bits) - 1;
    let part = len / n;
    let mut step = |j: usize, r: &mut Cursor| -> DecodeResult<u8> {
        let t = tables[prev[j] as usize]
            .as_ref()
            .ok_or_else(|| "rANS order-1 context without a table".to_string())?;
        let x = st[j];
        let (s, f, c) = t.symbol(x & mask)?;
        st[j] = renorm16(
            r,
            f.wrapping_mul(x >> bits)
                .wrapping_add(x & mask)
                .wrapping_sub(c),
        )?;
        prev[j] = s;
        Ok(s)
    };
    for i in 0..part {
        for j in 0..n {
            out[i + j * part] = step(j, r)?;
        }
    }
    for i in part * n..len {
        out[i] = step(n - 1, r)?;
    }
    Ok(out)
}

/// The pack metadata: symbol map and the packed length.
fn pack_meta(r: &mut Cursor) -> DecodeResult<(Vec<u8>, usize)> {
    let nsym = r.u8()? as usize;
    let p = r.take(nsym)?.to_vec();
    let len = r.uint7_len(MAX_OUT, "packed data")?;
    Ok((p, len))
}

/// Expand bit-packed data to `len` symbols (CRAMcodecs `DecodePack`).
pub(crate) fn unpack(data: &[u8], p: &[u8], len: usize) -> DecodeResult<Vec<u8>> {
    let nsym = p.len();
    let mut out = Vec::with_capacity(len.min(16 << 20));
    if nsym == 0 {
        return Err("bit packing with no symbols".into());
    }
    if nsym == 1 {
        out.resize(len, p[0]);
        return Ok(out);
    }
    let (bits, per) = match nsym {
        2 => (1u32, 8usize),
        3..=4 => (2, 4),
        5..=16 => (4, 2),
        _ => return Err(format!("bit packing with {nsym} symbols")),
    };
    let mask = (1u8 << bits) - 1;
    let mut v = 0u8;
    for i in 0..len {
        if i % per == 0 {
            v = *data
                .get(i / per)
                .ok_or_else(|| "packed data ends early".to_string())?;
        }
        let x = (v & mask) as usize;
        out.push(
            *p.get(x)
                .ok_or_else(|| "packed value outside the symbol map".to_string())?,
        );
        v >>= bits;
    }
    Ok(out)
}

/// Run-length metadata of rANS Nx16: which symbols carry runs, and the run lengths.
fn rle_meta(r: &mut Cursor, n: usize) -> DecodeResult<([bool; 256], Vec<u8>, usize)> {
    let meta_len = r.uint7_len(2 * MAX_OUT, "RLE metadata")?;
    let len = r.uint7_len(MAX_OUT, "RLE data")?;
    let meta = if meta_len & 1 != 0 {
        r.take(meta_len / 2)?.to_vec()
    } else {
        let clen = r.uint7_len(MAX_OUT, "RLE metadata")?;
        let c = r.take(clen)?;
        nx16_order0(&mut Cursor::new(c), meta_len / 2, n)?
    };
    let mut m = Cursor::new(&meta);
    let mut nsym = m.u8()? as usize;
    if nsym == 0 {
        nsym = 256;
    }
    let mut runs = [false; 256];
    for _ in 0..nsym {
        runs[m.u8()? as usize] = true;
    }
    Ok((runs, m.rest().to_vec(), len))
}

/// Expand run-length data to `out_len` bytes.
pub(crate) fn unrle(
    data: &[u8],
    runs: &[bool; 256],
    meta: &[u8],
    out_len: usize,
) -> DecodeResult<Vec<u8>> {
    let mut m = Cursor::new(meta);
    let mut out = Vec::with_capacity(out_len.min(16 << 20));
    for &s in data {
        if runs[s as usize] {
            let run = m.uint7_len(MAX_OUT, "run")?;
            if out.len() + run + 1 > out_len {
                return Err("run-length data longer than declared".into());
            }
            out.extend(std::iter::repeat_n(s, run + 1));
        } else {
            if out.len() >= out_len {
                return Err("run-length data longer than declared".into());
            }
            out.push(s);
        }
    }
    if out.len() != out_len {
        return Err(format!(
            "run-length data expands to {} bytes, {out_len} declared",
            out.len()
        ));
    }
    Ok(out)
}

/// rANS Nx16 (block method 5): `len` is the size when the stream does not record one.
pub(crate) fn decode_nx16(data: &[u8], len: Option<usize>) -> DecodeResult<Vec<u8>> {
    decode_nx16_at(&mut Cursor::new(data), len, 0)
}

fn decode_nx16_at(r: &mut Cursor, len: Option<usize>, depth: u32) -> DecodeResult<Vec<u8>> {
    if depth > 4 {
        return Err("rANS Nx16 stripes nested too deep".into());
    }
    let flags = r.u8()?;
    let mut len = if flags & NOSIZE == 0 {
        r.uint7_len(MAX_OUT, "rANS Nx16 block")?
    } else {
        len.ok_or_else(|| "rANS Nx16 stream without a size".to_string())?
    };
    if flags & STRIPE != 0 {
        let n = r.u8()? as usize;
        if n == 0 {
            return Err("rANS Nx16 stripe of 0 streams".into());
        }
        let mut stripe_lens = Vec::with_capacity(n);
        for _ in 0..n {
            stripe_lens.push(r.uint7_len(MAX_OUT, "stripe")?);
        }
        let mut parts = Vec::with_capacity(n);
        for (j, cl) in stripe_lens.iter().enumerate() {
            let ulen = len / n + usize::from(len % n > j);
            let sub = r.take(*cl)?;
            parts.push(decode_nx16_at(
                &mut Cursor::new(sub),
                Some(ulen),
                depth + 1,
            )?);
        }
        let mut out = vec![0u8; len];
        for (j, t) in parts.iter().enumerate() {
            for (i, &b) in t.iter().enumerate() {
                if let Some(o) = out.get_mut(i * n + j) {
                    *o = b;
                }
            }
        }
        return Ok(out);
    }
    let n = if flags & N32 != 0 { 32 } else { 4 };
    let mut pack = None;
    if flags & PACK != 0 {
        let (p, plen) = pack_meta(r)?;
        pack = Some((p, len));
        len = plen;
    }
    let mut rle = None;
    if flags & RLE != 0 {
        let (runs, meta, rlen) = rle_meta(r, n)?;
        rle = Some((runs, meta, len));
        len = rlen;
    }
    let mut data = if flags & CAT != 0 {
        r.take(len)?.to_vec()
    } else if flags & ORDER != 0 {
        nx16_order1(r, len, n)?
    } else {
        nx16_order0(r, len, n)?
    };
    if let Some((runs, meta, out_len)) = rle {
        data = unrle(&data, &runs, &meta, out_len)?;
    }
    if let Some((p, out_len)) = pack {
        data = unpack(&data, &p, out_len)?;
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpacks_and_expands_runs() {
        assert_eq!(unpack(&[0b1110_0100], b"ACGT", 4).unwrap(), b"ACGT");
        assert_eq!(unpack(&[], &[7], 3).unwrap(), vec![7, 7, 7]);
        let mut runs = [false; 256];
        runs[b'A' as usize] = true;
        assert_eq!(unrle(b"AB", &runs, &[2], 4).unwrap(), b"AAAB");
        assert!(unrle(b"AB", &runs, &[9], 4).is_err());
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(decode_4x8(&[0, 1, 0, 0, 0, 9, 0, 0, 0]).is_err());
        assert!(decode_nx16(&[0xff], None).is_err());
        assert!(decode_nx16(&[0, 4, 0], None).is_err());
    }
}
