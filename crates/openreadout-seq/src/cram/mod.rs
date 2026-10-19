//! CRAM (`.cram`): reference-compressed sequence alignments, versions 2.0, 2.1, 3.0 and 3.1.
//!
//! Written from the GA4GH / samtools CRAM specifications (`CRAMv3.tex`, `CRAMv2.1.tex`) and the
//! CRAM codecs document (`CRAMcodecs.tex`) in hts-specs. A file is a 26-byte definition, a header
//! container holding the SAM header text, data containers, and (from 2.1 on) an empty
//! end-of-file container. A data container holds a compression header and slices; a slice holds
//! a core bit stream and external byte streams, each block compressed on its own.
//!
//! [`CramRecords`] streams the records as the read-file reader's [`Rec`] values. Everything but
//! the bases is decoded without a reference. Bases of aligned reads are stored as differences to
//! the reference: they come from a reference embedded in the slice, or from a FASTA file beside
//! the CRAM file (see [`find_reference`]); without either, [`Rec::bases_unknown`] is set and the
//! bases are left out of the statistics. Notes: `docs/formats/cram.md`.

pub(crate) mod arith;
pub(crate) mod block;
pub(crate) mod encoding;
pub(crate) mod fqzcomp;
pub(crate) mod io;
pub(crate) mod rans;
pub mod reference;
pub(crate) mod slice;
pub(crate) mod tok3;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use openreadout_core::source::{Input, SourceFile};
use openreadout_core::{Error, Result};

use crate::reads::records::{BamHeader, Rec, Records};
use block::{COMPRESSION_HEADER, CORE_DATA, EXTERNAL_DATA, FILE_HEADER, SLICE_HEADER};
use io::{Cursor, FMT};
pub use reference::Fasta;

/// The CRAM magic.
pub const CRAM_MAGIC: &[u8; 4] = b"CRAM";

/// Largest container accepted (bytes).
const MAX_CONTAINER: u64 = 1 << 31;

fn bad(detail: impl Into<String>) -> Error {
    Error::corrupt(FMT, detail)
}

/// What decoding found, shared between the record stream and the dataset.
#[derive(Debug, Clone, Default)]
pub struct CramFacts {
    /// Data containers read.
    pub containers: u64,
    /// Slices read.
    pub slices: u64,
    /// Blocks per compression method (`gzip`, `rans4x8`, ...), counting data blocks only.
    pub methods: BTreeMap<String, u64>,
    /// Slices whose reference is embedded in the file.
    pub embedded_reference_slices: u64,
    /// Slices with aligned reads whose bases need the external reference.
    pub external_reference_slices: u64,
    /// Containers that say the reference is not required to restore the data.
    pub reference_not_required: u64,
    /// Records whose bases could not be restored (no reference, or a reference that does not
    /// match the slice checksum).
    pub reads_without_bases: u64,
    /// Slices whose external reference failed the slice MD5 check.
    pub reference_mismatches: u64,
    /// Blocks and container headers whose CRC32 did not match.
    pub crc_failures: u64,
    /// The first problem with the reference, for notes and `check`.
    pub reference_problem: Option<String>,
}

/// The file definition and SAM header of a CRAM file.
#[derive(Debug, Clone)]
pub struct CramHeader {
    /// Major format version.
    pub major: u8,
    /// Minor format version.
    pub minor: u8,
    /// The SAM header and the references it lists.
    pub sam: BamHeader,
    /// Byte offset of the first data container.
    pub data_start: u64,
}

/// One container header.
#[derive(Debug, Clone)]
struct ContainerHeader {
    length: u64,
    records: i64,
    header_len: u64,
    crc_ok: Option<bool>,
}

