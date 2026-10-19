//! The fqzcomp quality codec of CRAM 3.1 (block method 7): adaptive models over a 16-bit
//! context of previous qualities, position, running delta and selector, read with the shared
//! range coder. Written from the CRAM codecs specification (`CRAMcodecs.tex` § 6, hts-specs).

use super::arith::{Model, RangeDecoder};
use super::io::{Cursor, DecodeResult};
use super::rans::MAX_OUT;

/// One parameter block.
struct Params {
    context: u32,
    do_dedup: bool,
    fixed_len: bool,
    do_sel: bool,
    have_qmap: bool,
    have_ptab: bool,
    have_dtab: bool,
    max_sym: usize,
    qbits: u32,
    qshift: u32,
    qloc: u32,
    sloc: u32,
    ploc: u32,
    dloc: u32,
    qmap: Vec<u8>,
    qtab: Vec<u32>,
    ptab: Vec<u32>,
    dtab: Vec<u32>,
    first_len: bool,
    last_len: usize,
}

/// A table mapping 0..n to a monotonic smaller range, stored with two levels of run length
/// (CRAMcodecs `ReadArray`).
fn read_array(r: &mut Cursor, n: usize) -> DecodeResult<Vec<u32>> {
    let mut runs: Vec<u32> = Vec::new();
    let mut z = 0usize;
    let mut last: i32 = -1;
    while z < n {
        let run = r.u8()?;
        runs.push(u32::from(run));
        z += run as usize;
        if i32::from(run) == last {
            let copy = r.u8()?;
            for _ in 0..copy {
                runs.push(u32::from(run));
            }
            z += run as usize * copy as usize;
        }
        last = i32::from(run);
        if runs.len() > 4 * n + 16 {
            return Err("fqzcomp table runs do not add up".into());
        }
    }
    let mut a = vec![0u32; n];
    let (mut i, mut j, mut z) = (0u32, 0usize, 0usize);
    while z < n {
        let mut run_len = 0usize;
        loop {
            let part = *runs
                .get(j)
                .ok_or_else(|| "fqzcomp table ends early".to_string())?;
            j += 1;
            run_len += part as usize;
            if part != 255 {
                break;
            }
        }
        for _ in 0..run_len {
            if z < n {
                a[z] = i;
            }
            z += 1;
        }
        i += 1;
    }
    Ok(a)
}

fn read_params(r: &mut Cursor) -> DecodeResult<Params> {
    let context = u32::from(r.u16_le()?);
    let flags = r.u8()?;
    let max_sym = r.u8()? as usize;
    let x = r.u8()?;
    let (qbits, qshift) = (u32::from(x >> 4), u32::from(x & 15));
    let x = r.u8()?;
    let (qloc, sloc) = (u32::from(x >> 4), u32::from(x & 15));
    let x = r.u8()?;
    let (ploc, dloc) = (u32::from(x >> 4), u32::from(x & 15));
    let mut p = Params {
        context,
        do_dedup: flags & 2 != 0,
        fixed_len: flags & 4 != 0,
        do_sel: flags & 8 != 0,
        have_qmap: flags & 16 != 0,
        have_ptab: flags & 32 != 0,
        have_dtab: flags & 64 != 0,
        max_sym,
        qbits,
        qshift,
        qloc,
        sloc,
        ploc,
        dloc,
        qmap: Vec::new(),
        qtab: (0..256).collect(),
        ptab: vec![0; 1024],
        dtab: vec![0; 256],
        first_len: true,
        last_len: 0,
    };
    if p.have_qmap {
        p.qmap = r.take(max_sym)?.to_vec();
    }
    if flags & 128 != 0 {
        p.qtab = read_array(r, 256)?;
    }
    if p.have_ptab {
        p.ptab = read_array(r, 1024)?;
    }
    if p.have_dtab {
        p.dtab = read_array(r, 256)?;
    }
    Ok(p)
}

