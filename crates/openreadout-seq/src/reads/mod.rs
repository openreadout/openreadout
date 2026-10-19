//! FASTQ, BAM and CRAM read files: what a sequencer (or its basecaller) hands on.
//!
//! `info` streams the first part of the file (up to [`INFO_SCAN_BYTES`] of it, so it stays fast
//! on files of hundreds of gigabytes) and reports read counts, lengths (N50), qualities and
//! base composition, exact when the scan reached the end and estimated from the bytes read
//! otherwise (`tables[0].extra.read_count_exact`). An indexed BAM (`.bai` beside it) gives exact
//! mapped and unmapped counts without a scan. `check` reads every record and reports the exact
//! totals. Table 0 lists one row per record (length, mean quality, GC, and for BAM the flag,
//! mapping quality, position and PacBio passes and accuracy); tables 1–3 are the quality by
//! position, the read-length histogram and the read-quality histogram of the scanned reads.
//! CRAM records come from [`crate::cram`] and fill the same table as BAM.

pub mod records;
pub mod stats;

use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use openreadout_core::experiment::{Experiment, ExperimentInstrument, Method, Quantity, Sample};
use openreadout_core::model::{
    CheckReport, DetectConfidence, FileInfo, Finding, FormatDescriptor, LsEntry, Table,
};
use openreadout_core::pixel::Plane;
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, Detection, FormatReader, PlaneIndex, has_extension};
use openreadout_core::source::Input;
use openreadout_core::{Error, Result, vocab};
use serde_json::{Value, json};

use crate::cram::{self, CramFacts, CramHeader, CramRecords, Fasta};
use crate::table::{Col, MemTable, tidy};
use records::{BamHeader, BamRecords, FastqRecords, Rec, Records};
use stats::ReadStats;

/// Format id of FASTQ files.
pub const FASTQ_FORMAT_ID: &str = "fastq";
/// Format id of BAM files.
pub const BAM_FORMAT_ID: &str = "bam";
/// Format id of CRAM files.
pub const CRAM_FORMAT_ID: &str = "cram";

/// What to do when a CRAM file's reference is missing.
pub const REFERENCE_HINT: &str = "Put the reference FASTA (uncompressed, with or without its .fai index) beside the CRAM file with the same name and the extension .fa, .fasta or .fna, then run the command again.";

/// File bytes `info` reads at most (compressed bytes for gzip and BGZF).
pub const INFO_SCAN_BYTES: u64 = 32 << 20;

/// FASTQ or BAM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadFormat {
    /// FASTQ, plain or gzip.
    Fastq,
    /// BAM.
    Bam,
    /// CRAM.
    Cram,
}

/// What the scan of `info` found.
#[derive(Debug, Clone)]
struct Scan {
    stats: ReadStats,
    /// Every record (BAM: including secondary and supplementary alignments).
    records: u64,
    /// Primary alignments that are mapped (BAM).
    mapped: u64,
    complete: bool,
    consumed: u64,
    first_name: Vec<u8>,
    /// The first FASTQ title line, without the `@` (empty for BAM).
    first_title: Vec<u8>,
    /// CRAM: what decoding found.
    cram: Option<CramFacts>,
}

/// A CRAM file's header and reference.
struct CramState {
    header: CramHeader,
    fasta: Option<Arc<Fasta>>,
    /// A reference FASTA was found but could not be used.
    fasta_problem: Option<String>,
}

/// An open FASTQ or BAM file.
pub struct ReadsDataset {
    descriptor: FormatDescriptor,
    format: ReadFormat,
    input: Input,
    size: u64,
    gz: bool,
    header: Option<BamHeader>,
    bai: Option<(u64, u64)>,
    scan: OnceCell<std::result::Result<Scan, String>>,
    cursor: Option<(Box<dyn Records>, u64)>,
    cram: Option<CramState>,
}

impl std::fmt::Debug for ReadsDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadsDataset")
            .field("format", &self.format)
            .field("path", &self.input.path())
            .finish_non_exhaustive()
    }
}

