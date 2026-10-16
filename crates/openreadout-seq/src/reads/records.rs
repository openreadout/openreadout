//! Streaming record parsers: FASTQ (plain or gzip, four-line or multi-line records) and BAM
//! (BGZF-compressed binary alignments, SAMv1 § 4).

use std::io::{BufRead, BufReader, Read};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};

/// Longest FASTQ line or BAM record accepted (bytes).
pub const MAX_RECORD: usize = 256 << 20;

/// One read.
#[derive(Debug, Clone, Default)]
pub struct Rec {
    /// Bases as letters.
    pub seq: Vec<u8>,
    /// Qualities as Phred values (empty when the record has none).
    pub qual: Vec<u8>,
    /// Name (FASTQ title up to the first space, BAM read name).
    pub name: Vec<u8>,
    /// BAM flag.
    pub flag: Option<u16>,
    /// BAM mapping quality.
    pub mapq: Option<u8>,
    /// BAM reference index (-1 unmapped).
    pub ref_id: Option<i32>,
    /// BAM 1-based position.
    pub pos: Option<i64>,
    /// PacBio passes (`np`).
    pub passes: Option<i64>,
    /// PacBio read accuracy (`rq`).
    pub accuracy: Option<f64>,
    /// Smallest raw quality character (FASTQ).
    pub min_qchar: u8,
}

impl Rec {
    /// A secondary or supplementary alignment (not a read of its own).
    pub fn is_extra_alignment(&self) -> bool {
        self.flag.is_some_and(|f| f & 0x900 != 0)
    }
}

/// Counts the bytes read from the file (compressed bytes for gzip and BGZF).
#[derive(Debug)]
pub struct Counting<R> {
    inner: R,
    n: Arc<AtomicU64>,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.n.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

/// A source of records.
pub trait Records
where
    Self: Send,
{
    /// Read the next record into `rec`; false at the end.
    fn next_rec(&mut self, rec: &mut Rec) -> Result<bool>;
    /// Bytes of the file consumed so far.
    fn consumed(&self) -> u64;
}

/// Open `file` as a byte stream, decompressing gzip/BGZF; returns the stream and the counter
/// of file bytes consumed.
pub fn open_stream(file: SourceFile, gz: bool) -> (Box<dyn BufRead + Send>, Arc<AtomicU64>) {
    let n = Arc::new(AtomicU64::new(0));
    let c = Counting {
        inner: file,
        n: n.clone(),
    };
    let r: Box<dyn BufRead + Send> = if gz {
        Box::new(BufReader::with_capacity(
            1 << 20,
            flate2::read::MultiGzDecoder::new(BufReader::with_capacity(1 << 18, c)),
        ))
    } else {
        Box::new(BufReader::with_capacity(1 << 20, c))
    };
    (r, n)
}

/// FASTQ records.
pub struct FastqRecords {
    r: Box<dyn BufRead + Send>,
    n: Arc<AtomicU64>,
    line: Vec<u8>,
    pending: Option<Vec<u8>>,
    format: &'static str,
    record: u64,
}

impl std::fmt::Debug for FastqRecords {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FastqRecords")
            .field("record", &self.record)
            .finish_non_exhaustive()
    }
}

impl FastqRecords {
    /// Records of the stream `r`.
    pub fn new(r: Box<dyn BufRead + Send>, n: Arc<AtomicU64>, format: &'static str) -> Self {
        FastqRecords {
            r,
            n,
            line: Vec::new(),
            pending: None,
            format,
            record: 0,
        }
    }

    /// The next line without its line ending; `None` at the end.
    fn line(&mut self) -> Result<Option<Vec<u8>>> {
        if let Some(p) = self.pending.take() {
            return Ok(Some(p));
        }
        self.line.clear();
        let n = (&mut self.r)
            .take(MAX_RECORD as u64 + 1)
            .read_until(b'\n', &mut self.line)
            .map_err(|e| Error::corrupt(self.format, format!("record {}: {e}", self.record)))?;
        if n == 0 {
            return Ok(None);
        }
        if self.line.len() > MAX_RECORD {
            return Err(Error::corrupt(
                self.format,
                format!(
                    "record {}: a line longer than {MAX_RECORD} bytes",
                    self.record
                ),
            ));
        }
        while matches!(self.line.last(), Some(b'\n' | b'\r')) {
            self.line.pop();
        }
        Ok(Some(std::mem::take(&mut self.line)))
    }
}

