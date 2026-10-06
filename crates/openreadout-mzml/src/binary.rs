//! Binary data arrays: base64, compression and numeric encodings of mzML and mzXML.

use crate::numpress;

/// Numeric type of the stored values (after decompression).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    Float32,
    Float64,
    Int32,
    Int64,
}

impl ValueType {
    pub fn width(self) -> usize {
        match self {
            ValueType::Float32 | ValueType::Int32 => 4,
            ValueType::Float64 | ValueType::Int64 => 8,
        }
    }
}

/// How the bytes were compressed or encoded before base64.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    NoCompression,
    Zlib,
    Zstd,
    /// MS-Numpress, optionally followed by a general-purpose compressor.
    NumpressLinear(Outer),
    NumpressPic(Outer),
    NumpressSlof(Outer),
    /// A compression term we recognise but do not decode (the accession is kept for the message).
    Unsupported(&'static str),
}

/// A general-purpose compressor applied after MS-Numpress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outer {
    Plain,
    Zlib,
    Zstd,
}

/// Everything needed to decode one array.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArrayEncoding {
    pub value_type: ValueType,
    pub compression: Compression,
    /// mzXML "network" byte order; mzML is always little-endian.
    pub big_endian: bool,
}

/// Why an array could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArrayError {
    Base64(String),
    Decompress(String),
    Layout(String),
    Unsupported(String),
}

impl std::fmt::Display for ArrayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArrayError::Base64(m) => write!(f, "base64: {m}"),
            ArrayError::Decompress(m) => write!(f, "decompression: {m}"),
            ArrayError::Layout(m) => write!(f, "{m}"),
            ArrayError::Unsupported(m) => write!(f, "unsupported encoding: {m}"),
        }
    }
}

/// The value of each base64 alphabet character; 0xFF for every other byte.
const B64_TABLE: [u8; 256] = {
    let mut t = [0xFFu8; 256];
    let mut c = 0;
    while c < 256 {
        if let Some(v) = b64_value(c as u8) {
            t[c] = v as u8;
        }
        c += 1;
    }
    t
};

const fn b64_value(c: u8) -> Option<u32> {
    Some(match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    } as u32)
}

/// Decode standard base64 (RFC 4648), ignoring ASCII whitespace. Returns the bytes and the
/// number of base64 characters seen (whitespace excluded), which `check` compares with the
/// declared `encodedLength`.
pub fn decode_base64(text: &[u8]) -> Result<(Vec<u8>, usize), ArrayError> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    // Fast path: whole groups of four alphabet characters. The loop below takes over from the
    // first group holding anything else (whitespace, padding, a bad character), with the same
    // results as if it had decoded everything.
    let mut fast = 0usize;
    for q in text.as_chunks::<4>().0 {
        let sextets = q.map(|ch| B64_TABLE[usize::from(ch)]);
        if sextets.iter().any(|&s| s & 0x80 != 0) {
            break;
        }
        let bits = sextets
            .iter()
            .fold(0u32, |acc, &s| (acc << 6) | u32::from(s));
        out.extend_from_slice(&[(bits >> 16) as u8, (bits >> 8) as u8, bits as u8]);
        fast += 4;
    }
    let mut acc = 0u32;
    let mut n = 0u8;
    let mut seen = fast;
    let mut pad = 0usize;
    for &c in &text[fast..] {
        if c.is_ascii_whitespace() {
            continue;
        }
        seen += 1;
        if c == b'=' {
            pad += 1;
            continue;
        }
        if pad > 0 {
            return Err(ArrayError::Base64("data after '=' padding".into()));
        }
        let v = b64_value(c)
            .ok_or_else(|| ArrayError::Base64(format!("invalid character {:?}", char::from(c))))?;
        acc = (acc << 6) | v;
        n += 1;
        if n == 4 {
            out.extend_from_slice(&[(acc >> 16) as u8, (acc >> 8) as u8, acc as u8]);
            acc = 0;
            n = 0;
        }
    }
    match n {
        0 => {}
        2 => out.push((acc >> 4) as u8),
        3 => out.extend_from_slice(&[(acc >> 10) as u8, (acc >> 2) as u8]),
        _ => return Err(ArrayError::Base64("truncated base64 quantum".into())),
    }
    if pad > 2 {
        return Err(ArrayError::Base64("too much '=' padding".into()));
    }
    Ok((out, seen))
}

fn inflate(data: &[u8], expected: usize) -> Result<Vec<u8>, ArrayError> {
    openreadout_codecs::zlib_decode(data, expected)
        .map_err(|e| ArrayError::Decompress(e.to_string()))
}

fn unzstd(data: &[u8]) -> Result<Vec<u8>, ArrayError> {
    openreadout_codecs::zstd_decode(data, 0).map_err(|e| ArrayError::Decompress(e.to_string()))
}

fn outer(data: Vec<u8>, o: Outer) -> Result<Vec<u8>, ArrayError> {
    match o {
        Outer::Plain => Ok(data),
        Outer::Zlib => inflate(&data, 0),
        Outer::Zstd => unzstd(&data),
    }
}