impl ReadsDataset {
    fn fid(&self) -> &'static str {
        match self.format {
            ReadFormat::Fastq => FASTQ_FORMAT_ID,
            ReadFormat::Bam => BAM_FORMAT_ID,
            ReadFormat::Cram => CRAM_FORMAT_ID,
        }
    }

    fn records(&self) -> Result<Box<dyn Records>> {
        self.records_with(Arc::new(Mutex::new(CramFacts::default())))
    }

    /// The record stream; CRAM decoding reports what it found to `facts`.
    fn records_with(&self, facts: Arc<Mutex<CramFacts>>) -> Result<Box<dyn Records>> {
        let f = self.input.open()?;
        if let Some(c) = &self.cram {
            return Ok(Box::new(CramRecords::new(
                f,
                &c.header,
                c.fasta.clone(),
                facts,
            )?));
        }
        let (r, n) = records::open_stream(f, self.gz);
        Ok(match self.format {
            ReadFormat::Fastq => Box::new(FastqRecords::new(r, n, FASTQ_FORMAT_ID)),
            ReadFormat::Bam | ReadFormat::Cram => Box::new(BamRecords::new(r, n)?),
        })
    }

    fn has_alignments(&self) -> bool {
        self.format != ReadFormat::Fastq
    }

    /// Stream records, feeding primary reads to the statistics, until the end or `budget`
    /// file bytes.
    fn run_scan(&self, budget: u64) -> Result<Scan> {
        let facts = Arc::new(Mutex::new(CramFacts::default()));
        let mut recs = self.records_with(facts.clone())?;
        let mut s = Scan {
            stats: ReadStats::default(),
            records: 0,
            mapped: 0,
            complete: false,
            consumed: 0,
            first_name: Vec::new(),
            first_title: if self.format == ReadFormat::Fastq {
                self.first_title()?
            } else {
                Vec::new()
            },
            cram: None,
        };
        let mut rec = Rec::default();
        loop {
            if !recs.next_rec(&mut rec)? {
                s.complete = true;
                break;
            }
            if s.records == 0 {
                s.first_name = rec.name.clone();
            }
            s.records += 1;
            if !rec.is_extra_alignment() {
                if rec.bases_unknown {
                    s.stats.add_without_bases(rec.seq.len() as u64, &rec.qual);
                } else {
                    s.stats.add(&rec.seq, &rec.qual);
                }
                if rec.min_qchar < s.stats.min_qchar {
                    s.stats.min_qchar = rec.min_qchar;
                }
                if rec.flag.is_some_and(|f| f & 4 == 0) {
                    s.mapped += 1;
                }
            }
            if recs.consumed() >= budget {
                break;
            }
        }
        s.consumed = recs.consumed();
        drop(recs);
        if self.cram.is_some() {
            s.cram = facts.lock().ok().map(|f| f.clone());
        }
        Ok(s)
    }

    /// The first line of a FASTQ file without the `@` and the line ending (at most 64 KiB).
    fn first_title(&self) -> Result<Vec<u8>> {
        use std::io::{BufRead, Read};
        let (mut r, _) = records::open_stream(self.input.open()?, self.gz);
        let mut line = Vec::new();
        // a read error here also stops the scan that follows, which reports it
        let _ = (&mut r).take(64 << 10).read_until(b'\n', &mut line);
        while line.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
            line.pop();
        }
        Ok(line
            .strip_prefix(b"@")
            .map(<[u8]>::to_vec)
            .unwrap_or_default())
    }

    fn scan(&self) -> Result<&Scan> {
        let s = self
            .scan
            .get_or_init(|| self.run_scan(INFO_SCAN_BYTES).map_err(|e| e.to_string()));
        s.as_ref()
            .map_err(|e| Error::corrupt(self.fid(), e.clone()))
    }

    fn estimate(&self, s: &Scan, n: u64) -> u64 {
        if s.complete || s.consumed == 0 {
            n
        } else {
            (n as f64 * self.size as f64 / s.consumed as f64).round() as u64
        }
    }

    fn read_columns(&self) -> Vec<Col> {
        let mut c = vec![
            Col::int("length"),
            Col::num("mean_quality", None),
            Col::num("gc_percent", Some("%")),
            Col::int("n_bases"),
        ];
        if self.has_alignments() {
            c.extend([
                Col::int("flag"),
                Col::int("mapq"),
                Col::int("ref_id"),
                Col::int("pos"),
                Col::int("passes"),
                Col::num("accuracy", None),
            ]);
        }
        c
    }

    /// The note `info` adds when CRAM bases could not be restored.
    fn cram_bases_note(&self, s: &Scan) -> Option<String> {
        let f = s.cram.as_ref()?;
        if f.reads_without_bases == 0 {
            return None;
        }
        let why = self
            .cram
            .as_ref()
            .and_then(|c| c.fasta_problem.clone())
            .or_else(|| f.reference_problem.clone())
            .unwrap_or_else(|| "the reference sequence is not available".into());
        Some(format!(
            "the bases of {} reads were not restored: {why}. Their GC and N counts are left out (null); lengths, qualities, flags and positions do not need the reference. {REFERENCE_HINT}",
            f.reads_without_bases
        ))
    }

    fn header_facts(&self) -> BTreeMap<String, Value> {
        let mut m = BTreeMap::new();
        let Some(h) = &self.header else {
            return m;
        };
        let mut rgs = Vec::new();
        let mut pgs = Vec::new();
        for line in h.text.lines() {
            let mut f = line.split('\t');
            let kind = f.next().unwrap_or_default();
            let kv: BTreeMap<&str, &str> = f.filter_map(|x| x.split_once(':')).collect();
            match kind {
                "@HD" => {
                    if let Some(so) = kv.get("SO") {
                        m.insert("sort_order".into(), json!(so));
                    }
                    if let Some(v) = kv.get("VN") {
                        m.insert("sam_version".into(), json!(v));
                    }
                }
                "@RG" => rgs.push(json!(kv)),
                "@PG" => pgs.push(json!(kv)),
                _ => {}
            }
        }
        if let Some(pl) = rgs.iter().find_map(|r| r.get("PL").and_then(Value::as_str)) {
            m.insert("platform".into(), json!(pl.to_ascii_uppercase()));
        }
        if !rgs.is_empty() {
            m.insert("read_groups".into(), json!(rgs));
        }
        // the program that wrote the file: the last @PG that no other @PG names as its
        // previous program (`PP`)
        let previous: Vec<&str> = pgs
            .iter()
            .filter_map(|p| p.get("PP").and_then(Value::as_str))
            .collect();
        if let Some(w) = pgs
            .iter()
            .rev()
            .find(|p| {
                p.get("ID")
                    .and_then(Value::as_str)
                    .is_none_or(|id| !previous.contains(&id))
            })
            .and_then(|p| p.get("PN").or_else(|| p.get("ID")))
            .and_then(Value::as_str)
        {
            m.insert("writer".into(), json!(w));
        }
        if !pgs.is_empty() {
            m.insert("programs".into(), json!(pgs));
        }
        m.insert("references".into(), json!(h.refs.len()));
        m.insert(
            "reference_length".into(),
            json!(h.refs.iter().map(|r| r.1).sum::<u64>()),
        );
        m
    }

    /// Instrument facts from the first read's name: Illumina
    /// (`instrument:run:flowcell:lane:tile:x:y`), Oxford Nanopore (`key=value` comments are not
    /// kept here) or PacBio (`m<instrument>_<date>_<time>/<zmw>/...`).
    fn name_facts(name: &[u8]) -> BTreeMap<String, Value> {
        let mut m = BTreeMap::new();
        let n = String::from_utf8_lossy(name);
        let parts: Vec<&str> = n.split(':').collect();
        if parts.len() == 7 && parts[3].parse::<u32>().is_ok() && parts[4].parse::<u32>().is_ok() {
            m.insert("name_style".into(), json!("illumina"));
            m.insert("instrument".into(), json!(parts[0]));
            m.insert("run_number".into(), json!(parts[1]));
            m.insert("flowcell".into(), json!(parts[2]));
        } else if let Some(movie) = n
            .split('/')
            .next()
            .filter(|s| s.starts_with('m') && s.matches('_').count() >= 2 && n.contains('/'))
        {
            m.insert("name_style".into(), json!("pacbio"));
            m.insert("movie".into(), json!(movie));
        } else if n.len() == 36 && n.matches('-').count() == 4 {
            m.insert("name_style".into(), json!("uuid (Oxford Nanopore)"));
        }
        m
    }
}