fn container_header(
    f: &SourceFile,
    at: u64,
    size: u64,
    major: u8,
) -> Result<Option<ContainerHeader>> {
    if at >= size {
        return Ok(None);
    }
    let n = (size - at).min(1 << 20) as usize;
    let mut buf = vec![0u8; n];
    f.read_exact_at(at, &mut buf)
        .map_err(|e| bad(format!("container at byte {at}: {e}")))?;
    let parse = || -> io::DecodeResult<ContainerHeader> {
        let mut r = Cursor::new(&buf);
        let length = r.i32_le()?;
        let length = u64::try_from(length).map_err(|_| format!("container length {length}"))?;
        r.itf8()?; // reference id
        r.itf8()?; // start
        r.itf8()?; // span
        let records = i64::from(r.itf8()?);
        if major >= 3 {
            r.ltf8()?;
        } else {
            r.itf8()?;
        }
        r.ltf8()?; // bases
        r.itf8()?; // blocks
        let nl = r.itf8_len(1 << 20, "landmark count")?;
        for _ in 0..nl {
            r.itf8()?;
        }
        let crc_ok = if major >= 3 {
            let used = r.pos();
            let want = r.u32_le()?;
            Some(crc32fast::hash(&buf[..used]) == want)
        } else {
            None
        };
        Ok(ContainerHeader {
            length,
            records,
            header_len: r.pos() as u64,
            crc_ok,
        })
    };
    parse()
        .map(Some)
        .map_err(|e| Error::corrupt_at(FMT, at, format!("container header: {e}")))
}

/// Read the file definition and the SAM header.
pub fn read_header(f: &SourceFile) -> Result<CramHeader> {
    let size = f.size().map_err(|e| bad(e.to_string()))?;
    let mut def = [0u8; 26];
    f.read_exact_at(0, &mut def)
        .map_err(|_| bad("shorter than the 26-byte CRAM file definition"))?;
    if &def[..4] != CRAM_MAGIC {
        return Err(bad("no CRAM magic"));
    }
    let (major, minor) = (def[4], def[5]);
    if !(2..=3).contains(&major) {
        return Err(Error::unsupported(
            FMT,
            format!("CRAM version {major}.{minor}"),
            "Versions 2.0 to 3.1 are read; convert the file with `samtools view -C` to a current version.",
        ));
    }
    let ch = container_header(f, 26, size, major)?.ok_or_else(|| bad("no header container"))?;
    let body_at = 26 + ch.header_len;
    if ch.length > MAX_CONTAINER || body_at + ch.length > size {
        return Err(bad("the header container runs past the end of the file"));
    }
    let mut body = vec![0u8; ch.length as usize];
    f.read_exact_at(body_at, &mut body)
        .map_err(|e| bad(format!("header container: {e}")))?;
    let mut r = Cursor::new(&body);
    let (b, _) =
        block::read_block(&mut r, major >= 3).map_err(|e| bad(format!("SAM header block: {e}")))?;
    if b.content_type != FILE_HEADER {
        return Err(bad(format!(
            "the first block of the header container has content type {}",
            b.content_type
        )));
    }
    let mut hr = Cursor::new(&b.data);
    let l = hr.i32_le().map_err(bad)?;
    let l = usize::try_from(l).map_err(|_| bad(format!("SAM header text of {l} bytes")))?;
    let text = hr.take(l.min(hr.remaining())).map_err(bad)?;
    let text = String::from_utf8_lossy(text)
        .trim_end_matches('\0')
        .to_string();
    let mut refs = Vec::new();
    for line in text.lines().filter(|l| l.starts_with("@SQ")) {
        let mut name = None;
        let mut len = 0u64;
        for f in line.split('\t').skip(1) {
            if let Some(v) = f.strip_prefix("SN:") {
                name = Some(v.to_string());
            } else if let Some(v) = f.strip_prefix("LN:") {
                len = v.trim().parse().unwrap_or(0);
            }
        }
        refs.push((name.unwrap_or_default(), len));
    }
    Ok(CramHeader {
        major,
        minor,
        sam: BamHeader { text, refs },
        data_start: body_at + ch.length,
    })
}

/// The reference FASTA for a CRAM file: beside it with the same stem (`.fa`, `.fasta`, `.fna`,
/// also after the full name: `x.cram.fa`), or a local file named by an `@SQ UR:` field.
pub fn find_reference(input: &Input, header: &BamHeader) -> Option<PathBuf> {
    let fs = input.fs();
    let p = input.path();
    let mut cands = Vec::new();
    for ext in ["fa", "fasta", "fna"] {
        cands.push(p.with_extension(ext));
        cands.push(PathBuf::from(format!("{}.{ext}", p.display())));
    }
    for line in header.text.lines().filter(|l| l.starts_with("@SQ")) {
        if let Some(u) = line.split('\t').find_map(|f| f.strip_prefix("UR:")) {
            let u = u.strip_prefix("file://").unwrap_or(u);
            if !u.contains("://") && !u.to_ascii_lowercase().ends_with(".gz") {
                let q = Path::new(u);
                cands.push(if q.is_absolute() {
                    q.to_path_buf()
                } else {
                    p.parent().unwrap_or(Path::new("")).join(q)
                });
            }
        }
    }
    cands.into_iter().find(|c| fs.is_file(c))
}