impl Records for FastqRecords {
    fn next_rec(&mut self, rec: &mut Rec) -> Result<bool> {
        // skip blank lines between records
        let title = loop {
            match self.line()? {
                None => return Ok(false),
                Some(l) if l.is_empty() => {}
                Some(l) => break l,
            }
        };
        let format = self.format;
        let bad = move |what: String| Error::corrupt(format, what);
        if title.first() != Some(&b'@') {
            return Err(bad(format!(
                "record {}: a title line must start with '@' (found {:?})",
                self.record,
                String::from_utf8_lossy(&title[..title.len().min(40)])
            )));
        }
        rec.name.clear();
        rec.name.extend(
            title[1..]
                .iter()
                .take_while(|c| !c.is_ascii_whitespace())
                .copied(),
        );
        rec.seq.clear();
        loop {
            let l = self.line()?.ok_or_else(|| {
                bad(format!(
                    "record {}: ends before its '+' line (truncated)",
                    self.record
                ))
            })?;
            if l.first() == Some(&b'+') {
                break;
            }
            rec.seq.extend_from_slice(&l);
            if rec.seq.len() > MAX_RECORD {
                return Err(bad(format!("record {}: sequence too long", self.record)));
            }
        }
        rec.qual.clear();
        let mut min = u8::MAX;
        while rec.qual.len() < rec.seq.len() {
            let l = self.line()?.ok_or_else(|| {
                bad(format!(
                    "record {}: quality ends early (truncated)",
                    self.record
                ))
            })?;
            for &c in &l {
                if !(33..=126).contains(&c) {
                    return Err(bad(format!(
                        "record {}: quality character {c} outside '!'..'~'",
                        self.record
                    )));
                }
                min = min.min(c);
            }
            rec.qual.extend_from_slice(&l);
        }
        if rec.qual.len() != rec.seq.len() {
            return Err(bad(format!(
                "record {}: {} qualities for {} bases",
                self.record,
                rec.qual.len(),
                rec.seq.len()
            )));
        }
        rec.min_qchar = min;
        for q in &mut rec.qual {
            *q -= 33;
        }
        self.record += 1;
        Ok(true)
    }

    fn consumed(&self) -> u64 {
        self.n.load(Ordering::Relaxed)
    }
}

/// A BAM header.
#[derive(Debug, Clone, Default)]
pub struct BamHeader {
    /// The SAM header text.
    pub text: String,
    /// Reference sequences: name and length.
    pub refs: Vec<(String, u64)>,
}

/// BAM records.
pub struct BamRecords {
    r: Box<dyn BufRead + Send>,
    n: Arc<AtomicU64>,
    buf: Vec<u8>,
    record: u64,
    /// The header.
    pub header: BamHeader,
}

impl std::fmt::Debug for BamRecords {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BamRecords")
            .field("record", &self.record)
            .finish_non_exhaustive()
    }
}

const FORMAT_BAM: &str = "bam";

fn read_exact_or(r: &mut dyn BufRead, buf: &mut [u8], what: &str) -> Result<()> {
    r.read_exact(buf).map_err(|e| {
        Error::corrupt(
            FORMAT_BAM,
            format!("{what}: {e} (the file is truncated or not BAM)"),
        )
    })
}

