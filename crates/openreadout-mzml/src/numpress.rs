//! MS-Numpress decoders (linear prediction, positive integer, short logged float).
//!
//! Written from the description in Teleman et al., MCP 13(6):1537 (2014) and the dual
//! Apache-2.0/BSD-3-Clause reference implementation read as documentation (see
//! `docs/provenance/mzml.md`). Every read is bounds-checked; malformed input is an error.

/// Why a numpress payload could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumpressError(pub String);

impl std::fmt::Display for NumpressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn err<T>(msg: &str) -> Result<T, NumpressError> {
    Err(NumpressError(msg.to_string()))
}

/// The 8-byte fixed point that starts linear and slof payloads: an IEEE-754 double, big-endian.
fn fixed_point(data: &[u8]) -> Result<f64, NumpressError> {
    let b: [u8; 8] = data
        .get(..8)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| NumpressError("not enough bytes for the fixed point".into()))?;
    Ok(f64::from_be_bytes(b))
}

/// Reads half-byte (nibble) packed integers.
struct Nibbles<'a> {
    data: &'a [u8],
    /// Next byte.
    at: usize,
    /// 0: the high nibble of `data[at]` is next; 1: the low nibble.
    half: u8,
}

impl Nibbles<'_> {
    fn next_nibble(&mut self) -> Result<u32, NumpressError> {
        let b = *self
            .data
            .get(self.at)
            .ok_or_else(|| NumpressError("packed integer runs past the end".into()))?;
        let v = if self.half == 0 {
            b >> 4
        } else {
            self.at += 1;
            b & 0xf
        };
        self.half ^= 1;
        Ok(u32::from(v))
    }

    /// True when only a single zero padding nibble is left.
    fn at_padding(&self) -> bool {
        self.at + 1 == self.data.len() && self.half == 1 && self.data[self.at] & 0xf == 0
    }

    fn done(&self) -> bool {
        self.at >= self.data.len()
    }

    /// One integer: a head nibble `n` (0–8: that many leading zero nibbles; 9–15: `n - 8`
    /// leading `0xF` nibbles), then the remaining nibbles, least significant first.
    fn next_int(&mut self) -> Result<u32, NumpressError> {
        let head = self.next_nibble()?;
        let (lead, mut v) = if head <= 8 {
            (head, 0u32)
        } else {
            let n = head - 8;
            let mut v = 0u32;
            for i in 0..n {
                v |= 0xf000_0000u32 >> (4 * i);
            }
            (n, v)
        };
        for i in lead..8 {
            let nib = self.next_nibble()?;
            v |= nib << ((i - lead) * 4);
        }
        Ok(v)
    }
}

/// Decode `MS-Numpress linear prediction compression` (MS:1002312).
pub fn decode_linear(data: &[u8]) -> Result<Vec<f64>, NumpressError> {
    if data.len() == 8 {
        return Ok(Vec::new());
    }
    let fp = fixed_point(data)?;
    if !(fp.is_finite() && fp != 0.0) {
        return err("fixed point is zero or not finite");
    }
    let first = |at: usize| -> Result<i64, NumpressError> {
        let b: [u8; 4] = data
            .get(at..at + 4)
            .and_then(|s| s.try_into().ok())
            .ok_or_else(|| NumpressError("not enough bytes for the first values".into()))?;
        Ok(i64::from(u32::from_le_bytes(b)))
    };
    let mut out = Vec::with_capacity(data.len().saturating_sub(8).saturating_mul(2));
    let mut prev2 = first(8)?;
    out.push(prev2 as f64 / fp);
    if data.len() == 12 {
        return Ok(out);
    }
    let mut prev1 = first(12)?;
    out.push(prev1 as f64 / fp);
    let mut nib = Nibbles {
        data: data.get(16..).unwrap_or(&[]),
        at: 0,
        half: 0,
    };
    while !nib.done() {
        if nib.at_padding() {
            break;
        }
        let diff = i64::from(nib.next_int()?.cast_signed());
        let extrapolated = prev1
            .checked_mul(2)
            .and_then(|x| x.checked_sub(prev2))
            .and_then(|x| x.checked_add(diff))
            .ok_or_else(|| NumpressError("linear prediction overflows".into()))?;
        out.push(extrapolated as f64 / fp);
        prev2 = prev1;
        prev1 = extrapolated;
    }
    Ok(out)
}

