//! LZMA2 (raw chunk stream, as inside `.xz` blocks and 7-Zip archives) decoder, written from
//! the public-domain LZMA specification (`lzma-specification.txt` in Igor Pavlov's LZMA SDK)
//! and the LZMA2 chunk layout documented with xz (`xz-file-format.txt`, public domain).
//!
//! The whole output is the dictionary, so a match may reach back to any byte written since
//! the last dictionary reset. Every read is bounds-checked; malformed input is an error.

use crate::{CodecError, MAX_PREALLOC, Result, output_limit};

const CODEC: &str = "lzma2";

fn err(detail: impl Into<String>) -> CodecError {
    CodecError::Decode {
        codec: CODEC,
        detail: detail.into(),
    }
}

const PROB_INIT: u16 = 1024;
const NUM_STATES: usize = 12;
const POS_STATES_MAX: usize = 1 << 4;
const END_POS_MODEL_INDEX: u32 = 14;
const NUM_FULL_DISTANCES: usize = 1 << (END_POS_MODEL_INDEX >> 1);
const NUM_ALIGN_BITS: u32 = 4;
const MATCH_MIN_LEN: usize = 2;

/// Range decoder over one LZMA chunk's compressed bytes.
struct RangeDecoder<'a> {
    data: &'a [u8],
    pos: usize,
    range: u32,
    code: u32,
}

impl<'a> RangeDecoder<'a> {
    fn new(data: &'a [u8]) -> Result<Self> {
        if data.len() < 5 {
            return Err(err("chunk shorter than the range coder's 5 initial bytes"));
        }
        if data[0] != 0 {
            return Err(err("range coder does not start with a zero byte"));
        }
        let code = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
        if code == u32::MAX {
            return Err(err("range coder initial code is invalid"));
        }
        Ok(RangeDecoder {
            data,
            pos: 5,
            range: u32::MAX,
            code,
        })
    }

    fn next_byte(&mut self) -> Result<u8> {
        let b = *self
            .data
            .get(self.pos)
            .ok_or_else(|| err("compressed chunk ends early"))?;
        self.pos += 1;
        Ok(b)
    }

    fn normalize(&mut self) -> Result<()> {
        if self.range < (1 << 24) {
            self.range <<= 8;
            self.code = (self.code << 8) | u32::from(self.next_byte()?);
        }
        Ok(())
    }

    fn bit(&mut self, prob: &mut u16) -> Result<u32> {
        let bound = (self.range >> 11) * u32::from(*prob);
        let bit = if self.code < bound {
            self.range = bound;
            *prob += (2048 - *prob) >> 5;
            0
        } else {
            self.range -= bound;
            self.code -= bound;
            *prob -= *prob >> 5;
            1
        };
        self.normalize()?;
        Ok(bit)
    }

    fn direct_bits(&mut self, n: u32) -> Result<u32> {
        let mut res = 0u32;
        for _ in 0..n {
            self.range >>= 1;
            let bit = if self.code >= self.range {
                self.code -= self.range;
                1
            } else {
                0
            };
            res = (res << 1) | bit;
            self.normalize()?;
        }
        Ok(res)
    }

    fn tree(&mut self, probs: &mut [u16], bits: u32) -> Result<u32> {
        let mut m = 1usize;
        for _ in 0..bits {
            let p = probs.get_mut(m).ok_or_else(|| err("probability index"))?;
            m = (m << 1) + self.bit(p)? as usize;
        }
        Ok((m - (1usize << bits)) as u32)
    }

    fn reverse_tree(&mut self, probs: &mut [u16], bits: u32) -> Result<u32> {
        let mut m = 1usize;
        let mut sym = 0u32;
        for i in 0..bits {
            let p = probs.get_mut(m).ok_or_else(|| err("probability index"))?;
            let b = self.bit(p)?;
            m = (m << 1) + b as usize;
            sym |= b << i;
        }
        Ok(sym)
    }
}

struct LenDecoder {
    choice: u16,
    choice2: u16,
    low: Vec<[u16; 8]>,
    mid: Vec<[u16; 8]>,
    high: [u16; 256],
}

impl LenDecoder {
    fn new() -> Self {
        LenDecoder {
            choice: PROB_INIT,
            choice2: PROB_INIT,
            low: vec![[PROB_INIT; 8]; POS_STATES_MAX],
            mid: vec![[PROB_INIT; 8]; POS_STATES_MAX],
            high: [PROB_INIT; 256],
        }
    }