/// The convention of a FASTQ title line (without the `@`), which tells which kind of software
/// wrote the file: Illumina 1.8+ (`instrument:run:flowcell:lane:tile:x:y read:filtered:control:index`),
/// Illumina before 1.8 (`instrument:lane:tile:x:y#index/read`), PacBio (`movie/zmw/...`),
/// Oxford Nanopore read ids with MinKNOW `key=value` comments or with SAM tags (`qs:f:12.5`, as
/// `samtools fastq -T` writes them from a basecaller's BAM), or SRA (`SRR123.1`).
fn title_convention(title: &[u8]) -> Option<&'static str> {
    let t = String::from_utf8_lossy(title);
    let (name, comment) = t
        .split_once([' ', '\t'])
        .map_or((t.as_ref(), ""), |(n, c)| (n, c));
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let parts: Vec<&str> = name.split(':').collect();
    if parts.len() == 7 && digits(parts[3]) && digits(parts[4]) {
        return Some("illumina 1.8+");
    }
    let bare = name.split(['#', '/']).next().unwrap_or(name);
    let old: Vec<&str> = bare.split(':').collect();
    if old.len() == 5 && old[1..].iter().all(|p| digits(p)) {
        return Some("illumina before 1.8");
    }
    let movie = name.split('/').next().unwrap_or("");
    if name.contains('/') && movie.starts_with('m') && movie.matches('_').count() >= 2 {
        return Some("pacbio");
    }
    if name.len() == 36 && name.matches('-').count() == 4 {
        let sam_tag = |w: &str| {
            let b = w.as_bytes();
            b.len() > 5
                && b[0].is_ascii_alphabetic()
                && b[1].is_ascii_alphanumeric()
                && b[2] == b':'
                && b"AifZHB".contains(&b[3])
                && b[4] == b':'
        };
        return Some(
            if comment.split_whitespace().any(|w| w.starts_with("runid=")) {
                "nanopore minknow"
            } else if comment.split_whitespace().any(sam_tag) {
                "nanopore sam tags"
            } else {
                "nanopore"
            },
        );
    }
    let sra = name
        .strip_prefix("SRR")
        .or_else(|| name.strip_prefix("ERR"))
        .or_else(|| name.strip_prefix("DRR"))
        .and_then(|r| r.split_once('.'))
        .is_some_and(|(run, spot)| digits(run) && digits(spot.split('/').next().unwrap_or("")));
    sra.then_some("sra")
}

