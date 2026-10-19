//! Decoding the records of one slice (CRAM specification § 10): positions, names, mate data,
//! tags, read features, mapping quality, qualities and, with the reference, the bases.

use std::collections::HashMap;

use super::encoding::{CompressionHeader, Encoding, Streams};
use super::io::{BitReader, Cursor, DecodeResult};

/// Longest read accepted (bases).
const MAX_READ: usize = 1 << 26;

/// The slice header (§ 8.5).
#[derive(Debug, Clone)]
pub(crate) struct SliceHeader {
    pub(crate) ref_id: i32,
    pub(crate) start: i64,
    pub(crate) span: i64,
    pub(crate) records: usize,
    pub(crate) block_count: usize,
    pub(crate) embedded_ref: i32,
    pub(crate) md5: [u8; 16],
}

pub(crate) fn slice_header(data: &[u8], major: u8) -> DecodeResult<SliceHeader> {
    let mut r = Cursor::new(data);
    let ref_id = r.itf8()?;
    let start = i64::from(r.itf8()?);
    let span = i64::from(r.itf8()?);
    let records = r.itf8_len(1 << 28, "slice record count")?;
    if major >= 3 {
        r.ltf8()?;
    } else {
        r.itf8()?;
    }
    let block_count = r.itf8_len(1 << 16, "slice block count")?;
    let n = r.itf8_len(1 << 16, "slice block id count")?;
    for _ in 0..n {
        r.itf8()?;
    }
    let embedded_ref = r.itf8()?;
    let mut md5 = [0u8; 16];
    md5.copy_from_slice(r.take(16)?);
    Ok(SliceHeader {
        ref_id,
        start,
        span,
        records,
        block_count,
        embedded_ref,
        md5,
    })
}

/// A read feature (§ 10.6).
#[derive(Debug, Clone)]
pub(crate) enum Feature {
    /// Read bases replacing aligned bases (`b`, or `B` with one base).
    Bases(Vec<u8>),
    /// A substitution code against the reference base (`X`).
    Sub(u8),
    /// Bases inserted or soft-clipped (`I`, `i`, `S`): they take read positions only.
    Inserted(Vec<u8>),
    /// Reference bases skipped (`D`, `N`).
    RefSkip(i64),
    /// No effect on bases (`H`, `P`, `Q`, `q`).
    None,
}

/// One decoded record, before its bases are built.
#[derive(Debug, Clone, Default)]
pub(crate) struct RawRecord {
    pub(crate) flag: u16,
    pub(crate) cf: i32,
    pub(crate) ref_id: i32,
    pub(crate) read_len: usize,
    pub(crate) pos: i64,
    pub(crate) name: Vec<u8>,
    pub(crate) next_frag: Option<usize>,
    pub(crate) mapq: u8,
    pub(crate) qual: Vec<u8>,
    /// Unmapped reads: the bases as stored.
    pub(crate) bases: Option<Vec<u8>>,
    /// Mapped reads: features with their 1-based read positions.
    pub(crate) features: Vec<(usize, Feature)>,
    pub(crate) passes: Option<i64>,
    pub(crate) accuracy: Option<f64>,
}

fn enc(h: &CompressionHeader, k: [u8; 2]) -> DecodeResult<&Encoding> {
    h.series.get(&k).ok_or_else(|| {
        format!(
            "the {} data series has no encoding",
            String::from_utf8_lossy(&k)
        )
    })
}

/// The integer value of a BAM tag value of type `ty`.
fn tag_int(ty: u8, v: &[u8]) -> Option<i64> {
    let b = |n: usize| v.get(..n);
    Some(match ty {
        b'c' => i64::from(*v.first()? as i8),
        b'C' => i64::from(*v.first()?),
        b's' => i64::from(i16::from_le_bytes(b(2)?.try_into().ok()?)),
        b'S' => i64::from(u16::from_le_bytes(b(2)?.try_into().ok()?)),
        b'i' => i64::from(i32::from_le_bytes(b(4)?.try_into().ok()?)),
        b'I' => i64::from(u32::from_le_bytes(b(4)?.try_into().ok()?)),
        _ => return None,
    })
}