/// Records of a CRAM file, slice by slice.
pub struct CramRecords {
    file: SourceFile,
    size: u64,
    major: u8,
    at: u64,
    refs: Vec<(String, u64)>,
    fasta: Option<Arc<Fasta>>,
    contig: Option<(i32, Arc<Vec<u8>>)>,
    pending: VecDeque<Rec>,
    facts: Arc<Mutex<CramFacts>>,
    done: bool,
}

impl std::fmt::Debug for CramRecords {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CramRecords")
            .field("at", &self.at)
            .finish_non_exhaustive()
    }
}

impl CramRecords {
    /// Records of `file`, whose header is `header`, with the reference `fasta` when there is one.
    pub fn new(
        file: SourceFile,
        header: &CramHeader,
        fasta: Option<Arc<Fasta>>,
        facts: Arc<Mutex<CramFacts>>,
    ) -> Result<Self> {
        let size = file.size().map_err(|e| bad(e.to_string()))?;
        Ok(CramRecords {
            file,
            size,
            major: header.major,
            at: header.data_start,
            refs: header.sam.refs.clone(),
            fasta,
            contig: None,
            pending: VecDeque::new(),
            facts,
            done: false,
        })
    }

    fn fact(&self, f: impl FnOnce(&mut CramFacts)) {
        if let Ok(mut g) = self.facts.lock() {
            f(&mut g);
        }
    }

    /// The whole sequence `ref_id` from the FASTA, kept for the next slice (multi-reference
    /// slices and slices of one long contig).
    fn contig(&mut self, ref_id: i32) -> Option<Arc<Vec<u8>>> {
        if let Some((id, c)) = &self.contig
            && *id == ref_id
        {
            return Some(c.clone());
        }
        let fasta = self.fasta.as_ref()?;
        let name = self.refs.get(usize::try_from(ref_id).ok()?)?.0.clone();
        let len = fasta.len_of(&name)?;
        match fasta.fetch(&name, 0, len) {
            Ok(b) => {
                let c = Arc::new(b);
                self.contig = Some((ref_id, c.clone()));
                Some(c)
            }
            Err(e) => {
                self.fact(|f| {
                    f.reference_problem.get_or_insert(e);
                });
                None
            }
        }
    }

    /// Read the next data container into `pending`; false at the end.
    fn next_container(&mut self) -> Result<bool> {
        loop {
            let Some(ch) = container_header(&self.file, self.at, self.size, self.major)? else {
                return Ok(false);
            };
            let body_at = self.at + ch.header_len;
            if ch.length > MAX_CONTAINER || body_at + ch.length > self.size {
                return Err(Error::corrupt_at(
                    FMT,
                    self.at,
                    "the container runs past the end of the file (truncated)",
                ));
            }
            let start = self.at;
            self.at = body_at + ch.length;
            if ch.crc_ok == Some(false) {
                self.fact(|f| f.crc_failures += 1);
            }
            if ch.records <= 0 {
                // the end-of-file container, or an empty one
                if self.at >= self.size {
                    return Ok(false);
                }
                continue;
            }
            let mut body = vec![0u8; ch.length as usize];
            self.file
                .read_exact_at(body_at, &mut body)
                .map_err(|e| Error::corrupt_at(FMT, start, e.to_string()))?;
            self.decode_container(&body)
                .map_err(|e| Error::corrupt_at(FMT, start, e))?;
            return Ok(true);
        }
    }

