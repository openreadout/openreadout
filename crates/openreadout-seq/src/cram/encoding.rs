//! The compression header (preservation map, data-series encodings, tag encodings) and the
//! encodings that read one value of a data series from the core bit stream or an external block
//! (CRAM specification §§ 8.4 and 13).

use std::collections::HashMap;

use super::io::{BitReader, Cursor, DecodeResult};

/// Longest byte array one value may hold.
const MAX_ARRAY: usize = 256 << 20;

/// How a data series is stored.
#[derive(Debug, Clone)]
pub(crate) enum Encoding {
    /// Not stored.
    Null,
    /// Verbatim in an external block (ITF8 for integers).
    External(i32),
    /// Canonical Huffman codes in the core block.
    Huffman(Huffman),
    /// A length (first encoding) then that many bytes (second encoding).
    ByteArrayLen(Box<Encoding>, Box<Encoding>),
    /// Bytes from an external block up to a stop byte.
    ByteArrayStop(u8, i32),
    /// A fixed number of bits, minus an offset.
    Beta { offset: i32, bits: u32 },
    /// Subexponential code with parameter `k`, minus an offset.
    Subexp { offset: i32, k: u32 },
    /// Elias gamma code, minus an offset.
    Gamma { offset: i32 },
    /// Golomb code (deprecated in the specification).
    Golomb { offset: i32, m: u32 },
    /// Golomb-Rice code (deprecated in the specification).
    GolombRice { offset: i32, log2m: u32 },
}

/// A canonical Huffman code.
#[derive(Debug, Clone)]
pub(crate) struct Huffman {
    /// The only symbol when the alphabet has one (zero-length code).
    single: Option<i32>,
    /// Per code length (index): first code, index of its first symbol in `symbols`, count.
    lens: Vec<(u32, usize, usize)>,
    /// Symbols sorted by (code length, value).
    symbols: Vec<i32>,
}

impl Huffman {
    fn new(alphabet: Vec<i32>, lengths: Vec<i32>) -> DecodeResult<Self> {
        if alphabet.len() != lengths.len() {
            return Err(format!(
                "Huffman alphabet of {} symbols with {} code lengths",
                alphabet.len(),
                lengths.len()
            ));
        }
        if alphabet.is_empty() {
            // a series that holds no values (some writers store such an encoding)
            return Ok(Huffman {
                single: None,
                lens: Vec::new(),
                symbols: Vec::new(),
            });
        }
        if alphabet.len() == 1 {
            return Ok(Huffman {
                single: Some(alphabet[0]),
                lens: Vec::new(),
                symbols: alphabet,
            });
        }
        let mut pairs: Vec<(u32, i32)> = Vec::with_capacity(alphabet.len());
        for (s, l) in alphabet.iter().zip(&lengths) {
            let l = u32::try_from(*l)
                .ok()
                .filter(|l| (1..=31).contains(l))
                .ok_or_else(|| format!("Huffman code length {l}"))?;
            pairs.push((l, *s));
        }
        pairs.sort_unstable();
        let max = pairs.last().map_or(0, |p| p.0) as usize;
        let mut lens = vec![(0u32, 0usize, 0usize); max + 1];
        let mut code: u64 = 0;
        let mut prev_len = pairs[0].0;
        for (i, (l, _)) in pairs.iter().enumerate() {
            if i > 0 {
                code += 1;
                if *l > prev_len {
                    code <<= l - prev_len;
                    prev_len = *l;
                }
            } else {
                prev_len = *l;
            }
            if code >= (1u64 << l) {
                return Err("Huffman code lengths over-subscribe the code space".into());
            }
            let e = &mut lens[*l as usize];
            if e.2 == 0 {
                *e = (code as u32, i, 0);
            }
            e.2 += 1;
        }
        Ok(Huffman {
            single: None,
            lens,
            symbols: pairs.into_iter().map(|p| p.1).collect(),
        })
    }

    fn decode(&self, bits: &mut BitReader) -> DecodeResult<i32> {
        if let Some(s) = self.single {
            return Ok(s);
        }
        let mut code: u32 = 0;
        for l in 1..self.lens.len() {
            code = (code << 1) | bits.bit()?;
            let (first, at, n) = self.lens[l];
            if n > 0 && code >= first && code - first < n as u32 {
                return Ok(self.symbols[at + (code - first) as usize]);
            }
        }
        Err("no Huffman code matches the core data".into())
    }
}