/// Decode the records of a slice from its blocks.
pub(crate) fn decode_records(
    h: &CompressionHeader,
    s: &SliceHeader,
    core: &[u8],
    external: &HashMap<i32, Vec<u8>>,
) -> DecodeResult<Vec<RawRecord>> {
    let mut st = Streams {
        core: BitReader::new(core),
        external: external
            .iter()
            .map(|(k, v)| (*k, Cursor::new(v.as_slice())))
            .collect(),
    };
    let mut out: Vec<RawRecord> = Vec::with_capacity(s.records.min(1 << 20));
    let mut last_pos = s.start;
    for i in 0..s.records {
        let mut r = RawRecord::default();
        let bf = st.int(enc(h, *b"BF")?)?;
        r.flag = u16::try_from(bf).map_err(|_| format!("record {i}: BAM flags {bf}"))?;
        r.cf = st.int(enc(h, *b"CF")?)?;
        r.ref_id = if s.ref_id == -2 {
            st.int(enc(h, *b"RI")?)?
        } else {
            s.ref_id
        };
        let rl = st.int(enc(h, *b"RL")?)?;
        r.read_len = usize::try_from(rl)
            .ok()
            .filter(|n| *n <= MAX_READ)
            .ok_or_else(|| format!("record {i}: read length {rl}"))?;
        let ap = i64::from(st.int(enc(h, *b"AP")?)?);
        r.pos = if h.ap_delta {
            last_pos += ap;
            last_pos
        } else {
            ap
        };
        st.int(enc(h, *b"RG")?)?;
        if h.read_names {
            r.name = st.bytes(enc(h, *b"RN")?)?;
        }
        if r.cf & 2 != 0 {
            let mf = st.int(enc(h, *b"MF")?)?;
            if mf & 1 != 0 {
                r.flag |= 0x20;
            }
            if mf & 2 != 0 {
                r.flag |= 0x08;
            }
            if !h.read_names {
                r.name = st.bytes(enc(h, *b"RN")?)?;
            }
            st.int(enc(h, *b"NS")?)?;
            st.int(enc(h, *b"NP")?)?;
            st.int(enc(h, *b"TS")?)?;
        } else if r.cf & 4 != 0 {
            let nf = st.int(enc(h, *b"NF")?)?;
            let nf = usize::try_from(nf).map_err(|_| format!("record {i}: NF {nf}"))?;
            r.next_frag = i.checked_add(nf).and_then(|j| j.checked_add(1));
        }
        let tl = st.int(enc(h, *b"TL")?)?;
        let tags = usize::try_from(tl)
            .ok()
            .and_then(|t| h.tag_dict.get(t))
            .ok_or_else(|| format!("record {i}: tag line {tl} is not in the dictionary"))?;
        for (id, ty, key) in tags {
            let e = h.tags.get(key).ok_or_else(|| {
                format!(
                    "record {i}: tag {}:{} has no encoding",
                    String::from_utf8_lossy(id),
                    *ty as char
                )
            })?;
            let v = st.bytes(e)?;
            match id {
                b"np" => r.passes = tag_int(*ty, &v),
                b"rq" if *ty == b'f' && v.len() >= 4 => {
                    r.accuracy = Some(f64::from(f32::from_le_bytes([v[0], v[1], v[2], v[3]])));
                }
                _ => {}
            }
        }
        if r.flag & 4 == 0 {
            let fn_ = st.int(enc(h, *b"FN")?)?;
            let fn_ = usize::try_from(fn_)
                .ok()
                .filter(|n| *n <= MAX_READ)
                .ok_or_else(|| format!("record {i}: {fn_} read features"))?;
            let mut fpos = 0usize;
            let mut qual_at: Vec<(usize, Vec<u8>)> = Vec::new();
            for _ in 0..fn_ {
                let code = st.byte(enc(h, *b"FC")?)?;
                let d = st.int(enc(h, *b"FP")?)?;
                fpos = usize::try_from(d)
                    .ok()
                    .and_then(|d| fpos.checked_add(d))
                    .ok_or_else(|| format!("record {i}: feature position delta {d}"))?;
                let f = match code {
                    b'B' => {
                        let b = st.byte(enc(h, *b"BA")?)?;
                        let q = st.byte(enc(h, *b"QS")?)?;
                        qual_at.push((fpos, vec![q]));
                        Feature::Bases(vec![b])
                    }
                    b'X' => Feature::Sub(st.byte(enc(h, *b"BS")?)?),
                    b'I' => Feature::Inserted(st.bytes(enc(h, *b"IN")?)?),
                    b'S' => Feature::Inserted(st.bytes(enc(h, *b"SC")?)?),
                    b'i' => Feature::Inserted(vec![st.byte(enc(h, *b"BA")?)?]),
                    b'b' => Feature::Bases(st.bytes(enc(h, *b"BB")?)?),
                    b'q' => {
                        qual_at.push((fpos, st.bytes(enc(h, *b"QQ")?)?));
                        Feature::None
                    }
                    b'Q' => {
                        qual_at.push((fpos, vec![st.byte(enc(h, *b"QS")?)?]));
                        Feature::None
                    }
                    b'D' => Feature::RefSkip(i64::from(st.int(enc(h, *b"DL")?)?)),
                    b'N' => Feature::RefSkip(i64::from(st.int(enc(h, *b"RS")?)?)),
                    b'P' => {
                        st.int(enc(h, *b"PD")?)?;
                        Feature::None
                    }
                    b'H' => {
                        st.int(enc(h, *b"HC")?)?;
                        Feature::None
                    }
                    other => {
                        return Err(format!(
                            "record {i}: read feature code {:?} is not defined",
                            other as char
                        ));
                    }
                };
                r.features.push((fpos, f));
            }
            let mq = st.int(enc(h, *b"MQ")?)?;
            r.mapq = u8::try_from(mq).map_err(|_| format!("record {i}: mapping quality {mq}"))?;
            if r.cf & 1 != 0 {
                r.qual = st.byte_run(enc(h, *b"QS")?, r.read_len)?;
            } else if !qual_at.is_empty() {
                r.qual = vec![0xff; r.read_len];
                for (p, q) in qual_at {
                    for (k, v) in q.into_iter().enumerate() {
                        if let Some(slot) = p.checked_sub(1).and_then(|p| r.qual.get_mut(p + k)) {
                            *slot = v;
                        }
                    }
                }
            }
        } else {
            r.bases = Some(st.byte_run(enc(h, *b"BA")?, r.read_len)?);
            if r.cf & 1 != 0 {
                r.qual = st.byte_run(enc(h, *b"QS")?, r.read_len)?;
            }
        }
        out.push(r);
    }
    resolve_mates(&mut out);
    Ok(out)
}

