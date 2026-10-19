# CRAM

CRAM is the reference-compressed form of SAM/BAM alignments, an open GA4GH standard maintained with samtools. ENA and SRA accept it, and large sequencing projects archive in it. OpenReadout reads CRAM 2.0, 2.1, 3.0 and 3.1 and returns the same reads table and read statistics as for BAM (`docs/formats/sequence-reads.md`). Derived from the CRAM specifications and the CRAM codecs document in hts-specs (`CRAMv3.tex`, `CRAMv2.1.tex`, `CRAMcodecs.tex`, Apache-2.0), with pysam (htslib) run as the reference reader. Provenance: `docs/provenance/cram.md`.

Format id `cram`, extension `.cram`, magic `CRAM` followed by the major and minor version bytes.

## Layout

1. **File definition** (26 bytes): `CRAM`, major and minor version, a 20-byte file id.
2. **Header container**: a container whose first block holds the SAM header text after a 32-bit length. The `@SQ` lines give the reference names, in the order the records' reference ids count them.
3. **Data containers**: a container header (length, reference id, start, span, record count, record counter, base count, block count, landmarks; from 3.0 on a CRC32), then blocks. The first block is the compression header; then each slice is a slice header block followed by its core data block and external data blocks. The end-of-file container (from 2.1 on) holds no records.
4. **Blocks**: method, content type, content id, compressed and raw sizes (ITF8), the data and (from 3.0 on) a CRC32. Methods: raw, gzip, bzip2, lzma (xz stream), rANS 4x8, rANS Nx16, the adaptive arithmetic coder, fqzcomp (qualities) and the name tokeniser (read names). The last five are written from the CRAM codecs document; bzip2 and xz are decoded by the pure-Rust `bzip2-rs` and `lzma-rs` crates.

## Compression header and records

The preservation map says whether read names are stored (`RN`), whether alignment starts are deltas (`AP`), whether the reference is needed (`RR`), the substitution matrix (`SM`) and the tag dictionary (`TD`). The data-series encodings say how each series is stored: EXTERNAL (an external block, ITF8 for integers), HUFFMAN (canonical codes in the core bit stream; a one-symbol alphabet takes no bits), BYTE_ARRAY_LEN, BYTE_ARRAY_STOP, BETA, SUBEXP, GAMMA and the two deprecated Golomb codes. The tag encodings map a tag id and type to an encoding of its value bytes.

Each record is read in the order the specification gives: BAM flags, CRAM flags, reference id (multi-reference slices), read length, alignment start, read group, read name, mate data (stored verbatim when detached, else the distance to the mate downstream in the slice), tags, then for aligned reads the read features, mapping quality and qualities, and for unaligned reads the bases and qualities.

- **Mate flags.** A record with its mate downstream takes the mate's strand and unmapped bits (`0x20`, `0x08`) from that mate; the last record of a chain takes them from the first.
- **Bases.** An aligned read's bases are the reference bases at its position, changed by its read features: substitutions (`X`, through the substitution matrix), stretches and single read bases (`b`, `B`), insertions and soft clips (`I`, `i`, `S`), deletions and reference skips (`D`, `N`). Positions past the end of the reference read as `N`. A record whose sequence is `*` (CRAM flag 8) has no bases and no qualities.
- **Qualities.** Stored as an array when CRAM flag 1 is set; otherwise only the positions that read features give, and a read whose first quality is unset (255) has none, as in BAM.
- `np` and `rq` tags become the PacBio passes and accuracy, as in BAM.

## The reference

Bases need the reference sequence unless the slice embeds it or the container says the reference is not required (the bases are then stored verbatim). The reader looks for a FASTA file beside the CRAM file: the same name with the extension `.fa`, `.fasta` or `.fna` (or the full name followed by it), or a local file that an `@SQ UR:` field names. The FASTA must be uncompressed; its `.fai` index is used when it is there, and otherwise the file is indexed in one pass (lines of one length per sequence, as samtools faidx requires). Sequences are matched by `@SQ` name.

- When a slice records a reference MD5, the reference bases of the slice (external, or the embedded block) must match it. A slice whose reference does not match keeps every other value, and its reads' bases are not restored.
- When the reference is missing or does not match, the reads' GC and N columns are null, `tables[0].extra.summary.reads_without_bases` counts them, `info` says so in `notes` with what to do, `check` reports `reference_missing` or `reference_mismatch`, and the assurance block lists the bases as undecoded. Lengths, qualities, flags, mapping qualities and positions never need the reference.
- Two external blocks of one slice may share a content id (`cram-samtools-mpileup1-v31`). The first serves the data series; the embedded reference is the one whose MD5 matches the slice.

## `info` (`tables[0].extra`)

Everything the BAM reader reports (`docs/formats/sequence-reads.md`), plus:

| key | meaning |
| --- | --- |
| `kind` | `cram_records` |
| `compression` | `cram blocks` |
| `cram_version` | the version in the file definition (`3.1`); also `format_version` |
| `cram_codecs` | blocks per compression method in the scanned containers |
| `reference.source` | the FASTA used, `embedded in the file`, `not found`, or `not needed` |
| `reference.slices_embedded`, `reference.slices_external` | slices whose aligned reads used an embedded or an external reference |
| `reference.containers_reference_not_required` | containers whose compression header says the reference is not needed |
| `reference.slices_checksum_mismatch` | slices whose reference failed the slice MD5 |
| `summary.reads_without_bases` | reads whose bases were not restored |

`sam_version` is the `@HD VN` of the SAM header.

## `check`

`check` decodes every container and block, checks the CRC32 of container headers and blocks (CRAM 3, `crc_mismatch`), and reports `reference_missing` (warning) or `reference_mismatch` (error) as above, with the read counts.

## Not read

CRAM 1.0 and the draft 4.0 are refused (exit 6). Alignments are not interpreted (no CIGAR, pileup or coverage).

## Vocabulary

| identifier | meaning |
| --- | --- |
| `CramReader` | the CRAM `FormatReader` |
| `CRAM_FORMAT_ID` | `cram` |
| `CRAM_MAGIC` | the four magic bytes `CRAM` |
| `CramHeader` | the file definition and SAM header: `major`, `minor`, `sam` (the header text and references), `data_start` (offset of the first data container) |
| `read_header` | read the file definition and the SAM header |
| `find_reference` | the reference FASTA beside a CRAM file, or named by a local `@SQ UR:` |
| `CramRecords` | the records of a CRAM file, slice by slice; `new(file, header, fasta, facts)` |
| `CramFacts` | what decoding found: `containers`, `slices`, `methods` (blocks per compression method), `embedded_reference_slices`, `external_reference_slices`, `reference_not_required`, `reads_without_bases`, `reference_mismatches`, `crc_failures`, `reference_problem` |
| `is_cram` | whether the start of a file is the CRAM magic with a known major version |
| `decode_codec` | decode data with one of the CRAM 3.x entropy codecs (methods 4 to 8), for tests and fuzz targets |
| `Fasta` | an indexed reference FASTA: `path`, `open(fs, path)`, `len_of(name)`, `fetch(name, start, len)` (0-based, upper-cased) |
| `reference` | the module of `Fasta` |
| `cram_version`, `cram_codecs`, `reference`, `source`, `slices_embedded`, `slices_external`, `containers_reference_not_required`, `slices_checksum_mismatch`, `reads_without_bases` | `info` keys (table above) |
| `crc_mismatch`, `reference_missing`, `reference_mismatch` | `check` finding codes |
