# Provenance log — FASTQ and BAM read files

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + licence), what was inferred.

Both formats are open and specified. The readers summarise them (counts, lengths, qualities and
header facts); they are not alignment tools.

## 2026-10-15 — before the FASTQ and BAM readers were written (Richard Zimring with Claude as assistant)

**Specifications consulted:**

- Cock, Fields, Goto, Heuer, Rice, "The Sanger FASTQ file format for sequences with quality scores,
  and the Solexa/Illumina FASTQ variants", Nucleic Acids Research 38(6):1767–1771 (2010), open
  access: four-line records (`@` title, sequence, `+` optional repeat of the title, quality),
  Phred+33 (Sanger, Illumina 1.8+) and Phred+64 (Illumina 1.3–1.7) qualities, multi-line records.
- Sequence Alignment/Map Format Specification (SAMv1, The SAM/BAM Format Specification Working
  Group, https://samtools.github.io/hts-specs/SAMv1.pdf, version of 2024): BGZF blocks (gzip
  members with a `BC` extra field giving the block size, the empty end-of-file block), the BAM
  header (`BAM\1`, header text, reference list), alignment records (block size, refID, pos,
  l_read_name, mapq, bin, n_cigar_op, flag, l_seq, next refID/pos, tlen, read name, CIGAR, 4-bit
  sequence `=ACMGRSVTWYHKDBN`, qualities, auxiliary fields with types `AcCsSiIfZHB`), FLAG bits,
  and header record types `@HD @SQ @RG @PG @CO`.
- PacBio BAM format specification (Pacific Biosciences, public documentation,
  https://pacbiofileformats.readthedocs.io/en/latest/BAM.html): `@RG` `PL:PACBIO` and `DS`
  key=value pairs (`READTYPE=CCS`, `BINDINGKIT`, `SEQUENCINGKIT`, `BASECALLERVERSION`,
  `FRAMERATEHZ`), per-read tags `np` (passes), `rq` (read accuracy), `zm` (ZMW).

**Corpus files used** (nf-core/test-datasets, branch `modules`, MIT licence of the repository):
`seq-fastq-illumina-test1` (`homo_sapiens/illumina/fastq/test_1.fastq.gz`),
`seq-fastq-pacbio-hifi` (`homo_sapiens/pacbio/fastq/test_hifi.fastq.gz`),
`seq-bam-pacbio-alz-ccs` (`homo_sapiens/pacbio/bam/alz.ccs.bam`, unaligned CCS reads),
`seq-bam-illumina-paired-sorted` (`homo_sapiens/illumina/bam/test.paired_end.sorted.bam`,
coordinate-sorted alignments).

**Ground-truth readers (run only):** pysam (MIT, wrapping htslib, MIT/BSD) for BAM and FASTQ
counts, lengths and qualities; Biopython (BSD-3) `SeqIO` as a second reading of FASTQ.

**Inferred:** nothing beyond the specifications was needed; the files follow them. Illumina
FASTQ titles of the development file are of the bcl2fastq form
`<instrument>:<run>:<flowcell>:<lane>:<tile>:<x>:<y> <read>:<filtered>:<control>:<index>`.