/// Mate flags of records whose mate is later in the same slice (CF bit 4): each record takes
/// 0x20 and 0x08 from its next fragment, and the last fragment of a chain from the first.
fn resolve_mates(recs: &mut [RawRecord]) {
    let n = recs.len();
    let mut is_target = vec![false; n];
    for r in recs.iter() {
        if let Some(j) = r.next_frag.filter(|j| *j < n) {
            is_target[j] = true;
        }
    }
    let mate_bits = |f: u16| -> u16 {
        let mut b = 0;
        if f & 0x10 != 0 {
            b |= 0x20;
        }
        if f & 0x04 != 0 {
            b |= 0x08;
        }
        b
    };
    for i in 0..n {
        if let Some(j) = recs[i].next_frag.filter(|j| *j < n) {
            let b = mate_bits(recs[j].flag);
            recs[i].flag |= b;
        }
    }
    for head in 0..n {
        if is_target[head] || recs[head].next_frag.is_none_or(|j| j >= n) {
            continue;
        }
        let mut k = head;
        let mut steps = 0;
        while let Some(j) = recs[k].next_frag.filter(|j| *j < n && *j > k) {
            k = j;
            steps += 1;
            if steps > n {
                break;
            }
        }
        if k != head {
            let b = mate_bits(recs[head].flag);
            recs[k].flag |= b;
        }
    }
}