impl Dataset for ReadsDataset {
    fn info(&self) -> Result<FileInfo> {
        let s = self.scan()?;
        let mut reads = MemTable::new("reads", self.read_columns());
        reads.extra.insert(
            "kind".into(),
            json!(match self.format {
                ReadFormat::Fastq => "fastq_reads",
                ReadFormat::Bam => "bam_records",
                ReadFormat::Cram => "cram_records",
            }),
        );
        let mut info0 = reads.info(0);
        let (rows, exact) = match (self.bai, self.format) {
            (Some((m, u)), ReadFormat::Bam) if !s.complete => (m + u, true),
            _ => (self.estimate(s, s.records), s.complete),
        };
        info0.row_count = rows;
        let e = &mut info0.extra;
        e.insert("read_count_exact".into(), json!(exact));
        if !s.complete {
            e.insert(
                "read_count_basis".into(),
                json!(if exact {
                    "the BAM index (.bai)".to_string()
                } else {
                    format!(
                        "estimated from {} records in the first {:.1} MB of {:.1} MB; `check` counts them all",
                        s.records,
                        s.consumed as f64 / 1e6,
                        self.size as f64 / 1e6
                    )
                }),
            );
        }
        let mut summary = s.stats.summary();
        summary["basis"] = json!(if s.complete {
            "every read".to_string()
        } else {
            format!("the first {} reads", s.stats.reads)
        });
        if !s.complete && !exact {
            summary["reads_estimated"] = json!(self.estimate(s, s.stats.reads));
            summary["bases_estimated"] = json!(self.estimate(s, s.stats.bases));
        }
        if self.has_alignments() {
            summary["records"] = json!(s.records);
            summary["mapped"] = json!(s.mapped);
            if let Some((m, u)) = self.bai {
                summary["index_mapped"] = json!(m);
                summary["index_unmapped"] = json!(u);
            }
        }
        e.insert("summary".into(), summary);
        e.insert(
            "compression".into(),
            json!(match (self.format, self.gz) {
                (ReadFormat::Bam, _) => "bgzf",
                (ReadFormat::Cram, _) => "cram blocks",
                (_, true) => "gzip",
                _ => "none",
            }),
        );
        if let Some(c) = &self.cram {
            e.insert(
                "cram_version".into(),
                json!(format!("{}.{}", c.header.major, c.header.minor)),
            );
            if let Some(f) = &s.cram {
                e.insert("cram_codecs".into(), json!(f.methods));
                let reference =
                    if f.external_reference_slices == 0 && f.embedded_reference_slices == 0 {
                        "not needed (no aligned reads, or bases stored verbatim)".to_string()
                    } else if let Some(fa) = &c.fasta {
                        format!(
                            "{}{}",
                            fa.path
                                .file_name()
                                .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                            if f.embedded_reference_slices > 0 {
                                " and embedded in the file"
                            } else {
                                ""
                            }
                        )
                    } else if f.external_reference_slices == 0 {
                        "embedded in the file".to_string()
                    } else {
                        "not found".to_string()
                    };
                e.insert(
                    "reference".into(),
                    json!({
                        "source": reference,
                        "slices_embedded": f.embedded_reference_slices,
                        "slices_external": f.external_reference_slices,
                        "containers_reference_not_required": f.reference_not_required,
                        "slices_checksum_mismatch": f.reference_mismatches,
                    }),
                );
            }
        }
        if self.format == ReadFormat::Fastq && s.stats.reads > 0 {
            e.insert(
                "quality_encoding".into(),
                json!(if s.stats.min_qchar >= 64 {
                    "phred+64 or phred+33 without low scores"
                } else {
                    "phred+33"
                }),
            );
        }
        for (k, v) in self.header_facts() {
            e.insert(k, v);
        }
        if let Some(c) = title_convention(&s.first_title) {
            e.insert("title_convention".into(), json!(c));
        }
        for (k, v) in Self::name_facts(&s.first_name) {
            e.entry(k).or_insert(v);
        }
        let mut t1 = s.stats.position_table();
        let mut t2 = s.stats.length_table();
        let mut t3 = s.stats.quality_table();
        for t in [&mut t1, &mut t2, &mut t3] {
            t.extra.insert(
                "basis".into(),
                json!(if s.complete {
                    "every read"
                } else {
                    "the reads info scanned"
                }),
            );
        }
        let mut notes = if s.complete || exact {
            vec![]
        } else {
            vec![format!(
                "read counts are estimated: info reads the first {:.0} MB; `check` reads every record",
                INFO_SCAN_BYTES as f64 / 1e6
            )]
        };
        if let Some(n) = self.cram_bases_note(s) {
            notes.push(n);
        }
        Ok(FileInfo {
            path: self.input.path().display().to_string(),
            size_bytes: self.size,
            format: self.descriptor.clone(),
            format_version: match &self.cram {
                Some(c) => Some(format!("{}.{}", c.header.major, c.header.minor)),
                None => self
                    .header_facts()
                    .get("sam_version")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            },
            images: vec![],
            tables: vec![info0, t1.info(1), t2.info(2), t3.info(3)],
            spectra: vec![],
            traces: vec![],
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        Ok(match &self.header {
            Some(h) => json!({"header": h.text, "references": h.refs.len()}),
            None => Value::Null,
        })
    }

    fn provenance(&self) -> ProvenanceMap {
        ProvenanceMap::from([("tables[0]".into(), Source::Spec)])
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut e = Vec::new();
        if let Some(h) = &self.header {
            for (name, len) in h.refs.iter().take(1000) {
                e.push(LsEntry {
                    kind: "reference".into(),
                    name: name.clone(),
                    offset: None,
                    size: Some(*len),
                    image: None,
                    details: Value::Null,
                });
            }
        }
        Ok(e)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::Usage(
            "a read file holds reads (tables), not images".into(),
        ))
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        if index > 0 {
            let s = self.scan()?;
            let t = match index {
                1 => s.stats.position_table(),
                2 => s.stats.length_table(),
                3 => s.stats.quality_table(),
                _ => {
                    return Err(Error::Usage(format!(
                        "table {index} does not exist; tables 0..3 (reads, quality by position, read lengths, read quality)"
                    )));
                }
            };
            return Ok(t.slice(index, first_row, max_rows));
        }
        // continue the previous read when it ended where this one starts
        let (mut recs, mut at) = match self.cursor.take() {
            Some((r, at)) if at <= first_row => (r, at),
            _ => (self.records()?, 0),
        };
        let mut rec = Rec::default();
        let mut stats = ReadStats::default();
        let mut cols = self.read_columns();
        while at < first_row {
            if !recs.next_rec(&mut rec)? {
                break;
            }
            at += 1;
        }
        let mut got = 0u64;
        while got < max_rows {
            if !recs.next_rec(&mut rec)? {
                break;
            }
            at += 1;
            got += 1;
            let v = if rec.bases_unknown {
                stats.add_without_bases(rec.seq.len() as u64, &rec.qual)
            } else {
                stats.add(&rec.seq, &rec.qual)
            };
            cols[0].push(Some(v.length as f64));
            cols[1].push(Some(v.mean_quality).filter(|x| x.is_finite()));
            cols[2].push(Some(v.gc_percent).filter(|x| x.is_finite()));
            cols[3].push(v.n_count.map(|n| n as f64));
            if self.has_alignments() {
                cols[4].push(rec.flag.map(f64::from));
                cols[5].push(rec.mapq.map(f64::from));
                cols[6].push(rec.ref_id.map(f64::from));
                cols[7].push(rec.pos.map(|p| p as f64));
                cols[8].push(rec.passes.map(|p| p as f64));
                cols[9].push(rec.accuracy);
            }
        }
        self.cursor = Some((recs, at));
        Ok(Table {
            table: 0,
            first_row,
            columns: cols.into_iter().map(|c| c.values).collect(),
        })
    }

    fn check_headers(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.input.path().display().to_string(), self.fid());
        r.performed("header and the first records parsed");
        self.records()?;
        Ok(r)
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.input.path().display().to_string(), self.fid());
        r.performed("every record parsed (lengths, qualities, record structure)");
        match self.run_scan(u64::MAX) {
            Ok(s) => {
                let sum = s.stats.summary();
                r.push(Finding::info(
                    "read_counts",
                    format!(
                        "{} records, {} reads, {} bases, N50 {}, {}% ≥ Q30",
                        s.records,
                        s.stats.reads,
                        s.stats.bases,
                        s.stats.n50().unwrap_or(0),
                        sum["percent_q30"]
                            .as_f64()
                            .map_or("-".into(), |v| format!("{}", tidy(v, 4)))
                    ),
                ));
                if let Some(f) = &s.cram {
                    r.performed("decompressed every CRAM block and checked the CRC32 of container headers and blocks (CRAM 3)");
                    if f.crc_failures > 0 {
                        r.push(Finding::error(
                            "crc_mismatch",
                            format!(
                                "{} CRAM blocks or container headers fail their CRC32",
                                f.crc_failures
                            ),
                        ));
                    }
                    if f.reference_mismatches > 0 {
                        r.push(Finding::error(
                            "reference_mismatch",
                            format!(
                                "{} slices: the reference FASTA does not match the slice checksum (MD5), so their bases were not restored. {REFERENCE_HINT}",
                                f.reference_mismatches
                            ),
                        ));
                    } else if f.reads_without_bases > 0 {
                        r.push(Finding::warning(
                            "reference_missing",
                            self.cram_bases_note(&s).unwrap_or_default(),
                        ));
                    }
                }
                if let (Some((m, u)), ReadFormat::Bam) = (self.bai, self.format) {
                    let primary = s.stats.reads;
                    if m + u != primary && m + u != s.records {
                        r.push(Finding::warning(
                            "index_mismatch",
                            format!("the .bai counts {} reads, the file holds {primary} primary records; the index may be stale", m + u),
                        ));
                    }
                }
            }
            Err(e) => r.push(Finding::error("records_unreadable", e.to_string())),
        }
        Ok(r)
    }

    fn experiment(&self) -> Option<Experiment> {
        let s = self.scan().ok()?;
        let mut e = Experiment::default();
        let facts = self.header_facts();
        let names = Self::name_facts(&s.first_name);
        // the platform comes from the header's @RG PL, else from the style of the read names
        let platform_in_header = facts.get("platform").and_then(Value::as_str).is_some();
        let platform = facts
            .get("platform")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| match names.get("name_style").and_then(Value::as_str) {
                Some("illumina") => Some("ILLUMINA".into()),
                Some("pacbio") => Some("PACBIO".into()),
                Some(s) if s.starts_with("uuid") => Some("ONT".into()),
                _ => None,
            });
        let vendor = platform.as_deref().map(|p| match p {
            "ILLUMINA" => "Illumina",
            "PACBIO" => "Pacific Biosciences",
            "ONT" => "Oxford Nanopore Technologies",
            "ELEMENT" => "Element Biosciences",
            "ULTIMA" => "Ultima Genomics",
            "IONTORRENT" => "Thermo Fisher Scientific",
            "DNBSEQ" | "BGI" | "MGI" => "MGI",
            other => other,
        });
        let rg = facts
            .get("read_groups")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .cloned()
            .unwrap_or(Value::Null);
        e.instrument = Some(ExperimentInstrument {
            vendor: vendor.map(str::to_string),
            model: rg.get("PM").and_then(Value::as_str).map(str::to_string),
            serial: names
                .get("instrument")
                .and_then(Value::as_str)
                .map(str::to_string),
            software: facts
                .get("programs")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(|p| p.get("PN").or_else(|| p.get("ID")))
                .and_then(Value::as_str)
                .map(str::to_string),
            kind: vocab::term("OBI:0400103"),
            ..ExperimentInstrument::default()
        });
        let mut method = Method {
            technique: vocab::term("OBI:0000626"),
            assay: vocab::term("OBI:0000626"),
            ..Method::default()
        };
        if let Some(p) = &platform {
            method
                .parameters
                .insert("platform".into(), Quantity::plain(p.clone()));
        }
        if let Some(f) = names.get("flowcell") {
            method
                .parameters
                .insert("flowcell".into(), Quantity::plain(f.clone()));
        }
        e.method = Some(method);
        if let Some(sm) = rg.get("SM").and_then(Value::as_str) {
            e.sample = Some(Sample {
                id: Some(sm.to_string()),
                source_field: Some("@RG SM".into()),
                ..Sample::default()
            });
        }
        crate::cover(&mut e, |path| match path {
            "sample.id" => (Source::Spec, "@RG SM"),
            "instrument.model" => (Source::Spec, "@RG PM"),
            "instrument.software" => (Source::Spec, "@PG PN"),
            "instrument.vendor" | "method.parameters.platform" if platform_in_header => {
                (Source::Spec, "@RG PL")
            }
            "instrument.vendor" | "method.parameters.platform" => (Source::Inferred, "read names"),
            "instrument.serial" | "method.parameters.flowcell" => (Source::Inferred, "read names"),
            // the instrument kind, the technique and the assay follow from the format itself
            _ => (Source::Inferred, "format"),
        });
        Some(e)
    }
}

