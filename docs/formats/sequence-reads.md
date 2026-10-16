# FASTQ and BAM read files

FASTQ and unaligned or aligned BAM are what sequencers and basecallers hand on: Illumina's
conversion tools write FASTQ, PacBio HiFi and Oxford Nanopore basecallers write BAM. OpenReadout
summarises them as instrument output (counts, lengths, qualities, header facts); it is not an
alignment tool and does not interpret CIGARs, coverage or variants.

Both are open formats: FASTQ as Cock et al. (2010) describe it, BAM as the SAMv1 specification
and the PacBio BAM documentation define it. pysam and Biopython are the ground truth.
Provenance: `docs/provenance/sequence-reads.md`. Crate: `openreadout-seq` (`src/reads/`).

| format id | files | reads | confidence |
| --- | --- | --- | --- |
| `fastq` | `.fastq`, `.fq`, gzip-compressed or not | four-line and multi-line records, Phred+33 | evidence rubric (`docs/assurance.md`) |
| `bam` | `.bam` (BGZF) | header, every record, `np`/`rq` tags, `.bai` counts | evidence rubric |

## Reading

- **FASTQ**: a title line starting with `@` (the name is the text up to the first space), the
  sequence (one or more lines), a `+` line, the qualities (as many as bases; characters `!`–`~`,
  Phred+33). gzip files (and concatenated gzip members) are streamed. Files whose smallest
  quality character is `@` or above are reported as possibly Phred+64.
- **BAM**: BGZF blocks (gzip members with the `BC` subfield) streamed as one stream; `BAM\1`,
  the header text, the reference list, then records: refID, pos, read-name length, MAPQ, bin,
  CIGAR count, flag, sequence length, mate fields, name, CIGAR, 4-bit sequence
  (`=ACMGRSVTWYHKDBN`), qualities (`0xFF` first: none), auxiliary fields (types `AcCsSiIfZHB`;
  `np` and `rq` are kept). Header facts: `@HD SO` and `VN`, `@RG` records (platform `PL`, model
  `PM`, sample `SM`, `DS`), `@PG` records, reference count and total length. Secondary and
  supplementary records (flag `0x900`) are listed but not counted as reads. A `.bai` beside the
  file (`x.bam.bai` or `x.bai`) gives exact mapped and unmapped counts (per-reference bin 37450
  and the trailing no-coordinate count) without reading the records.

## Speed

`info` streams up to 32 MB of the file (compressed bytes for gzip and BGZF). When the file ends
within that, every number is exact; otherwise counts are extrapolated from the bytes read
(`tables[0].extra.read_count_exact` false, `read_count_basis` says how) and the distributions
describe the reads scanned. `check` reads every record and reports the exact counts. Reading
table 0 in order continues one stream, so `table --filter ... --count` over a large file reads
it once.

## What the readers return

- **Table 0, reads** (one row per record): `length`, `mean_quality` (the Phred value of the
  mean error probability, as basecallers report read quality), `gc_percent`, `n_bases`; BAM
  adds `flag`, `mapq`, `ref_id`, `pos` (1-based), `passes` (`np`) and `accuracy` (`rq`).
  `extra.summary`: reads, bases, shortest, longest, mean, median, N50, % bases ≥ Q20 and ≥ Q30,
  median read quality, % GC, % N, reads without qualities; BAM: records, mapped, index counts.
  `extra`: compression, quality encoding, sort order, SAM version, platform, read groups,
  programs, references, and facts from the first read's name (Illumina instrument, run and
  flow cell; a PacBio movie; an Oxford Nanopore UUID).
- **Table 1, quality by position** (mean quality per position, binned to at most 1000 rows),
  **table 2, read lengths** (a histogram: about two bins per square root of the reads, linear
  or logarithmic) and **table 3, read quality** (reads per whole Phred unit of mean quality).
  In aligned BAM, sequences on the reverse strand are stored reverse-complemented, so the
  position profile is in reference orientation.
- **Experiment**: vendor from the platform (`@RG PL`, else the read names), model (`PM`),
  serial (Illumina instrument id), DNA sequencer, DNA sequencing assay, sample (`SM`).
- **Preview**: read-length histogram with the N50 marked, read-quality histogram and quality by
  position.

## Validation