/// The bases of a mapped read from its features and the reference: `refb(p)` gives the
/// reference base at 1-based position `p` (`N` past the end of the sequence), or `None`
/// without a reference. Returns the bases and whether every base is known.
pub(crate) fn build_bases(
    r: &RawRecord,
    sub: &[[u8; 4]; 5],
    refb: &mut dyn FnMut(i64) -> Option<u8>,
) -> (Vec<u8>, bool) {
    fn fill(
        seq: &mut Vec<u8>,
        upto: usize,
        rpos: &mut i64,
        known: &mut bool,
        refb: &mut dyn FnMut(i64) -> Option<u8>,
    ) {
        while seq.len() < upto {
            if let Some(b) = refb(*rpos) {
                seq.push(b);
            } else {
                *known = false;
                seq.push(b'N');
            }
            *rpos += 1;
        }
    }
    let mut seq = Vec::with_capacity(r.read_len.min(1 << 20));
    let mut known = true;
    let mut rpos = r.pos;
    for (fpos, f) in &r.features {
        let upto = fpos.saturating_sub(1).min(r.read_len);
        fill(&mut seq, upto, &mut rpos, &mut known, refb);
        match f {
            Feature::Bases(b) => {
                for &x in b {
                    if seq.len() < r.read_len {
                        seq.push(x.to_ascii_uppercase());
                    }
                }
                rpos += b.len() as i64;
            }
            Feature::Sub(code) => {
                let base = if let Some(rb) = refb(rpos) {
                    let row = match rb {
                        b'A' => 0,
                        b'C' => 1,
                        b'G' => 2,
                        b'T' => 3,
                        _ => 4,
                    };
                    sub[row][(*code & 3) as usize]
                } else {
                    known = false;
                    b'N'
                };
                if seq.len() < r.read_len {
                    seq.push(base);
                }
                rpos += 1;
            }
            Feature::Inserted(b) => {
                for &x in b {
                    if seq.len() < r.read_len {
                        seq.push(x.to_ascii_uppercase());
                    }
                }
            }
            Feature::RefSkip(n) => rpos += (*n).max(0),
            Feature::None => {}
        }
    }
    fill(&mut seq, r.read_len, &mut rpos, &mut known, refb);
    (seq, known)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bases_from_features_and_reference() {
        let reference = b"ACGTACGTAC";
        let mut refb = |p: i64| {
            Some(
                usize::try_from(p - 1)
                    .ok()
                    .and_then(|i| reference.get(i))
                    .copied()
                    .unwrap_or(b'N'),
            )
        };
        let sub = [*b"CGTN", *b"AGTN", *b"ACTN", *b"ACGN", *b"ACGT"];
        let r = RawRecord {
            read_len: 8,
            pos: 2,
            features: vec![
                (2, Feature::Sub(0)),
                (4, Feature::Inserted(b"tt".to_vec())),
                (6, Feature::RefSkip(2)),
            ],
            ..RawRecord::default()
        };
        // ref from 2: C G T A C G T A ...; read: C, sub(G→A), T, ins TT, (skip 2) G T A
        let (seq, known) = build_bases(&r, &sub, &mut refb);
        assert!(known);
        assert_eq!(seq, b"CATTTGTA");
        let (_, known) = build_bases(&r, &sub, &mut |_| None);
        assert!(!known);
    }
}