/// Parse one encoding: codec id, parameter length, parameters.
pub(crate) fn parse(r: &mut Cursor) -> DecodeResult<Encoding> {
    let id = r.itf8()?;
    let n = r.itf8_len(1 << 20, "encoding parameter size")?;
    let mut p = Cursor::new(r.take(n)?);
    let enc = match id {
        0 => Encoding::Null,
        1 => Encoding::External(p.itf8()?),
        3 => {
            let na = p.itf8_len(1 << 20, "Huffman alphabet size")?;
            let mut alphabet = Vec::with_capacity(na);
            for _ in 0..na {
                alphabet.push(p.itf8()?);
            }
            let nl = p.itf8_len(1 << 20, "Huffman code-length count")?;
            let mut lengths = Vec::with_capacity(nl);
            for _ in 0..nl {
                lengths.push(p.itf8()?);
            }
            Encoding::Huffman(Huffman::new(alphabet, lengths)?)
        }
        4 => {
            let a = parse(&mut p)?;
            let b = parse(&mut p)?;
            Encoding::ByteArrayLen(Box::new(a), Box::new(b))
        }
        5 => Encoding::ByteArrayStop(p.u8()?, p.itf8()?),
        6 => Encoding::Beta {
            offset: p.itf8()?,
            bits: u32::try_from(p.itf8()?)
                .ok()
                .filter(|b| *b <= 32)
                .ok_or("Beta coding with a bad bit count")?,
        },
        7 => Encoding::Subexp {
            offset: p.itf8()?,
            k: u32::try_from(p.itf8()?)
                .ok()
                .filter(|k| *k <= 31)
                .ok_or("subexponential coding with a bad k")?,
        },
        9 => Encoding::Gamma { offset: p.itf8()? },
        2 => Encoding::Golomb {
            offset: p.itf8()?,
            m: u32::try_from(p.itf8()?)
                .ok()
                .filter(|m| *m > 0)
                .ok_or("Golomb coding with a bad M")?,
        },
        8 => Encoding::GolombRice {
            offset: p.itf8()?,
            log2m: u32::try_from(p.itf8()?)
                .ok()
                .filter(|m| *m < 31)
                .ok_or("Golomb-Rice coding with a bad log2 M")?,
        },
        other => return Err(format!("encoding codec {other} is not defined")),
    };
    Ok(enc)
}

/// The compression header of a container.
#[derive(Debug, Clone)]
pub(crate) struct CompressionHeader {
    pub(crate) read_names: bool,
    pub(crate) ap_delta: bool,
    pub(crate) reference_required: bool,
    /// Substitution matrix: `sub[reference base][code]` = read base (bases A, C, G, T, N).
    pub(crate) sub: [[u8; 4]; 5],
    /// Tag lists: each entry is (tag id, type, the key in `tags`).
    pub(crate) tag_dict: Vec<Vec<([u8; 2], u8, i32)>>,
    pub(crate) series: HashMap<[u8; 2], Encoding>,
    pub(crate) tags: HashMap<i32, Encoding>,
}

const BASES: [u8; 5] = *b"ACGTN";

/// Parse the substitution matrix (5 bytes, one per reference base, four 2-bit codes high first
/// for the other bases in A, C, G, T, N order).
fn sub_matrix(b: &[u8]) -> [[u8; 4]; 5] {
    let mut m = [[b'N'; 4]; 5];
    for (ri, &byte) in b.iter().take(5).enumerate() {
        let others: Vec<u8> = BASES.iter().copied().filter(|x| *x != BASES[ri]).collect();
        for (k, &base) in others.iter().enumerate() {
            let code = (byte >> (6 - 2 * k)) & 3;
            m[ri][code as usize] = base;
        }
    }
    m
}