    /// Match length minus 2.
    fn decode(&mut self, rc: &mut RangeDecoder, pos_state: usize) -> Result<usize> {
        if rc.bit(&mut self.choice)? == 0 {
            return Ok(rc.tree(&mut self.low[pos_state], 3)? as usize);
        }
        if rc.bit(&mut self.choice2)? == 0 {
            return Ok(8 + rc.tree(&mut self.mid[pos_state], 3)? as usize);
        }
        Ok(16 + rc.tree(&mut self.high, 8)? as usize)
    }
}

/// Probabilities and state that LZMA2 keeps across chunks unless a chunk resets them.
struct LzmaState {
    lc: u32,
    lp: u32,
    pb: u32,
    literal: Vec<u16>,
    pos_slot: [[u16; 64]; 4],
    pos_special: [u16; 1 + NUM_FULL_DISTANCES - END_POS_MODEL_INDEX as usize],
    align: [u16; 1 << NUM_ALIGN_BITS],
    is_match: [u16; NUM_STATES * POS_STATES_MAX],
    is_rep: [u16; NUM_STATES],
    is_rep_g0: [u16; NUM_STATES],
    is_rep_g1: [u16; NUM_STATES],
    is_rep_g2: [u16; NUM_STATES],
    is_rep0_long: [u16; NUM_STATES * POS_STATES_MAX],
    len: LenDecoder,
    rep_len: LenDecoder,
    state: usize,
    reps: [usize; 4],
}

impl LzmaState {
    fn new(lc: u32, lp: u32, pb: u32) -> Self {
        LzmaState {
            lc,
            lp,
            pb,
            literal: vec![PROB_INIT; 0x300 << (lc + lp)],
            pos_slot: [[PROB_INIT; 64]; 4],
            pos_special: [PROB_INIT; 1 + NUM_FULL_DISTANCES - END_POS_MODEL_INDEX as usize],
            align: [PROB_INIT; 1 << NUM_ALIGN_BITS],
            is_match: [PROB_INIT; NUM_STATES * POS_STATES_MAX],
            is_rep: [PROB_INIT; NUM_STATES],
            is_rep_g0: [PROB_INIT; NUM_STATES],
            is_rep_g1: [PROB_INIT; NUM_STATES],
            is_rep_g2: [PROB_INIT; NUM_STATES],
            is_rep0_long: [PROB_INIT; NUM_STATES * POS_STATES_MAX],
            len: LenDecoder::new(),
            rep_len: LenDecoder::new(),
            state: 0,
            reps: [0; 4],
        }
    }

    fn distance(&mut self, rc: &mut RangeDecoder, len: usize) -> Result<usize> {
        let len_state = len.min(3);
        let slot = rc.tree(&mut self.pos_slot[len_state], 6)?;
        if slot < 4 {
            return Ok(slot as usize);
        }
        let direct = (slot >> 1) - 1;
        let mut dist = (2 | (slot & 1)) << direct;
        if slot < END_POS_MODEL_INDEX {
            let base = (dist - slot) as usize;
            let probs = self
                .pos_special
                .get_mut(base..)
                .ok_or_else(|| err("distance probabilities"))?;
            dist += rc.reverse_tree(probs, direct)?;
        } else {
            dist += rc.direct_bits(direct - NUM_ALIGN_BITS)? << NUM_ALIGN_BITS;
            dist += rc.reverse_tree(&mut self.align, NUM_ALIGN_BITS)?;
        }
        Ok(dist as usize)
    }