    fn decode_container(&mut self, body: &[u8]) -> io::DecodeResult<()> {
        let crc = self.major >= 3;
        let mut r = Cursor::new(body);
        let (ch, ok) = block::read_block(&mut r, crc)?;
        if ch.content_type != COMPRESSION_HEADER {
            return Err(format!(
                "the container starts with a block of content type {}, not the compression header",
                ch.content_type
            ));
        }
        let mut crc_bad = u64::from(ok == Some(false));
        let h = encoding::compression_header(&ch.data)?;
        let mut methods: BTreeMap<String, u64> = BTreeMap::new();
        let mut slices = 0u64;
        while !r.is_empty() {
            let (sb, ok) = block::read_block(&mut r, crc)?;
            crc_bad += u64::from(ok == Some(false));
            if sb.content_type != SLICE_HEADER {
                // padding or an unknown block between slices
                continue;
            }
            let s = slice::slice_header(&sb.data, self.major)?;
            let mut core = Vec::new();
            let mut ext: HashMap<i32, Vec<u8>> = HashMap::new();
            let mut dup_ids: Vec<(i32, Vec<u8>)> = Vec::new();
            for _ in 0..s.block_count {
                let (b, ok) = block::read_block(&mut r, crc)?;
                crc_bad += u64::from(ok == Some(false));
                *methods
                    .entry(block::method_name(b.method).to_string())
                    .or_default() += 1;
                match b.content_type {
                    CORE_DATA => core = b.data,
                    EXTERNAL_DATA => {
                        // two blocks with one content id: the first serves the data series,
                        // either may be the embedded reference
                        if let std::collections::hash_map::Entry::Vacant(v) =
                            ext.entry(b.content_id)
                        {
                            v.insert(b.data);
                        } else {
                            dup_ids.push((b.content_id, b.data));
                        }
                    }
                    _ => {}
                }
            }
            slices += 1;
            let recs = slice::decode_records(&h, &s, &core, &ext)?;
            self.restore(&h, &s, &ext, &dup_ids, recs);
        }
        self.fact(|f| {
            f.containers += 1;
            f.slices += slices;
            f.crc_failures += crc_bad;
            if !h.reference_required {
                f.reference_not_required += 1;
            }
            for (k, v) in methods {
                *f.methods.entry(k).or_default() += v;
            }
        });
        Ok(())
    }