/// Parse a compression header block.
pub(crate) fn compression_header(data: &[u8]) -> DecodeResult<CompressionHeader> {
    let mut r = Cursor::new(data);
    let mut h = CompressionHeader {
        read_names: true,
        ap_delta: true,
        reference_required: true,
        sub: sub_matrix(&[0x1b, 0x1b, 0x1b, 0x1b, 0x1b]),
        tag_dict: Vec::new(),
        series: HashMap::new(),
        tags: HashMap::new(),
    };
    // preservation map
    let size = r.itf8_len(r.remaining(), "preservation map size")?;
    let mut p = Cursor::new(r.take(size)?);
    let n = p.itf8_len(1024, "preservation map entries")?;
    for _ in 0..n {
        let k = p.take(2)?;
        match k {
            b"RN" => h.read_names = p.u8()? != 0,
            b"AP" => h.ap_delta = p.u8()? != 0,
            b"RR" => h.reference_required = p.u8()? != 0,
            b"SM" => h.sub = sub_matrix(p.take(5)?),
            b"TD" => {
                let len = p.itf8_len(MAX_ARRAY, "tag dictionary")?;
                let td = p.take(len)?;
                for list in td.split(|b| *b == 0) {
                    let mut tags = Vec::new();
                    for t in list.chunks(3) {
                        if t.len() != 3 {
                            return Err("tag dictionary entry is not 3 bytes".into());
                        }
                        let key =
                            (i32::from(t[0]) << 16) | (i32::from(t[1]) << 8) | i32::from(t[2]);
                        tags.push(([t[0], t[1]], t[2], key));
                    }
                    h.tag_dict.push(tags);
                }
                // a trailing nul ends the last list: drop the empty list after it
                if td.last() == Some(&0) && h.tag_dict.last().is_some_and(Vec::is_empty) {
                    h.tag_dict.pop();
                }
            }
            // unknown keys: their size is unknown, so stop reading the map
            _ => break,
        }
    }
    // data series encodings
    let size = r.itf8_len(r.remaining(), "encoding map size")?;
    let mut e = Cursor::new(r.take(size)?);
    let n = e.itf8_len(1024, "encoding map entries")?;
    for _ in 0..n {
        let k = e.take(2)?;
        let enc = parse(&mut e)?;
        h.series.insert([k[0], k[1]], enc);
    }
    // tag encodings
    let size = r.itf8_len(r.remaining(), "tag encoding map size")?;
    let mut t = Cursor::new(r.take(size)?);
    let n = t.itf8_len(1 << 16, "tag encoding map entries")?;
    for _ in 0..n {
        let k = t.itf8()?;
        let enc = parse(&mut t)?;
        h.tags.insert(k, enc);
    }
    Ok(h)
}

/// The blocks a slice's values are read from.
pub(crate) struct Streams<'a> {
    pub(crate) core: BitReader<'a>,
    pub(crate) external: HashMap<i32, Cursor<'a>>,
}