`tests/corpus/seq_oracle/mod.rs` with `oracle/seq_oracle.py`: FASTQ read counts (pysam and
Biopython), bases, shortest and longest read, N50, bases ≥ Q30, GC and sampled reads' length,
mean quality and GC against pysam; BAM records, primary records, mapped primaries, bases, N50
and sampled records' flag, MAPQ, reference, position, length, mean quality, passes and accuracy
against pysam. Development files: Illumina and PacBio HiFi FASTQ, an unaligned PacBio CCS BAM
and a coordinate-sorted Illumina BAM.

## Vocabulary (public API of `openreadout-seq`: `src/reads/`)

| identifier | meaning |
| --- | --- |
| `accuracy` | PacBio read accuracy (`rq`) |
| `add` | Add one read: its bases as letters and its qualities as Phred values (empty when it has none) |
| `bai_counts` | Mapped and unmapped read counts of an indexed BAM, from its `.bai` (the index's per-reference pseudo-bin 37450 and the count of reads without coordinates) |
| `BAM_FORMAT_ID` | Format id of BAM files |
| `BamHeader` | A BAM header |
| `BamReader` | Reader of BAM files |
| `BamRecords` | BAM records |
| `bases` | Bases seen |
| `composition` | A, C, G, T, N and other bases |
| `Counting` | Counts the bytes read from the file (compressed bytes for gzip and BGZF) |
| `FASTQ_FORMAT_ID` | Format id of FASTQ files |
| `FastqReader` | Reader of FASTQ files (`.fastq`, `.fq`, optionally gzip-compressed) |
| `FastqRecords` | FASTQ records |
| `flag` | BAM flag |
| `gc_percent` | GC content in percent of A, C, G and T bases |
| `header` | The header |
| `INFO_SCAN_BYTES` | File bytes `info` reads at most (compressed bytes for gzip and BGZF) |
| `is_extra_alignment` | A secondary or supplementary alignment (not a read of its own) |
| `length` | Length in bases |
| `length_table` | Table of read counts per length bin (about two bins per square root of the reads, 20 to 200, evenly spaced, or logarithmic when lengths span more than two decades) |
| `lengths` | Read length → reads |
| `mapq` | BAM mapping quality |
| `MAX_POSITIONS` | Positions tracked exactly for the per-position quality profile |
| `max_qchar` | Largest quality character seen |
| `MAX_RECORD` | Longest FASTQ line or BAM record accepted (bytes) |
| `mean_quality` | Mean quality: the Phred value of the mean error probability (NaN without qualities) |
| `min_qchar` | Smallest raw quality character (FASTQ) |
| `n50` | The length at which reads of that length or longer hold half the bases |
| `n_count` | N bases |
| `name` | Name (FASTQ title up to the first space, BAM read name) |
| `new` | Records of the stream `r` |
| `no_quality` | Reads without qualities (BAM `*`) |
| `open_stream` | Open `file` as a byte stream, decompressing gzip/BGZF; returns the stream and the counter of file bytes consumed |
| `passes` | PacBio passes (`np`) |
| `pos` | BAM 1-based position |
| `pos_q` | Sum of quality and bases per position (first `MAX_POSITIONS`) |
| `POSITION_ROWS` | Rows of the per-position quality table (positions are binned beyond this) |
| `position_table` | Table of mean quality per position (binned to at most `POSITION_ROWS` rows) |
| `q20` | Bases at Q20 or above |
| `q30` | Bases at Q30 or above |
| `qual` | Qualities as Phred values (empty when the record has none) |
| `qual_bases` | Bases with a quality |
| `quality_table` | Table of reads per mean read quality (whole Phred units) |
| `read_quality` | Mean read quality (rounded down) → reads |
| `ReadFormat` | FASTQ or BAM |
| `reads` | Records seen |
| `ReadsDataset` | An open FASTQ or BAM file |
| `ReadStats` | Accumulated statistics |
| `ReadValues` | Per-read values the reads table keeps |
| `Rec` | One read |
| `Records` | A source of records |
| `ref_id` | BAM reference index (-1 unmapped) |
| `refs` | Reference sequences: name and length |
| `seq` | Bases as letters |
| `summary` | The summary for `info` |
| `text` | The SAM header text |
