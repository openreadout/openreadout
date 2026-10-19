//! The read-name tokeniser of CRAM 3.1 (block method 8): names split into typed tokens, each
//! token position and type a byte stream of its own, compressed with rANS Nx16 or the
//! arithmetic coder. Written from the CRAM codecs specification (`CRAMcodecs.tex` § 5,
//! hts-specs). The output is every name followed by a nul byte.

use std::collections::HashMap;

use super::io::{Cursor, DecodeResult};
use super::rans::MAX_OUT;

const TYPE: u8 = 0;
const STRING: u8 = 1;
const CHAR: u8 = 2;
const DIGITS0: u8 = 3;
const DZLEN: u8 = 4;
const DUP: u8 = 5;
const DIFF: u8 = 6;
const DIGITS: u8 = 7;
const DELTA: u8 = 8;
const DELTA0: u8 = 9;
const MATCH: u8 = 10;
const END: u8 = 12;

/// Most tokens in one name.
const MAX_TOKENS: usize = 128;

/// One decoded token: its text and, for numbers, the value and printed width.
#[derive(Debug, Clone, Default)]
struct Tok {
    text: Vec<u8>,
    num: Option<(u32, usize)>,
}

/// A byte stream with a read position.
#[derive(Default)]
struct Stream {
    data: Vec<u8>,
    at: usize,
}

impl Stream {
    fn u8(&mut self) -> DecodeResult<u8> {
        let b = *self
            .data
            .get(self.at)
            .ok_or_else(|| "name tokeniser stream ends early".to_string())?;
        self.at += 1;
        Ok(b)
    }

    fn u32(&mut self) -> DecodeResult<u32> {
        let mut v = [0u8; 4];
        for b in &mut v {
            *b = self.u8()?;
        }
        Ok(u32::from_le_bytes(v))
    }

    fn string(&mut self) -> DecodeResult<Vec<u8>> {
        let rest = self.data.get(self.at..).unwrap_or(&[]);
        let end = rest
            .iter()
            .position(|b| *b == 0)
            .ok_or_else(|| "name tokeniser string without its nul".to_string())?;
        let s = rest[..end].to_vec();
        self.at += end + 1;
        Ok(s)
    }
}

fn stream(b: &mut HashMap<(usize, u8), Stream>, pos: usize, ty: u8) -> DecodeResult<&mut Stream> {
    b.get_mut(&(pos, ty))
        .ok_or_else(|| format!("name token stream {pos}/{ty} is missing"))
}

/// Decode the serialised token streams and every name (`len` is the block's raw size).
pub(crate) fn decode(data: &[u8]) -> DecodeResult<Vec<u8>> {
    let mut r = Cursor::new(data);
    let ulen = r.u32_le()? as usize;
    let nnames = r.u32_le()? as usize;
    let use_arith = r.u8()? != 0;
    if ulen > MAX_OUT || nnames > ulen.max(1) || nnames > 1 << 24 {
        return Err(format!(
            "name tokeniser block of {ulen} bytes for {nnames} names"
        ));
    }
    let mut b: HashMap<(usize, u8), Stream> = HashMap::new();
    let mut t: i64 = -1;
    while !r.is_empty() {
        let ttype = r.u8()?;
        let tok_new = ttype & 128 != 0;
        let tok_dup = ttype & 64 != 0;
        let ty = ttype & 63;
        if tok_new {
            t += 1;
            if t as usize > MAX_TOKENS {
                return Err("more than 128 name token positions".into());
            }
            if ty != TYPE {
                let mut types = vec![MATCH; nnames];
                if let Some(f) = types.first_mut() {
                    *f = ty;
                }
                b.insert((t as usize, TYPE), Stream { data: types, at: 0 });
            }
        }
        if t < 0 {
            return Err("name token stream before its position".into());
        }
        let data = if tok_dup {
            let pos = r.u8()? as usize;
            let dty = r.u8()?;
            b.get(&(pos, dty))
                .map(|s| s.data.clone())
                .ok_or_else(|| "name token stream duplicates a missing one".to_string())?
        } else {
            let clen = r.uint7_len(MAX_OUT, "name token stream")?;
            let c = r.take(clen)?;
            if use_arith {
                super::arith::decode(c, None)?
            } else {
                super::rans::decode_nx16(c, None)?
            }
        };
        b.insert((t as usize, ty), Stream { data, at: 0 });
    }
    let mut names: Vec<Vec<u8>> = Vec::with_capacity(nnames.min(1 << 16));
    let mut toks: Vec<Vec<Tok>> = Vec::with_capacity(nnames.min(1 << 16));
    let mut out = Vec::with_capacity(ulen.min(16 << 20));
    for n in 0..nnames {
        let ty = stream(&mut b, 0, TYPE)?.u8()?;
        let dist = stream(&mut b, 0, ty)?.u32()? as usize;
        let m = n
            .checked_sub(dist)
            .filter(|m| *m < n || (dist == 0 && ty == DIFF))
            .ok_or_else(|| format!("name {n} refers {dist} names back"))?;
        if ty == DUP {
            if m >= n {
                return Err("a duplicate name refers to itself".into());
            }
            names.push(names[m].clone());
            toks.push(toks[m].clone());
            out.extend_from_slice(&names[n]);
            out.push(0);
            continue;
        }
        let mut name = Vec::new();
        let mut these: Vec<Tok> = vec![Tok::default()];
        let mut pos = 1usize;
        loop {
            if pos > MAX_TOKENS {
                return Err("name with more than 128 tokens".into());
            }
            let ty = stream(&mut b, pos, TYPE)?.u8()?;
            let prev = if m < n { toks[m].get(pos) } else { None };
            let tok = match ty {
                CHAR => Tok {
                    text: vec![stream(&mut b, pos, CHAR)?.u8()?],
                    num: None,
                },
                STRING => Tok {
                    text: stream(&mut b, pos, STRING)?.string()?,
                    num: None,
                },
                DIGITS => {
                    let v = stream(&mut b, pos, DIGITS)?.u32()?;
                    let text = v.to_string().into_bytes();
                    let w = text.len();
                    Tok {
                        text,
                        num: Some((v, w)),
                    }
                }
                DIGITS0 => {
                    let v = stream(&mut b, pos, DIGITS0)?.u32()?;
                    let w = stream(&mut b, pos, DZLEN)?.u8()? as usize;
                    Tok {
                        text: format!("{v:0w$}").into_bytes(),
                        num: Some((v, w)),
                    }
                }
                DELTA | DELTA0 => {
                    let d = u32::from(stream(&mut b, pos, ty)?.u8()?);
                    let (pv, pw) = prev
                        .and_then(|p| p.num)
                        .ok_or_else(|| "name delta against a token without a number".to_string())?;
                    let v = pv.wrapping_add(d);
                    if ty == DELTA {
                        let text = v.to_string().into_bytes();
                        let w = text.len();
                        Tok {
                            text,
                            num: Some((v, w)),
                        }
                    } else {
                        Tok {
                            text: format!("{v:0pw$}").into_bytes(),
                            num: Some((v, pw)),
                        }
                    }
                }
                MATCH => prev
                    .cloned()
                    .ok_or_else(|| "name token matches a missing token".to_string())?,
                _ => Tok {
                    text: Vec::new(),
                    num: None,
                },
            };
            name.extend_from_slice(&tok.text);
            these.push(tok);
            pos += 1;
            if ty == END {
                break;
            }
        }
        out.extend_from_slice(&name);
        out.push(0);
        names.push(name);
        toks.push(these);
        if out.len() > ulen + 1 {
            return Err("names longer than the tokeniser block says".into());
        }
    }
    Ok(out)
}