/// fqzcomp (block method 7).
pub(crate) fn decode(data: &[u8]) -> DecodeResult<Vec<u8>> {
    let mut r = Cursor::new(data);
    let buf_len = r.uint7_len(MAX_OUT, "fqzcomp block")?;
    let vers = r.u8()?;
    if vers != 5 {
        return Err(format!("fqzcomp version {vers} (only 5 is defined)"));
    }
    let gflags = r.u8()?;
    let multi = gflags & 1 != 0;
    let have_stab = gflags & 2 != 0;
    let do_rev = gflags & 4 != 0;
    let (nparam, mut max_sel) = if multi {
        let n = r.u8()? as usize;
        (n, n)
    } else {
        (1, 0)
    };
    let mut stab: Vec<u32> = (0..256).collect();
    if have_stab {
        max_sel = r.u8()? as usize;
        stab = read_array(&mut r, 256)?;
    }
    if nparam == 0 {
        return Err("fqzcomp stream with no parameter block".into());
    }
    let mut params = Vec::with_capacity(nparam);
    let mut max_sym = 0;
    for _ in 0..nparam {
        let p = read_params(&mut r)?;
        max_sym = max_sym.max(p.max_sym);
        params.push(p);
    }
    let mut rc = RangeDecoder::new(r.clone())?;
    let mut model_len = vec![Model::new(256); 4];
    let mut model_qual: Vec<Option<Box<Model>>> = (0..1 << 16).map(|_| None).collect();
    let mut model_dup = Model::new(2);
    let mut model_rev = Model::new(2);
    let mut model_sel = (max_sel > 0).then(|| Model::new(max_sel + 1));
    let mut out = vec![0u8; buf_len];
    let mut rev: Vec<(usize, bool)> = Vec::new();
    let mut i = 0usize;
    while i < buf_len {
        // a new record
        let mut sel = 0u32;
        let mut x = 0usize;
        if let Some(m) = model_sel.as_mut() {
            sel = u32::from(m.decode(&mut rc)?);
            x = *stab.get(sel as usize).unwrap_or(&0) as usize;
        }
        let p = params
            .get_mut(x)
            .ok_or_else(|| format!("fqzcomp selector {x} names no parameter block"))?;
        // Parameter flag 4 marks a fixed read length, decoded with the first record only: the
        // hts-specs codec test vectors decode this way, while the CRAMcodecs pseudocode reads
        // the flag the other way round (docs/provenance/cram.md).
        let rec_len = if !p.fixed_len || p.first_len {
            let mut l = 0usize;
            for (k, m) in model_len.iter_mut().enumerate() {
                l |= (m.decode(&mut rc)? as usize) << (8 * k);
            }
            p.last_len = l;
            p.first_len = false;
            l
        } else {
            p.last_len
        };
        if rec_len == 0 || rec_len > buf_len - i {
            return Err(format!(
                "fqzcomp record of {rec_len} qualities with {} left in the block",
                buf_len - i
            ));
        }
        if do_rev {
            rev.push((rec_len, model_rev.decode(&mut rc)? != 0));
        }
        if p.do_dedup && model_dup.decode(&mut rc)? != 0 {
            if i < rec_len {
                return Err("fqzcomp duplicate of a record before the first".into());
            }
            out.copy_within(i - rec_len..i, i);
            i += rec_len;
            continue;
        }
        let (mut qctx, mut delta, mut prevq) = (0u32, 0u32, 0u32);
        let mut ctx = p.context;
        let mut pos = rec_len;
        while pos > 0 {
            let m = model_qual[(ctx & 0xffff) as usize]
                .get_or_insert_with(|| Box::new(Model::new(max_sym + 1)));
            let q = u32::from(m.decode(&mut rc)?);
            out[i] = if p.have_qmap {
                *p.qmap
                    .get(q as usize)
                    .ok_or_else(|| "fqzcomp quality outside its map".to_string())?
            } else {
                q as u8
            };
            // the context for the next quality
            let mut c = p.context;
            qctx = (qctx << p.qshift).wrapping_add(*p.qtab.get(q as usize).unwrap_or(&0));
            c = c.wrapping_add((qctx & ((1u32 << p.qbits) - 1)) << p.qloc);
            if p.have_ptab {
                c = c.wrapping_add(p.ptab[pos.min(1023)] << p.ploc);
            }
            if p.have_dtab {
                c = c.wrapping_add(p.dtab[(delta as usize).min(255)] << p.dloc);
                if prevq != q {
                    delta += 1;
                }
                prevq = q;
            }
            if p.do_sel {
                c = c.wrapping_add(sel << p.sloc);
            }
            ctx = c & 0xffff;
            i += 1;
            pos -= 1;
        }
    }
    if do_rev {
        let mut at = 0usize;
        for (len, r) in rev {
            if r {
                out[at..at + len].reverse();
            }
            at += len;
        }
    }
    Ok(out)
}