/// True when `head` (decompressed) looks like FASTQ: an `@` title, a sequence and a `+` line.
fn looks_like_fastq(b: &[u8]) -> bool {
    let mut lines = b.split(|c| *c == b'\n');
    let (Some(t), Some(s), Some(p)) = (lines.next(), lines.next(), lines.next()) else {
        return false;
    };
    t.first() == Some(&b'@')
        && !s.is_empty()
        && s.iter().all(|c| {
            c.is_ascii_alphabetic() || *c == b'\r' || *c == b'.' || *c == b'-' || *c == b'*'
        })
        && p.first() == Some(&b'+')
}

/// A BGZF member: gzip with the `BC` extra subfield.
fn is_bgzf(head: &[u8]) -> bool {
    head.len() >= 18
        && openreadout_core::gzip::is_gzip(head)
        && head[3] & 4 != 0
        && head[12] == b'B'
        && head[13] == b'C'
}

/// Reader of FASTQ files (`.fastq`, `.fq`, optionally gzip-compressed).
#[derive(Debug, Default, Clone, Copy)]
pub struct FastqReader;

/// Reader of BAM files.
#[derive(Debug, Default, Clone, Copy)]
pub struct BamReader;

/// Reader of CRAM files.
#[derive(Debug, Default, Clone, Copy)]
pub struct CramReader;

