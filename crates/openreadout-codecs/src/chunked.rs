//! Chunked compression of CZI subblocks (compression id 7), as libCZI's public documentation
//! describes it (<https://zeiss.github.io/libczi/pages/chunked_compression.html>), checked
//! against streams written by imagecodecs (BSD-3-Clause, run as a black box).
//!
//! The stream starts with header entries, each a varint id, a varint payload length and the
//! payload, ended by id 0. Varints are little-endian groups of seven bits, the high bit set
//! on every byte but the last. Ids: 1 the compressed size of each chunk (varints, required),
//! 2 the codec (one byte: 0 zstd, 1 LZ4; zstd when absent), 3 the decompressed chunk sizes
//! (required), 4 the preprocessing (one byte: 0 none, 1 the HiLo byte split of 16-bit
//! samples). The compressed chunks follow back to back.
//!
//! Entry 3 takes the forms the page documents: `[C]` (every chunk decompresses to C bytes),
//! `[C, L]` (every chunk but the last to C, the last to L), or one size per chunk. Other
//! lengths of that list are refused.
//!
//! HiLo with more than one chunk is refused: the page says the decoder reverses it after
//! decompressing the chunks, while imagecodecs (and so czifile) split each chunk on its own,
//! and no file written by ZEN or libCZI was available to settle which is meant. With one chunk
//! the two readings agree. Unknown header ids, codecs and preprocessing values are refused.

use crate::{CodecError, Result};

const CODEC: &str = "chunked";

fn bad(detail: impl Into<String>) -> CodecError {
    CodecError::Decode {
        codec: CODEC,
        detail: detail.into(),
    }
}

fn varint(d: &[u8], i: &mut usize) -> Result<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *d
            .get(*i)
            .ok_or_else(|| bad("header ends inside a number"))?;
        *i += 1;
        v |= u64::from(b & 0x7F)
            .checked_shl(shift)
            .filter(|x| x >> shift == u64::from(b & 0x7F))
            .ok_or_else(|| bad("number in the header overflows"))?;
        if b & 0x80 == 0 {
            return Ok(v);
        }
    }
    Err(bad("number in the header is too long"))
}

fn varints(d: &[u8]) -> Result<Vec<u64>> {
    let mut i = 0;
    let mut out = Vec::new();
    while i < d.len() {
        out.push(varint(d, &mut i)?);
    }
    Ok(out)
}