    /// Build bases and queue the records of a slice.
    fn restore(
        &mut self,
        h: &encoding::CompressionHeader,
        s: &slice::SliceHeader,
        ext: &HashMap<i32, Vec<u8>>,
        dup_ids: &[(i32, Vec<u8>)],
        recs: Vec<slice::RawRecord>,
    ) {
        let any_mapped = recs.iter().any(|r| r.flag & 4 == 0);
        // the reference bases of this slice: embedded, or from the FASTA
        let mut embedded: Option<(i64, &[u8])> = None;
        let mut whole: Option<Arc<Vec<u8>>> = None;
        let mut mismatch = false;
        if any_mapped {
            if s.embedded_ref >= 0 {
                self.fact(|f| f.embedded_reference_slices += 1);
                let span = usize::try_from(s.span).unwrap_or(0);
                let cands = ext.get(&s.embedded_ref).into_iter().chain(
                    dup_ids
                        .iter()
                        .filter(|d| d.0 == s.embedded_ref)
                        .map(|d| &d.1),
                );
                let mut first = None;
                for c in cands {
                    first.get_or_insert(c);
                    let bases: Vec<u8> = c
                        .iter()
                        .take(span.max(c.len()))
                        .map(u8::to_ascii_uppercase)
                        .collect();
                    if s.md5 == [0u8; 16] || reference::md5(&bases) == s.md5 {
                        embedded = Some((s.start, c.as_slice()));
                        break;
                    }
                }
                if embedded.is_none() && first.is_some() {
                    mismatch = true;
                    self.fact(|f| {
                        f.reference_mismatches += 1;
                        f.reference_problem.get_or_insert(
                            "the reference embedded in a slice does not match the slice checksum (MD5)".into(),
                        );
                    });
                }
            } else if s.ref_id >= 0 {
                self.fact(|f| f.external_reference_slices += 1);
                whole = self.contig(s.ref_id);
                if let Some(c) = &whole
                    && s.md5 != [0u8; 16]
                    && s.start >= 1
                {
                    let a = usize::try_from(s.start - 1)
                        .unwrap_or(usize::MAX)
                        .min(c.len());
                    let b = usize::try_from(s.start - 1 + s.span)
                        .unwrap_or(usize::MAX)
                        .min(c.len());
                    if reference::md5(&c[a..b.max(a)]) != s.md5 {
                        mismatch = true;
                        let name = self
                            .refs
                            .get(s.ref_id as usize)
                            .map_or("?", |r| r.0.as_str())
                            .to_string();
                        self.fact(|f| {
                            f.reference_mismatches += 1;
                            f.reference_problem.get_or_insert(format!(
                                "the reference FASTA's sequence {name} does not match the CRAM slice checksum (MD5): it is not the reference the file was written against"
                            ));
                        });
                    }
                }
            } else if s.ref_id == -2 {
                self.fact(|f| f.external_reference_slices += 1);
            }
        }
        let mut no_bases = 0u64;
        for r in recs {
            let mut rec = Rec {
                name: r.name.clone(),
                flag: Some(r.flag),
                mapq: Some(if r.flag & 4 == 0 { r.mapq } else { 0 }),
                ref_id: Some(r.ref_id),
                pos: (r.pos > 0).then_some(r.pos),
                passes: r.passes,
                accuracy: r.accuracy,
                min_qchar: 33,
                ..Rec::default()
            };
            if r.cf & 8 != 0 {
                // the sequence is `*`: no bases and no qualities
            } else if let Some(b) = &r.bases {
                rec.seq = b.iter().map(u8::to_ascii_uppercase).collect();
                rec.qual = r.qual.clone();
            } else {
                let ref_id = r.ref_id;
                let contig = if mismatch {
                    None
                } else if s.embedded_ref >= 0 || ref_id == s.ref_id {
                    whole.clone()
                } else {
                    self.contig(ref_id)
                };
                let mut refb = |p: i64| -> Option<u8> {
                    if let Some((start, e)) = embedded {
                        let i = usize::try_from(p - start).ok()?;
                        return Some(e.get(i).map_or(b'N', u8::to_ascii_uppercase));
                    }
                    let c = contig.as_ref()?;
                    let i = usize::try_from(p - 1).ok()?;
                    Some(c.get(i).copied().unwrap_or(b'N'))
                };
                let (seq, known) = slice::build_bases(&r, &h.sub, &mut refb);
                rec.seq = seq;
                rec.bases_unknown = !known;
                rec.qual = r.qual.clone();
                if !known {
                    no_bases += 1;
                }
            }
            if rec.qual.first() == Some(&0xff) {
                rec.qual.clear();
            }
            self.pending.push_back(rec);
        }
        if no_bases > 0 {
            let have_fasta = self.fasta.is_some();
            self.fact(|f| {
                f.reads_without_bases += no_bases;
                if !have_fasta {
                    f.reference_problem.get_or_insert(
                        "the bases of aligned reads are stored against a reference sequence that is not in the file, and no reference FASTA was found beside it".into(),
                    );
                }
            });
        }
    }
}

impl Records for CramRecords {
    fn next_rec(&mut self, rec: &mut Rec) -> Result<bool> {
        while self.pending.is_empty() {
            if self.done || !self.next_container()? {
                self.done = true;
                return Ok(false);
            }
        }
        match self.pending.pop_front() {
            Some(r) => {
                *rec = r;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn consumed(&self) -> u64 {
        self.at
    }
}

/// Whether `head` starts with the CRAM magic and a version this reader knows.
pub fn is_cram(head: &[u8]) -> bool {
    head.len() >= 6 && &head[..4] == CRAM_MAGIC && (1..=4).contains(&head[4])
}

/// Decode `data` with CRAM block compression `method` (4 to 8: rANS 4x8, rANS Nx16, arithmetic,
/// fqzcomp, name tokeniser), for tests and fuzz targets. The size is taken from the stream.
pub fn decode_codec(method: u8, data: &[u8]) -> std::result::Result<Vec<u8>, String> {
    match method {
        4 => rans::decode_4x8(data),
        5 => rans::decode_nx16(data, None),
        6 => arith::decode(data, None),
        7 => fqzcomp::decode(data),
        8 => tok3::decode(data),
        m => Err(format!("method {m} is not a CRAM 3.x entropy codec")),
    }
}