fn open_reads(desc: FormatDescriptor, format: ReadFormat, input: &Input) -> Result<ReadsDataset> {
    let mut head = [0u8; 4];
    let f = input.open()?;
    let size = f.size().map_err(|e| Error::io(input.path(), e))?;
    let gz = f.read_exact_at(0, &mut head).is_ok() && openreadout_core::gzip::is_gzip(&head);
    let mut ds = ReadsDataset {
        descriptor: desc,
        format,
        input: input.clone(),
        size,
        gz: gz || format == ReadFormat::Bam,
        header: None,
        bai: None,
        scan: OnceCell::new(),
        cursor: None,
        cram: None,
    };
    if format == ReadFormat::Cram {
        let header = cram::read_header(&input.open()?)?;
        ds.header = Some(header.sam.clone());
        let mut state = CramState {
            header,
            fasta: None,
            fasta_problem: None,
        };
        if let Some(p) = cram::find_reference(input, &state.header.sam) {
            match Fasta::open(input.fs(), &p) {
                Ok(f) => state.fasta = Some(Arc::new(f)),
                Err(e) => {
                    state.fasta_problem = Some(format!(
                        "the reference FASTA {} could not be read ({e})",
                        p.display()
                    ));
                }
            }
        }
        ds.cram = Some(state);
        // decode the first container now, so a damaged file fails at open
        let mut recs = ds.records()?;
        recs.next_rec(&mut Rec::default())?;
    } else if format == ReadFormat::Bam {
        let (r, n) = records::open_stream(input.open()?, true);
        let b = BamRecords::new(r, n)?;
        ds.header = Some(b.header.clone());
        let fs = input.fs();
        for p in [
            PathBuf::from(format!("{}.bai", input.path().display())),
            input.path().with_extension("bai"),
        ] {
            if fs.is_file(&p)
                && let Ok(bytes) = fs.read(&p)
            {
                ds.bai = records::bai_counts(&bytes);
                break;
            }
        }
    } else {
        // parse the first record now, so a damaged file fails at open
        let mut recs = ds.records()?;
        recs.next_rec(&mut Rec::default())?;
    }
    Ok(ds)
}