    /// Decode one LZMA chunk producing exactly `unpacked` bytes into `out`.
    fn chunk(
        &mut self,
        rc: &mut RangeDecoder,
        out: &mut Vec<u8>,
        dict_start: usize,
        unpacked: usize,
    ) -> Result<()> {
        let end = out
            .len()
            .checked_add(unpacked)
            .ok_or_else(|| err("size overflow"))?;
        let pb_mask = (1usize << self.pb) - 1;
        let lp_mask = (1usize << self.lp) - 1;
        while out.len() < end {
            let pos = out.len() - dict_start;
            let pos_state = pos & pb_mask;
            let s = self.state;
            if rc.bit(&mut self.is_match[s * POS_STATES_MAX + pos_state])? == 0 {
                let prev = if pos > 0 { out[out.len() - 1] } else { 0 };
                let lit_state = ((pos & lp_mask) << self.lc) + (usize::from(prev) >> (8 - self.lc));
                let base = 0x300 * lit_state;
                let probs = self
                    .literal
                    .get_mut(base..base + 0x300)
                    .ok_or_else(|| err("literal probabilities"))?;
                let mut sym = 1usize;
                if s >= 7 {
                    let back = self.reps[0] + 1;
                    if back > pos {
                        return Err(err("match distance before the dictionary start"));
                    }
                    let mut match_byte = usize::from(out[out.len() - back]);
                    while sym < 0x100 {
                        let match_bit = (match_byte >> 7) & 1;
                        match_byte <<= 1;
                        let bit = rc.bit(&mut probs[((1 + match_bit) << 8) + sym])? as usize;
                        sym = (sym << 1) | bit;
                        if match_bit != bit {
                            break;
                        }
                    }
                }
                while sym < 0x100 {
                    sym = (sym << 1) | rc.bit(&mut probs[sym])? as usize;
                }
                out.push((sym - 0x100) as u8);
                self.state = if s < 4 {
                    0
                } else if s < 10 {
                    s - 3
                } else {
                    s - 6
                };
                continue;
            }
            let len;
            if rc.bit(&mut self.is_rep[s])? == 0 {
                len = self.len.decode(rc, pos_state)?;
                self.state = if s < 7 { 7 } else { 10 };
                let dist = self.distance(rc, len)?;
                if dist == u32::MAX as usize {
                    return Err(err("end marker inside an LZMA2 chunk"));
                }
                self.reps = [dist, self.reps[0], self.reps[1], self.reps[2]];
            } else {
                if rc.bit(&mut self.is_rep_g0[s])? == 0 {
                    if rc.bit(&mut self.is_rep0_long[s * POS_STATES_MAX + pos_state])? == 0 {
                        // short rep: one byte at rep0
                        self.state = if s < 7 { 9 } else { 11 };
                        let back = self.reps[0] + 1;
                        if back > pos {
                            return Err(err("match distance before the dictionary start"));
                        }
                        let b = out[out.len() - back];
                        out.push(b);
                        continue;
                    }
                } else {
                    let dist;
                    if rc.bit(&mut self.is_rep_g1[s])? == 0 {
                        dist = self.reps[1];
                    } else {
                        if rc.bit(&mut self.is_rep_g2[s])? == 0 {
                            dist = self.reps[2];
                        } else {
                            dist = self.reps[3];
                            self.reps[3] = self.reps[2];
                        }
                        self.reps[2] = self.reps[1];
                    }
                    self.reps[1] = self.reps[0];
                    self.reps[0] = dist;
                }
                len = self.rep_len.decode(rc, pos_state)?;
                self.state = if s < 7 { 8 } else { 11 };
            }
            let n = len + MATCH_MIN_LEN;
            let back = self.reps[0] + 1;
            if back > pos {
                return Err(err("match distance before the dictionary start"));
            }
            if out.len() + n > end {
                return Err(err("match runs past the chunk's uncompressed size"));
            }
            let from = out.len() - back;
            for k in 0..n {
                let b = out[from + k];
                out.push(b);
            }
        }
        Ok(())
    }
}

fn props(byte: u8) -> Result<(u32, u32, u32)> {
    let mut d = u32::from(byte);
    if d >= 9 * 5 * 5 {
        return Err(err(format!("invalid properties byte {byte:#04x}")));
    }
    let lc = d % 9;
    d /= 9;
    let lp = d % 5;
    let pb = d / 5;
    if lc + lp > 4 {
        return Err(err(format!(
            "lc + lp = {} (LZMA2 allows at most 4)",
            lc + lp
        )));
    }
    Ok((lc, lp, pb))
}