/// Decode a chunked payload to `expected_len` bytes (0 = unknown). `bytes_per_sample` is
/// the size of one sample of the pixel type, needed to undo the HiLo split.
pub fn chunked_decode(
    data: &[u8],
    expected_len: usize,
    bytes_per_sample: usize,
) -> Result<Vec<u8>> {
    let mut i = 0usize;
    let mut sizes: Option<Vec<u64>> = None;
    let mut raw_sizes: Option<Vec<u64>> = None;
    let mut method = 0u8;
    let mut hilo = false;
    loop {
        let id = varint(data, &mut i)?;
        if id == 0 {
            break;
        }
        let len =
            usize::try_from(varint(data, &mut i)?).map_err(|_| bad("header entry too long"))?;
        let payload = i
            .checked_add(len)
            .and_then(|e| data.get(i..e))
            .ok_or_else(|| bad("header entry runs past the data"))?;
        i += len;
        match id {
            1 => sizes = Some(varints(payload)?),
            2 => {
                method = *payload.first().ok_or_else(|| bad("empty codec entry"))?;
            }
            3 => raw_sizes = Some(varints(payload)?),
            4 => match payload.first() {
                Some(0) => hilo = false,
                Some(1) => hilo = true,
                Some(v) => {
                    return Err(CodecError::Unsupported {
                        codec: CODEC,
                        detail: format!("preprocessing {v}"),
                    });
                }
                None => return Err(bad("empty preprocessing entry")),
            },
            other => {
                return Err(CodecError::Unsupported {
                    codec: CODEC,
                    detail: format!("header entry {other}"),
                });
            }
        }
    }
    let sizes = sizes.ok_or_else(|| bad("no chunk sizes in the header"))?;
    let raw_sizes = raw_sizes.ok_or_else(|| bad("no decompressed sizes in the header"))?;
    if method > 1 {
        return Err(CodecError::Unsupported {
            codec: CODEC,
            detail: format!("codec {method}"),
        });
    }
    if hilo && bytes_per_sample != 2 {
        return Err(CodecError::Unsupported {
            codec: CODEC,
            detail: format!("HiLo split of {bytes_per_sample}-byte samples"),
        });
    }
    let n = sizes.len();
    if hilo && n > 1 {
        return Err(CodecError::Unsupported {
            codec: CODEC,
            detail: format!("HiLo split across {n} chunks"),
        });
    }
    // The decompressed size of chunk k.
    let raw_size = |k: usize| -> Result<u64> {
        match raw_sizes.len() {
            m if m == n => Ok(raw_sizes[k]),
            1 => Ok(raw_sizes[0]),
            2 => Ok(if k + 1 == n {
                raw_sizes[1]
            } else {
                raw_sizes[0]
            }),
            m => Err(CodecError::Unsupported {
                codec: CODEC,
                detail: format!("{m} decompressed sizes for {n} chunks"),
            }),
        }
    };
    let limit = if expected_len == 0 {
        crate::MAX_UNSIZED_OUTPUT
    } else {
        expected_len
    };
    let mut out = Vec::with_capacity(limit.min(crate::MAX_PREALLOC));
    for (k, &csize) in sizes.iter().enumerate() {
        let csize = usize::try_from(csize).map_err(|_| bad("chunk size overflows"))?;
        let chunk = i
            .checked_add(csize)
            .and_then(|e| data.get(i..e))
            .ok_or_else(|| bad(format!("chunk {k} runs past the data")))?;
        i += csize;
        let room = limit.saturating_sub(out.len());
        let want = usize::try_from(raw_size(k)?).unwrap_or(usize::MAX);
        if want > room {
            return Err(bad(format!("chunk {k} decodes past the expected size")));
        }
        let mut d = match method {
            0 => crate::zstd_decode(chunk, want)?,
            _ => crate::lz4_block_decode(chunk, want)?,
        };
        if d.len() != want {
            return Err(bad(format!(
                "chunk {k} decodes to {} bytes, expected {want}",
                d.len()
            )));
        }
        if hilo {
            if d.len() % 2 != 0 {
                return Err(bad(format!("chunk {k} holds half a 16-bit sample")));
            }
            d = crate::unshuffle_hilo(&d);
        }
        out.extend_from_slice(&d);
    }
    if expected_len != 0 && out.len() != expected_len {
        return Err(CodecError::SizeMismatch {
            codec: CODEC,
            got: out.len(),
            expected: expected_len,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_are_seven_bit_groups() {
        let mut i = 0;
        assert_eq!(varint(&[0xCA, 0x27], &mut i).unwrap(), 5066);
        assert_eq!(i, 2);
        assert!(varint(&[0x80], &mut 0).is_err());
        assert!(varint(&[0xFF; 11], &mut 0).is_err());
    }

    #[test]
    fn malformed_headers_are_refused() {
        assert!(chunked_decode(&[], 0, 1).is_err());
        assert!(chunked_decode(&[0x00], 0, 1).is_err()); // no sizes
        assert!(chunked_decode(&[0x09, 0x01, 0x00, 0x00], 0, 1).is_err()); // unknown id
        assert!(chunked_decode(&[0x01, 0x05, 0x01], 0, 1).is_err()); // payload past end
        // codec 7 is refused as unsupported
        let r = chunked_decode(
            &[0x01, 0x01, 0x00, 0x03, 0x01, 0x00, 0x02, 0x01, 0x07, 0x00],
            0,
            1,
        );
        assert!(matches!(r, Err(CodecError::Unsupported { .. })));
        // HiLo over two chunks is refused before anything is decompressed
        let r = chunked_decode(
            &[
                0x01, 0x02, 0x00, 0x00, 0x03, 0x01, 0x00, 0x04, 0x01, 0x01, 0x00,
            ],
            0,
            2,
        );
        assert!(matches!(r, Err(CodecError::Unsupported { .. })));
    }

    /// Two LZ4 chunks of a 6-byte buffer in the `[C, L]` form: 4 bytes, then 2.
    #[cfg(feature = "lz4")]
    #[test]
    fn two_size_form_gives_the_last_chunk_its_own_size() {
        // LZ4 blocks of literals only: token 0x40 = 4 literals, 0x20 = 2 literals.
        let stream = [
            0x01, 0x02, 0x05, 0x03, // compressed sizes 5 and 3
            0x03, 0x02, 0x04, 0x02, // decompressed: 4 each, the last 2
            0x02, 0x01, 0x01, // LZ4
            0x00, // end of header
            0x40, 1, 2, 3, 4, 0x20, 5, 6,
        ];
        assert_eq!(chunked_decode(&stream, 6, 1).unwrap(), [1, 2, 3, 4, 5, 6]);
        assert!(chunked_decode(&stream, 7, 1).is_err());
    }
}