impl<'a> Streams<'a> {
    fn ext(&mut self, id: i32) -> &mut Cursor<'a> {
        // a series whose block is missing from the slice holds no data
        self.external.entry(id).or_insert_with(|| Cursor::new(&[]))
    }

    /// One integer of a series.
    pub(crate) fn int(&mut self, enc: &Encoding) -> DecodeResult<i32> {
        match enc {
            Encoding::External(id) => self.ext(*id).itf8(),
            Encoding::Huffman(h) => h.decode(&mut self.core),
            Encoding::Beta { offset, bits } => {
                Ok((self.core.bits(*bits)? as i32).wrapping_sub(*offset))
            }
            Encoding::Subexp { offset, k } => {
                let mut u = 0u32;
                while self.core.bit()? == 1 {
                    u += 1;
                    if u > 32 {
                        return Err("subexponential code too long".into());
                    }
                }
                let n: u64 = if u == 0 {
                    u64::from(self.core.bits(*k)?)
                } else {
                    let b = u + k - 1;
                    if b > 32 {
                        return Err("subexponential code too long".into());
                    }
                    (1u64 << b) + u64::from(self.core.bits(b)?)
                };
                Ok((n as i64 - i64::from(*offset)) as i32)
            }
            Encoding::Gamma { offset } => {
                let mut n = 0u32;
                while self.core.bit()? == 0 {
                    n += 1;
                    if n > 32 {
                        return Err("gamma code too long".into());
                    }
                }
                let v = (1u64 << n) | u64::from(self.core.bits(n)?);
                Ok((v as i64 - i64::from(*offset)) as i32)
            }
            Encoding::Golomb { offset, m } => {
                let mut q = 0u64;
                while self.core.bit()? == 1 {
                    q += 1;
                    if q > 1 << 20 {
                        return Err("Golomb code too long".into());
                    }
                }
                let b = 32 - (m - 1).leading_zeros();
                let cut = (1u64 << b) - u64::from(*m);
                let mut r = u64::from(self.core.bits(b.saturating_sub(1))?);
                if r >= cut {
                    r = r * 2 + u64::from(self.core.bit()?) - cut;
                }
                Ok(((q * u64::from(*m) + r) as i32).wrapping_sub(*offset))
            }
            Encoding::GolombRice { offset, log2m } => {
                let mut q = 0u64;
                while self.core.bit()? == 1 {
                    q += 1;
                    if q > 1 << 20 {
                        return Err("Golomb-Rice code too long".into());
                    }
                }
                let r = u64::from(self.core.bits(*log2m)?);
                Ok((((q << log2m) + r) as i32).wrapping_sub(*offset))
            }
            Encoding::Null => Err("a data series the record needs is not stored".into()),
            Encoding::ByteArrayLen(..) | Encoding::ByteArrayStop(..) => {
                Err("an integer series has a byte-array encoding".into())
            }
        }
    }

    /// One byte of a series.
    pub(crate) fn byte(&mut self, enc: &Encoding) -> DecodeResult<u8> {
        match enc {
            Encoding::External(id) => self.ext(*id).u8(),
            _ => self.int(enc).map(|v| v as u8),
        }
    }

    /// One byte array of a series.
    pub(crate) fn bytes(&mut self, enc: &Encoding) -> DecodeResult<Vec<u8>> {
        match enc {
            Encoding::ByteArrayLen(len, val) => {
                let n = usize::try_from(self.int(len)?)
                    .ok()
                    .filter(|n| *n <= MAX_ARRAY)
                    .ok_or("byte array of negative or huge length")?;
                if let Encoding::External(id) = val.as_ref() {
                    return Ok(self.ext(*id).take(n)?.to_vec());
                }
                (0..n).map(|_| self.byte(val)).collect()
            }
            Encoding::ByteArrayStop(stop, id) => {
                let c = self.ext(*id);
                let rest = c.rest();
                let end = rest
                    .iter()
                    .position(|b| b == stop)
                    .ok_or("byte array without its stop byte")?;
                let v = rest[..end].to_vec();
                c.skip(end + 1)?;
                Ok(v)
            }
            Encoding::External(id) => Err(format!(
                "a byte-array series is stored as plain EXTERNAL (block {id})"
            )),
            other => Err(format!("a byte-array series has encoding {other:?}")),
        }
    }

    /// `n` bytes of a byte series (qualities, bases).
    pub(crate) fn byte_run(&mut self, enc: &Encoding, n: usize) -> DecodeResult<Vec<u8>> {
        if let Encoding::External(id) = enc {
            return Ok(self.ext(*id).take(n)?.to_vec());
        }
        (0..n).map(|_| self.byte(enc)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitution_matrix_example() {
        // the specification's example matrix
        let m = sub_matrix(&[0x63, 0x4b, 0x87, 0x27, 0x1b]);
        assert_eq!(m[0], *b"TCGN");
        assert_eq!(m[1], *b"GATN");
        assert_eq!(m[4], *b"ACGT");
    }

    #[test]
    fn canonical_huffman() {
        // A=1, B..D=3, E,F=4 bits: 0, 100, 101, 110, 1110, 1111
        let h = Huffman::new(vec![65, 66, 67, 68, 69, 70], vec![1, 3, 3, 3, 4, 4]).unwrap();
        let bits = [0b0100_1011, 0b1101_1110];
        let mut r = BitReader::new(&bits);
        let got: Vec<i32> = (0..5).map(|_| h.decode(&mut r).unwrap()).collect();
        assert_eq!(got, vec![65, 66, 67, 69, 70]);
        let one = Huffman::new(vec![7], vec![0]).unwrap();
        assert_eq!(one.decode(&mut BitReader::new(&[])).unwrap(), 7);
    }
}