/// Decode a raw LZMA2 stream (a run of chunks ended by a `0x00` control byte; no `.xz`
/// container and no dictionary-size byte). `expected_len` is the decoded size when the caller
/// knows it (0 = unknown, capped at 1 GiB); a stream decoding to another size is an error.
/// Returns the decoded bytes and the number of input bytes consumed (through the end byte).
///
/// # Errors
/// [`CodecError::Decode`] for a malformed stream, one that ends without its end byte, or one
/// that decodes past the limit; [`CodecError::SizeMismatch`] when `expected_len` is given and
/// differs.
pub fn lzma2_decode(data: &[u8], expected_len: usize) -> Result<(Vec<u8>, usize)> {
    let limit = output_limit(expected_len);
    let mut out: Vec<u8> = Vec::with_capacity(expected_len.min(MAX_PREALLOC));
    let mut at = 0usize;
    let mut dict_start = 0usize;
    let mut need_dict_reset = true;
    let mut state: Option<LzmaState> = None;
    loop {
        let control = *data
            .get(at)
            .ok_or_else(|| err("stream ends without its end byte"))?;
        at += 1;
        if control == 0 {
            break;
        }
        let be16 = |i: usize| -> Result<usize> {
            data.get(i..i + 2)
                .map(|b| usize::from(u16::from_be_bytes([b[0], b[1]])))
                .ok_or_else(|| err("chunk header truncated"))
        };
        if control == 1 || control == 2 {
            // uncompressed chunk (1: with a dictionary reset)
            if control == 1 {
                dict_start = out.len();
                need_dict_reset = false;
            } else if need_dict_reset {
                return Err(err("first chunk does not reset the dictionary"));
            }
            let size = be16(at)? + 1;
            at += 2;
            let bytes = data
                .get(at..at + size)
                .ok_or_else(|| err("uncompressed chunk truncated"))?;
            if out.len() + size > limit {
                return Err(err("decoded data exceeds the expected size"));
            }
            out.extend_from_slice(bytes);
            at += size;
            continue;
        }
        if control < 0x80 {
            return Err(err(format!("invalid chunk control byte {control:#04x}")));
        }
        let unpacked = ((usize::from(control & 0x1F)) << 16) + be16(at)? + 1;
        let packed = be16(at + 2)? + 1;
        at += 4;
        let reset = (control >> 5) & 3;
        if reset == 3 {
            dict_start = out.len();
            need_dict_reset = false;
        } else if need_dict_reset {
            return Err(err("first chunk does not reset the dictionary"));
        }
        if reset >= 2 {
            let p = *data
                .get(at)
                .ok_or_else(|| err("chunk properties truncated"))?;
            at += 1;
            let (lc, lp, pb) = props(p)?;
            state = Some(LzmaState::new(lc, lp, pb));
        } else if reset == 1 {
            let s = state
                .as_ref()
                .ok_or_else(|| err("state reset before any properties"))?;
            state = Some(LzmaState::new(s.lc, s.lp, s.pb));
        }
        let st = state
            .as_mut()
            .ok_or_else(|| err("LZMA chunk before any properties"))?;
        let comp = data
            .get(at..at + packed)
            .ok_or_else(|| err("compressed chunk truncated"))?;
        if out.len() + unpacked > limit {
            return Err(err("decoded data exceeds the expected size"));
        }
        let mut rc = RangeDecoder::new(comp)?;
        st.chunk(&mut rc, &mut out, dict_start, unpacked)?;
        if rc.code != 0 || rc.pos != comp.len() {
            return Err(err(
                "range coder did not finish cleanly at the chunk's compressed size",
            ));
        }
        at += packed;
    }
    if expected_len != 0 && out.len() != expected_len {
        return Err(CodecError::SizeMismatch {
            codec: CODEC,
            got: out.len(),
            expected: expected_len,
        });
    }
    Ok((out, at))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn uncompressed_chunks() {
        // 0x01 (dict reset, 3 bytes), 0x02 (2 bytes), end
        let (out, n) =
            lzma2_decode(&[1, 0, 2, b'a', b'b', b'c', 2, 0, 1, b'd', b'e', 0], 0).unwrap();
        assert_eq!(out, b"abcde");
        assert_eq!(n, 12);
    }

    #[test]
    fn rejects_malformed() {
        assert!(lzma2_decode(&[], 0).is_err());
        assert!(lzma2_decode(&[2, 0, 0, b'a', 0], 0).is_err()); // no dict reset
        assert!(lzma2_decode(&[0x80, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0], 0).is_err()); // no props
        assert!(lzma2_decode(&[0x03], 0).is_err());
        assert!(lzma2_decode(&[1, 0, 2, b'a'], 0).is_err()); // truncated
    }

    #[test]
    fn python_lzma2_vector() {
        // lzma.compress(b"hello hello hello hello, lzma2!", format=FORMAT_RAW,
        //               filters=[{"id": FILTER_LZMA2, "preset": 6}])
        let data = unhex(VECTOR);
        let (out, _) = lzma2_decode(&data, 0).unwrap();
        assert_eq!(out, b"hello hello hello hello, lzma2!");
        assert!(matches!(
            lzma2_decode(&data, 5),
            Err(CodecError::Decode { .. } | CodecError::SizeMismatch { .. })
        ));
    }

    const VECTOR: &str = "e0001e00145d00341949ee8de9560ae7799a19124543a9df4c4c4000";
}