fn le_i32(b: &[u8]) -> i32 {
    i32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

impl BamRecords {
    /// Read the header of the decompressed stream `r`.
    pub fn new(mut r: Box<dyn BufRead + Send>, n: Arc<AtomicU64>) -> Result<Self> {
        let mut magic = [0u8; 8];
        read_exact_or(r.as_mut(), &mut magic, "header")?;
        if &magic[..4] != b"BAM\x01" {
            return Err(Error::corrupt(
                FORMAT_BAM,
                "no BAM magic (BAM\\1) after decompression",
            ));
        }
        let l_text = le_i32(&magic[4..8]);
        if !(0..=MAX_RECORD as i32).contains(&l_text) {
            return Err(Error::corrupt(
                FORMAT_BAM,
                format!("header text of {l_text} bytes"),
            ));
        }
        let mut text = vec![0u8; l_text as usize];
        read_exact_or(r.as_mut(), &mut text, "header text")?;
        let mut four = [0u8; 4];
        read_exact_or(r.as_mut(), &mut four, "reference count")?;
        let n_ref = le_i32(&four);
        if !(0..=50_000_000).contains(&n_ref) {
            return Err(Error::corrupt(
                FORMAT_BAM,
                format!("{n_ref} reference sequences"),
            ));
        }
        let mut refs = Vec::with_capacity((n_ref as usize).min(1 << 16));
        for _ in 0..n_ref {
            read_exact_or(r.as_mut(), &mut four, "reference name length")?;
            let l = le_i32(&four);
            if !(1..=1 << 16).contains(&l) {
                return Err(Error::corrupt(
                    FORMAT_BAM,
                    format!("reference name of {l} bytes"),
                ));
            }
            let mut name = vec![0u8; l as usize];
            read_exact_or(r.as_mut(), &mut name, "reference name")?;
            read_exact_or(r.as_mut(), &mut four, "reference length")?;
            let len = le_i32(&four).max(0) as u64;
            while name.last() == Some(&0) {
                name.pop();
            }
            refs.push((String::from_utf8_lossy(&name).into_owned(), len));
        }
        let text = String::from_utf8_lossy(&text)
            .trim_end_matches('\0')
            .to_string();
        Ok(BamRecords {
            r,
            n,
            buf: Vec::new(),
            record: 0,
            header: BamHeader { text, refs },
        })
    }
}

const SEQ_CODES: &[u8; 16] = b"=ACMGRSVTWYHKDBN";

/// Parse the auxiliary fields of a record for the tags kept.
fn aux(mut b: &[u8], rec: &mut Rec) {
    while b.len() >= 3 {
        let tag = [b[0], b[1]];
        let ty = b[2];
        b = &b[3..];
        let size = match ty {
            b'A' | b'c' | b'C' => 1,
            b's' | b'S' => 2,
            b'i' | b'I' | b'f' => 4,
            b'Z' | b'H' => match b.iter().position(|c| *c == 0) {
                Some(p) => p + 1,
                None => return,
            },
            b'B' => {
                if b.len() < 5 {
                    return;
                }
                let w = match b[0] {
                    b'c' | b'C' => 1,
                    b's' | b'S' => 2,
                    b'i' | b'I' | b'f' => 4,
                    _ => return,
                };
                let n = u32::from_le_bytes([b[1], b[2], b[3], b[4]]) as usize;
                match n.checked_mul(w).and_then(|x| x.checked_add(5)) {
                    Some(s) => s,
                    None => return,
                }
            }
            _ => return,
        };
        let Some(v) = b.get(..size) else { return };
        let int = || -> Option<i64> {
            Some(match ty {
                b'c' => i64::from(v[0] as i8),
                b'C' => i64::from(v[0]),
                b's' => i64::from(i16::from_le_bytes([v[0], v[1]])),
                b'S' => i64::from(u16::from_le_bytes([v[0], v[1]])),
                b'i' => i64::from(i32::from_le_bytes([v[0], v[1], v[2], v[3]])),
                b'I' => i64::from(u32::from_le_bytes([v[0], v[1], v[2], v[3]])),
                _ => return None,
            })
        };
        match &tag {
            b"np" => rec.passes = int(),
            b"rq" if ty == b'f' => {
                rec.accuracy = Some(f64::from(f32::from_le_bytes([v[0], v[1], v[2], v[3]])));
            }
            _ => {}
        }
        b = &b[size..];
    }
}

impl Records for BamRecords {
    fn next_rec(&mut self, rec: &mut Rec) -> Result<bool> {
        let mut four = [0u8; 4];
        // end of stream: no more bytes at a record boundary
        let got = self
            .r
            .fill_buf()
            .map_err(|e| Error::corrupt(FORMAT_BAM, format!("record {}: {e}", self.record)))?;
        if got.is_empty() {
            return Ok(false);
        }
        read_exact_or(self.r.as_mut(), &mut four, "record size")?;
        let size = le_i32(&four);
        if !(32..=MAX_RECORD as i32).contains(&size) {
            return Err(Error::corrupt(
                FORMAT_BAM,
                format!("record {}: size {size}", self.record),
            ));
        }
        self.buf.resize(size as usize, 0);
        read_exact_or(self.r.as_mut(), &mut self.buf, "record")?;
        let b = &self.buf;
        let ref_id = le_i32(&b[0..4]);
        let pos = le_i32(&b[4..8]);
        let l_name = b[8] as usize;
        let mapq = b[9];
        let n_cigar = u16::from_le_bytes([b[12], b[13]]) as usize;
        let flag = u16::from_le_bytes([b[14], b[15]]);
        let l_seq = le_i32(&b[16..20]);
        if l_seq < 0 {
            return Err(Error::corrupt(
                FORMAT_BAM,
                format!("record {}: negative length", self.record),
            ));
        }
        let l_seq = l_seq as usize;
        let name_end = 32 + l_name;
        let cigar_end = name_end + 4 * n_cigar;
        let seq_end = cigar_end + l_seq.div_ceil(2);
        let qual_end = seq_end + l_seq;
        if qual_end > b.len() {
            return Err(Error::corrupt(
                FORMAT_BAM,
                format!("record {}: fields run past the record", self.record),
            ));
        }
        rec.name.clear();
        rec.name
            .extend(b[32..name_end].iter().take_while(|c| **c != 0).copied());
        rec.seq.clear();
        for i in 0..l_seq {
            let byte = b[cigar_end + i / 2];
            let code = if i % 2 == 0 { byte >> 4 } else { byte & 15 };
            rec.seq.push(SEQ_CODES[code as usize]);
        }
        rec.qual.clear();
        let q = &b[seq_end..qual_end];
        if q.first() != Some(&0xFF) {
            rec.qual.extend_from_slice(q);
        }
        rec.flag = Some(flag);
        rec.mapq = Some(mapq);
        rec.ref_id = Some(ref_id);
        rec.pos = (pos >= 0).then(|| i64::from(pos) + 1);
        rec.passes = None;
        rec.accuracy = None;
        rec.min_qchar = 33;
        aux(&b[qual_end..], rec);
        self.record += 1;
        Ok(true)
    }

    fn consumed(&self) -> u64 {
        self.n.load(Ordering::Relaxed)
    }
}

/// Mapped and unmapped read counts of an indexed BAM, from its `.bai` (the index's per-reference
/// pseudo-bin 37450 and the count of reads without coordinates).
pub fn bai_counts(b: &[u8]) -> Option<(u64, u64)> {
    let mut at = 0usize;
    let u32_at = |at: usize| {
        b.get(at..at + 4)
            .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    };
    let u64_at = |at: usize| {
        b.get(at..at + 8)
            .map(|s| u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
    };
    if b.get(..4)? != b"BAI\x01" {
        return None;
    }
    at += 4;
    let n_ref = u32_at(at)? as usize;
    at += 4;
    let (mut mapped, mut unmapped) = (0u64, 0u64);
    let mut have = false;
    for _ in 0..n_ref {
        let n_bin = u32_at(at)? as usize;
        at += 4;
        for _ in 0..n_bin {
            let bin = u32_at(at)?;
            let n_chunk = u32_at(at + 4)? as usize;
            at += 8;
            if bin == 37450 && n_chunk == 2 {
                mapped += u64_at(at + 16)?;
                unmapped += u64_at(at + 24)?;
                have = true;
            }
            at = at.checked_add(n_chunk.checked_mul(16)?)?;
        }
        let n_intv = u32_at(at)? as usize;
        at = at.checked_add(4 + n_intv.checked_mul(8)?)?;
    }
    if let Some(no_coor) = u64_at(at) {
        unmapped += no_coor;
    }
    have.then_some((mapped, unmapped))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fq(text: &str) -> FastqRecords {
        let r: Box<dyn BufRead + Send> = Box::new(std::io::Cursor::new(text.as_bytes().to_vec()));
        FastqRecords::new(r, Arc::new(AtomicU64::new(0)), "fastq")
    }

    #[test]
    fn fastq_records_and_errors() {
        let mut r = fq("@a x\nACGT\n+\nIIII\n\n@b\nAC\nGT\n+b\nII\nII\n");
        let mut rec = Rec::default();
        assert!(r.next_rec(&mut rec).unwrap());
        assert_eq!(rec.name, b"a");
        assert_eq!(rec.qual, vec![40; 4]);
        assert!(r.next_rec(&mut rec).unwrap());
        assert_eq!(rec.seq, b"ACGT");
        assert!(!r.next_rec(&mut rec).unwrap());
        assert!(fq("ACGT\n").next_rec(&mut rec).is_err());
        assert!(fq("@a\nACGT\n+\nII\n").next_rec(&mut rec).is_err());
        assert!(fq("@a\nACGT\n").next_rec(&mut rec).is_err());
    }

    #[test]
    fn bai_garbage() {
        assert_eq!(bai_counts(b"BAI\x01"), None);
        assert_eq!(bai_counts(b"XXXX"), None);
        let mut b = b"BAI\x01".to_vec();
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&37450u32.to_le_bytes());
        b.extend_from_slice(&2u32.to_le_bytes());
        for v in [0u64, 0, 7, 3] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&5u64.to_le_bytes());
        assert_eq!(bai_counts(&b), Some((7, 8)));
    }
}