/// Decode `MS-Numpress positive integer compression` (MS:1002313).
pub fn decode_pic(data: &[u8]) -> Result<Vec<f64>, NumpressError> {
    let mut out = Vec::with_capacity(data.len().saturating_mul(2));
    let mut nib = Nibbles {
        data,
        at: 0,
        half: 0,
    };
    while !nib.done() {
        if nib.at_padding() {
            break;
        }
        out.push(f64::from(nib.next_int()?));
    }
    Ok(out)
}

/// Decode `MS-Numpress short logged float compression` (MS:1002314).
pub fn decode_slof(data: &[u8]) -> Result<Vec<f64>, NumpressError> {
    let fp = fixed_point(data)?;
    if !(fp.is_finite() && fp != 0.0) {
        return err("fixed point is zero or not finite");
    }
    let body = &data[8..];
    if !body.len().is_multiple_of(2) {
        return err("odd number of bytes after the fixed point");
    }
    Ok(body
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&c| (f64::from(u16::from_le_bytes(c)) / fp).exp() - 1.0)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encoder used only to test the decoders (mirrors the published layout).
    fn push_int(x: u32, nibbles: &mut Vec<u8>) {
        let top = x & 0xf000_0000;
        if top == 0 {
            let lead = (0..8)
                .find(|&i| x & (0xf000_0000u32 >> (4 * i)) != 0)
                .unwrap_or(8);
            nibbles.push(lead as u8);
            for i in lead..8 {
                nibbles.push(((x >> (4 * (i - lead))) & 0xf) as u8);
            }
        } else if top == 0xf000_0000 {
            let lead = (0..8)
                .find(|&i| x & (0xf000_0000u32 >> (4 * i)) != (0xf000_0000u32 >> (4 * i)))
                .unwrap_or(7);
            nibbles.push(lead as u8 + 8);
            for i in lead..8 {
                nibbles.push(((x >> (4 * (i - lead))) & 0xf) as u8);
            }
        } else {
            nibbles.push(0);
            for i in 0..8 {
                nibbles.push(((x >> (4 * i)) & 0xf) as u8);
            }
        }
    }

    fn pack(nibbles: &[u8]) -> Vec<u8> {
        nibbles
            .chunks(2)
            .map(|c| (c[0] << 4) | c.get(1).copied().unwrap_or(0))
            .collect()
    }

    #[test]
    fn pic_roundtrip() {
        let vals = [0u32, 1, 15, 16, 255, 65_535, 1_000_000, 7];
        let mut n = Vec::new();
        for v in vals {
            push_int(v, &mut n);
        }
        let got = decode_pic(&pack(&n)).unwrap();
        assert_eq!(got, vals.iter().map(|&v| f64::from(v)).collect::<Vec<_>>());
    }

    #[test]
    fn linear_roundtrip() {
        let fp = 1000.0f64;
        let ints: Vec<i64> = vec![100_000, 100_500, 101_010, 101_490, 102_100, 102_000];
        let mut data = fp.to_be_bytes().to_vec();
        data.extend_from_slice(&(ints[0] as u32).to_le_bytes());
        data.extend_from_slice(&(ints[1] as u32).to_le_bytes());
        let mut n = Vec::new();
        for i in 2..ints.len() {
            let diff = ints[i] - (2 * ints[i - 1] - ints[i - 2]);
            push_int((diff as i32).cast_unsigned(), &mut n);
        }
        data.extend(pack(&n));
        let got = decode_linear(&data).unwrap();
        let want: Vec<f64> = ints.iter().map(|&v| v as f64 / fp).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn slof_values() {
        let fp = 100.0f64;
        let mut data = fp.to_be_bytes().to_vec();
        for x in [0u16, 100, 693] {
            data.extend_from_slice(&x.to_le_bytes());
        }
        let got = decode_slof(&data).unwrap();
        assert!(got[0].abs() < 1e-12);
        assert!((got[1] - (1f64.exp() - 1.0)).abs() < 1e-12);
    }

    #[test]
    fn malformed_is_an_error() {
        assert!(decode_linear(&[1, 2, 3]).is_err());
        assert!(decode_slof(&[0; 9]).is_err());
        // head nibble promises 8 more nibbles that are not there
        assert!(decode_pic(&[0x00]).is_err());
    }
}