/// Decompressed bytes → values. `expected` (element count, when known) bounds the work.
pub fn decode_values(
    raw: Vec<u8>,
    enc: &ArrayEncoding,
    expected: Option<usize>,
) -> Result<Vec<f64>, ArrayError> {
    // An empty payload is an empty array whatever the compression (ProteoWizard writes an
    // empty <binary/> for zlib arrays of spectra with defaultArrayLength 0).
    if raw.is_empty() && expected.unwrap_or(0) == 0 {
        return Ok(Vec::new());
    }
    let np = |r: Result<Vec<f64>, numpress::NumpressError>| {
        r.map_err(|e| ArrayError::Layout(format!("MS-Numpress: {e}")))
    };
    let bytes = match enc.compression {
        Compression::NoCompression => raw,
        Compression::Zlib => inflate(&raw, expected.unwrap_or(0) * enc.value_type.width())?,
        Compression::Zstd => unzstd(&raw)?,
        Compression::NumpressLinear(o) => return np(numpress::decode_linear(&outer(raw, o)?)),
        Compression::NumpressPic(o) => return np(numpress::decode_pic(&outer(raw, o)?)),
        Compression::NumpressSlof(o) => return np(numpress::decode_slof(&outer(raw, o)?)),
        Compression::Unsupported(what) => return Err(ArrayError::Unsupported(what.to_string())),
    };
    let w = enc.value_type.width();
    if bytes.len() % w != 0 {
        return Err(ArrayError::Layout(format!(
            "{} bytes is not a whole number of {w}-byte values",
            bytes.len()
        )));
    }
    let be = enc.big_endian;
    Ok(match enc.value_type {
        ValueType::Float32 => bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&a| {
                f64::from(if be {
                    f32::from_be_bytes(a)
                } else {
                    f32::from_le_bytes(a)
                })
            })
            .collect(),
        ValueType::Float64 => bytes
            .as_chunks::<8>()
            .0
            .iter()
            .map(|&a| {
                if be {
                    f64::from_be_bytes(a)
                } else {
                    f64::from_le_bytes(a)
                }
            })
            .collect(),
        ValueType::Int32 => bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&a| {
                f64::from(if be {
                    i32::from_be_bytes(a)
                } else {
                    i32::from_le_bytes(a)
                })
            })
            .collect(),
        ValueType::Int64 => bytes
            .as_chunks::<8>()
            .0
            .iter()
            .map(|&a| {
                (if be {
                    i64::from_be_bytes(a)
                } else {
                    i64::from_le_bytes(a)
                }) as f64
            })
            .collect(),
    })
}

/// base64 text → values, plus the number of base64 characters.
pub fn decode_array(
    text: &[u8],
    enc: &ArrayEncoding,
    expected: Option<usize>,
) -> Result<(Vec<f64>, usize), ArrayError> {
    let (raw, chars) = decode_base64(text)?;
    Ok((decode_values(raw, enc, expected)?, chars))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_basics() {
        assert_eq!(decode_base64(b"TWFu").unwrap().0, b"Man");
        assert_eq!(decode_base64(b"TWE=").unwrap().0, b"Ma");
        assert_eq!(decode_base64(b"TQ==\n").unwrap(), (b"M".to_vec(), 4));
        assert_eq!(decode_base64(b"").unwrap().0, b"");
        assert!(decode_base64(b"TW!u").is_err());
        assert!(decode_base64(b"T").is_err());
    }

    #[test]
    fn base64_fast_path_agrees_with_the_slow_one() {
        // A leading space sends the whole text through the character-by-character loop.
        let cases: [&[u8]; 8] = [
            b"TWFuTWFuTWFu",
            b"TWFuTWFuTWE=",
            b"TWFuTWFuTQ==",
            b"TWFuTW\nFuTWFu",
            b"TWFuTWFuTWF",
            b"TWFuTW=uTWFu",
            b"TWFuTWFu!WFu",
            b"TWFuTWFuTWFu====",
        ];
        for text in cases {
            let slow = [b" ".as_slice(), text].concat();
            assert_eq!(
                decode_base64(text),
                decode_base64(&slow),
                "{}",
                String::from_utf8_lossy(text)
            );
        }
    }

    #[test]
    fn floats_both_orders() {
        let le = ArrayEncoding {
            value_type: ValueType::Float32,
            compression: Compression::NoCompression,
            big_endian: false,
        };
        let v = decode_values(1.5f32.to_le_bytes().to_vec(), &le, None).unwrap();
        assert_eq!(v, vec![1.5]);
        let be = ArrayEncoding {
            big_endian: true,
            value_type: ValueType::Float64,
            ..le
        };
        let v = decode_values(2.25f64.to_be_bytes().to_vec(), &be, None).unwrap();
        assert_eq!(v, vec![2.25]);
        assert!(decode_values(vec![0; 3], &le, None).is_err());
        // an empty zlib payload (defaultArrayLength 0) is an empty array
        let z = ArrayEncoding {
            compression: Compression::Zlib,
            ..le
        };
        assert!(decode_values(Vec::new(), &z, Some(0)).unwrap().is_empty());
        assert!(decode_values(Vec::new(), &z, Some(3)).is_err());
    }
}