impl FormatReader for FastqReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::FASTQ)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FASTQ_FORMAT_ID.into(),
            name: "FASTQ".into(),
            vendor: String::new(),
            extensions: vec!["fastq".into(), "fq".into(), "fastq.gz".into(), "fq.gz".into()],
            family: "sequencing".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::FASTQ.confidence,
            known_gaps: vec![
                "info estimates read counts of files larger than its scan (32 MB of file); check counts every read".into(),
                "Read names are not returned in tables; sequences and qualities are summarised, not listed".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        let text = if openreadout_core::gzip::is_gzip(head) {
            if is_bgzf(head)
                && openreadout_core::gzip::peek(head, 4).is_some_and(|b| b.starts_with(b"BAM"))
            {
                return None;
            }
            openreadout_core::gzip::peek(head, 16 << 10)?
        } else {
            head.to_vec()
        };
        if looks_like_fastq(&text) {
            return Some(Detection {
                format_id: FASTQ_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        let name = path.to_string_lossy().to_ascii_lowercase();
        [".fastq", ".fq", ".fastq.gz", ".fq.gz"]
            .iter()
            .any(|x| name.ends_with(x))
            .then_some(Detection {
                format_id: FASTQ_FORMAT_ID,
                confidence: DetectConfidence::ExtensionOnly,
                note: Some("FASTQ extension but the first lines are not a FASTQ record".into()),
            })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(open_reads(
            self.descriptor(),
            ReadFormat::Fastq,
            input,
        )?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

impl FormatReader for BamReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::BAM)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: BAM_FORMAT_ID.into(),
            name: "BAM (binary alignment/map)".into(),
            vendor: String::new(),
            extensions: vec!["bam".into()],
            family: "sequencing".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::BAM.confidence,
            known_gaps: vec![
                "Summaries count primary records as reads; secondary and supplementary alignments are listed in table 0 but not counted".into(),
                "Alignments are not interpreted (no CIGAR, coverage or pileup): use samtools for that".into(),
                "CRAM is read by its own reader (`cram`)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if is_bgzf(head)
            && openreadout_core::gzip::peek(head, 4).is_some_and(|b| b.starts_with(b"BAM\x01"))
        {
            return Some(Detection {
                format_id: BAM_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        has_extension(path, &["bam"]).then_some(Detection {
            format_id: BAM_FORMAT_ID,
            confidence: DetectConfidence::ExtensionOnly,
            note: Some("BAM extension but no BGZF block holding BAM\\1".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(open_reads(
            self.descriptor(),
            ReadFormat::Bam,
            input,
        )?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

impl FormatReader for CramReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::CRAM)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: CRAM_FORMAT_ID.into(),
            name: "CRAM (reference-compressed alignments)".into(),
            vendor: String::new(),
            extensions: vec!["cram".into()],
            family: "sequencing".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::CRAM.confidence,
            known_gaps: vec![
                "Bases of aligned reads need the reference: embedded in the file, or a FASTA beside it with the same name (.fa, .fasta, .fna); without it their GC and N counts are null and a note says so".into(),
                "Summaries count primary records as reads; secondary and supplementary alignments are listed in table 0 but not counted".into(),
                "Alignments are not interpreted (no CIGAR, coverage or pileup): use samtools for that".into(),
                "CRAM 1.0 and the draft 4.0 are not read (exit 6)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if cram::is_cram(head) {
            return Some(Detection {
                format_id: CRAM_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        (has_extension(path, &["cram"]) && !head.is_empty()).then_some(Detection {
            format_id: CRAM_FORMAT_ID,
            confidence: DetectConfidence::ExtensionOnly,
            note: Some("CRAM extension but no CRAM magic".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(open_reads(
            self.descriptor(),
            ReadFormat::Cram,
            input,
        )?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fastq_sniffing() {
        assert!(looks_like_fastq(b"@r1\nACGT\n+\nIIII\n"));
        assert!(!looks_like_fastq(b">r1\nACGT\n"));
        assert!(!looks_like_fastq(b"@r1\n\n+\n"));
        let f = FastqReader.sniff(b"@r1\nACGT\n+\nIIII\n", Path::new("x.txt"));
        assert_eq!(f.unwrap().confidence, DetectConfidence::Definite);
        let facts = ReadsDataset::name_facts(b"A00834:183:HMTFYDRXX:1:2101:1000:1000");
        assert_eq!(facts["flowcell"], json!("HMTFYDRXX"));
    }

    /// Title lines as the development files have them (`docs/provenance/sequence-reads.md`).
    #[test]
    fn title_conventions() {
        let c = |t: &str| title_convention(t.as_bytes());
        assert_eq!(
            c("ST-E00493:56:H33MFALXX:4:1101:23439:1379 1:N:0:NACAACCA"),
            Some("illumina 1.8+")
        );
        assert_eq!(
            c("HWI-EAS209_0006_FC706VJ:5:58:5894:21141#ATCACG/1"),
            Some("illumina before 1.8")
        );
        assert_eq!(c("m64011_190830_220126/1/ccs"), Some("pacbio"));
        assert_eq!(
            c("83ccd09b-02bf-4623-b1e5-2233a3fb1d35 runid=8c23 read=27944 ch=481"),
            Some("nanopore minknow")
        );
        assert_eq!(
            c("9ae4818c-61e9-4011-bf40-ca5721922da4\tqs:f:12.4562\tch:i:783"),
            Some("nanopore sam tags")
        );
        assert_eq!(c("c4f3c65f-4440-4a5f-80dd-cf576cea6e51"), Some("nanopore"));
        assert_eq!(c("SRR1234567.1 1 length=100"), Some("sra"));
        assert_eq!(c("1"), None);
        assert_eq!(c("FSRRS4401BE7HA [length=395]"), None);
    }
}
