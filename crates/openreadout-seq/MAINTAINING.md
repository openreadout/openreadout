# Maintaining `openreadout-seq`

Formats: `illumina-run`, `pod5`, `fast5`, `fastq`, `bam`, `cram`. Start with [docs/maintaining.md](../../docs/maintaining.md) for the project-wide process.

## Decode pipeline

The crate has four reader families that share `table.rs`.

- **Illumina run folders** (`illumina/`). `IlluminaRunReader` accepts the run directory or any file in it and walks up to `RunInfo.xml`. `runinfo.rs` parses `RunInfo.xml` and `RunParameters.xml`. `interop.rs` reads each `InterOp/*MetricsOut.bin` file, branching on the version byte at offset 0 (tile v2/v3, quality v4 to v7, error v3 to v6, extraction v2/v3, corrected intensity v2 to v4, extended tile v1 to v3, index v1/v2), and aggregates records into the run summary, per-cycle and per-tile tables while it reads. `basecalls.rs` finds either per-cycle BCL files or per-surface CBCL files plus filter and locs files, and `fastq.rs` turns them into FASTQ. Base-call counts and the FASTQ export decode only when asked, so `info` stays on headers and InterOp.
- **Nanopore signal** (`nanopore/`). `Pod5Reader` reads the FlatBuffer footer, then each embedded Arrow IPC table through `ipc.rs`. `Fast5Reader` opens the HDF5 file through `openreadout-hdf5` and reads raw chunks itself when the dataset uses the VBZ filter (32020). Both implement `SignalSource`, so `NanoporeDataset` serves reads, signal in pA and run metadata the same way. `vbz.rs` holds both VBZ flavours: POD5 (1 control bit per value) and the FAST5 filter (2 bits per value, with a size prefix).
- **Read files** (`reads/`). `records.rs` streams FASTQ (plain or gzip) and BAM (BGZF) records behind the `Records` trait. `stats.rs` accumulates counts, lengths, qualities and composition. `info` scans the first 32 MiB (`INFO_SCAN_BYTES`) and estimates the total from the bytes left, unless a `.bai` index gives exact counts. `table` continues past that point with a cursor.
- **CRAM** (`cram/`). `CramRecords` implements the same `Records` trait, so CRAM files fill the BAM reads table. `mod.rs` reads the file definition, the SAM header and each container; `block.rs` parses a block and dispatches its compression method (raw, gzip, bzip2 and xz through crates, `rans.rs` for rANS 4x8 and Nx16, `arith.rs` for the arithmetic coder, `fqzcomp.rs`, `tok3.rs`); `encoding.rs` parses the compression header and reads values of a data series from the core bit stream or an external block; `slice.rs` decodes a slice's records, resolves mate flags and builds bases from read features and the reference; `reference.rs` indexes the FASTA found beside the file. Versions branch in the container header (record counter width, CRC32 from 3.0 on) and in which block methods appear.

## Invariants and checks

- Every length, offset and count from a file goes through checked arithmetic and bounds-checked slices. The fuzz targets listed below cover each decoder.
- CRAM bases are restored only from a reference that matches the slice MD5 (external or embedded); otherwise the record keeps its other values and is counted in `reads_without_bases`. The codec decoders are checked against the 197 hts-specs codec test vectors (`tests/cram_codecs.rs`, run with `CRAM_CODEC_VECTORS`).
- InterOp files with an unknown version byte are skipped with a warning rather than failing the whole run, because a run is still useful without one metric file.
- The FASTQ export writes to a temporary file, reads it back and compares record counts before renaming it into place.
- `check` decodes every base-call file of a run, every signal block of a nanopore file and every record of a read file.
- Refused with exit 6: POD5 files written before pod5 0.1 (their run info and pore types are dictionaries of structs), CBCL files with other than 2-bit bases and qualities, filter files other than version 3, and FASTQ export from a run folder without base calls. NextSeq 500 `.bcl.bgzf` files and `.clocs` positions are not recognised as base calls, so such a run opens with its InterOp metrics only. Truncated or inconsistent files give exit 4.

## Debugging a new file

Start with `openreadout info <file> --json` and `openreadout check <file>`. The fixtures in `tests/fixtures/` are small hand-made versions of each layout (BCL and CBCL run folders, FASTQ, BAM, POD5, FAST5), written by `oracle/make_seq_fixtures.py`. To compare with independent readers, `oracle/seq_oracle.py` produces ground truth from interop, Picard, pod5, h5py and pysam. For InterOp, the per-file version byte and record size are the first thing to check against `docs/formats/illumina-run.md`.

## Fragile spots

- Q30 percentages leave out the last cycle of each read, as the interop library does. Changing this breaks agreement with its run summary.
- FASTQ cluster coordinates are computed in single precision, `round(10 * x + 1000)`. Double precision gives off-by-one coordinates on some clusters compared with Picard.
- The instrument model comes from `RunParameters.xml` and is refined by the instrument id prefix. Both are inferred from a few runs.
- The POD5 Arrow decoder supports only the column types POD5 tables use. A new writer that adds another type needs `ipc.rs` extended.
- CRAM fqzcomp: parameter flag 4 marks a fixed read length (decoded with the first record only), the opposite of the CRAMcodecs pseudocode; the codec test vectors pin it.
- CRAM mate flags `0x20`/`0x08` are derived within a slice from the mate downstream, and the last record of a chain takes them from the first. A change there shows in the `flag` column against pysam.
- FAST5 files vary a lot between MinKNOW versions (single-read and multi-read layouts, gzip or VBZ). Only three writers are in the corpus.

<!-- BEGIN GENERATED guide -->
## Facts (generated)

*Generated by `cargo xtask guides --write` from the sources, `corpus/manifest.toml`, `corpus/assurance/evidence.json` and `corpus/intake/`; do not edit. CI fails when it is stale.*

### Formats

| format id | notes and provenance | confidence | basis | development files: read / confirmed | depositors | held-out pass / fail |
| --- | --- | --- | --- | --- | --- | --- |
| `illumina-run` | [format note](../../docs/formats/illumina-run.md), [provenance log](../../docs/provenance/illumina-run.md) | low | vendor docs | 2 / 2 | 1 | - |
| `pod5` | [format note](../../docs/formats/nanopore.md), [provenance log](../../docs/provenance/nanopore.md) | high | vendor docs | 6 / 6 | 3 | - |
| `fast5` | [format note](../../docs/formats/nanopore.md), [provenance log](../../docs/provenance/nanopore.md) | medium | reverse engineered | 3 / 3 | 2 | - |
| `fastq` | [format note](../../docs/formats/sequence-reads.md), [provenance log](../../docs/provenance/sequence-reads.md) | high | open spec | 7 / 7 | 5 | - |
| `bam` | [format note](../../docs/formats/sequence-reads.md), [provenance log](../../docs/provenance/sequence-reads.md) | high | open spec | 11 / 11 | 5 | - |
| `cram` | [format note](../../docs/formats/cram.md), [provenance log](../../docs/provenance/cram.md) | high | open spec | 35 / 34 | 13 | - |

### Source map

| file | what it does (its module documentation) |
| --- | --- |
| [`src/assurance.rs`](src/assurance.rs) | Assurance profiles (`docs/assurance.md`) of the sequencing readers: the variant features of a file and the feature values the development corpus validates |
| [`src/cram/arith.rs`](src/cram/arith.rs) | The adaptive arithmetic (range) coder of CRAM 3.1 (block method 6) and the frequency models it shares with fqzcomp |
| [`src/cram/block.rs`](src/cram/block.rs) | CRAM blocks: the block header (method, content type, content id, sizes, CRC32 from 3.0 on) and the decompression of the block data (CRAM specification §§ 8 and 14) |
| [`src/cram/encoding.rs`](src/cram/encoding.rs) | The compression header (preservation map, data-series encodings, tag encodings) and the encodings that read one value of a data series from the core bit stream or an external block |
| [`src/cram/fqzcomp.rs`](src/cram/fqzcomp.rs) | The fqzcomp quality codec of CRAM 3.1 (block method 7): adaptive models over a 16-bit context of previous qualities, position, running delta and selector, read with the shared |
| [`src/cram/io.rs`](src/cram/io.rs) | Byte and bit cursors over decoded CRAM buffers, and the variable-length integers CRAM uses: ITF8 and LTF8 (CRAM specification § 2.3), uint7 (CRAM codecs § 1.4) |
| [`src/cram/mod.rs`](src/cram/mod.rs) | CRAM (`.cram`): reference-compressed sequence alignments, versions 2.0, 2.1, 3.0 and 3.1 |
| [`src/cram/rans.rs`](src/cram/rans.rs) | rANS entropy decoders: rANS 4x8 (CRAM 3.0, block method 4) and rANS Nx16 with its stripe, run-length and bit-packing transforms (CRAM 3.1, block method 5) |
| [`src/cram/reference.rs`](src/cram/reference.rs) | Reference sequences for CRAM: a FASTA file found beside the CRAM file (or named by a local `@SQ UR:` path), indexed by its `.fai` or by one pass over the file, and read one region… |
| [`src/cram/slice.rs`](src/cram/slice.rs) | Decoding the records of one slice (CRAM specification § 10): positions, names, mate data, tags, read features, mapping quality, qualities and, with the reference, the bases |
| [`src/cram/tok3.rs`](src/cram/tok3.rs) | The read-name tokeniser of CRAM 3.1 (block method 8): names split into typed tokens, each token position and type a byte stream of its own, compressed with rANS Nx16 or the |
| [`src/illumina/basecalls.rs`](src/illumina/basecalls.rs) | Base calls under `Data/Intensities/BaseCalls`: per-cycle BCL files (one per tile, `.bcl` or `.bcl.gz`) or CBCL files (one per lane and surface, every tile a gzip block), the… |
| [`src/illumina/fastq.rs`](src/illumina/fastq.rs) | FASTQ from the base calls of a run folder: one gzip-compressed file per lane and read (`<run>_L001_R1.fastq.gz`, index reads as `I1`, `I2`), records in tile and cluster order, |
| [`src/illumina/interop.rs`](src/illumina/interop.rs) | InterOp binary metrics (`InterOp/*MetricsOut.bin`): tile, quality, error, extraction, corrected-intensity, extended-tile and index metrics, aggregated while they are read |
| [`src/illumina/mod.rs`](src/illumina/mod.rs) | Illumina run folders: the directory an Illumina sequencer writes for one run (`RunInfo.xml`, `RunParameters.xml`, `InterOp/`, `Data/Intensities/BaseCalls/`) |
| [`src/illumina/runinfo.rs`](src/illumina/runinfo.rs) | `RunInfo.xml` (run id, flow cell, instrument, reads and their cycles, flow-cell layout) and `RunParameters.xml` (control software, RTA version, experiment name, reagent kits): the… |
| [`src/lib.rs`](src/lib.rs) | Clean-room readers for DNA sequencer output |
| [`src/nanopore/fast5.rs`](src/nanopore/fast5.rs) | FAST5 (`.fast5`): HDF5 files of nanopore reads |
| [`src/nanopore/ipc.rs`](src/nanopore/ipc.rs) | A small, bounds-checked reader of Apache Arrow IPC files for the column types POD5 tables use: integers, floats, booleans, timestamps, fixed-size and variable-size binary and… |
| [`src/nanopore/mod.rs`](src/nanopore/mod.rs) | Oxford Nanopore raw signal: POD5 (`pod5.rs`) and FAST5 (`fast5.rs`) behind one dataset |
| [`src/nanopore/pod5.rs`](src/nanopore/pod5.rs) | POD5 (`.pod5`): Arrow IPC tables (reads, signal, run info) bundled in a container with a FlatBuffer footer (`docs/formats/nanopore.md`) |
| [`src/nanopore/vbz.rs`](src/nanopore/vbz.rs) | VBZ, the compression of nanopore signal: zstd around variable-byte integers of the zig-zag encoded differences between successive samples |
| [`src/reads/mod.rs`](src/reads/mod.rs) | FASTQ, BAM and CRAM read files: what a sequencer (or its basecaller) hands on |
| [`src/reads/records.rs`](src/reads/records.rs) | Streaming record parsers: FASTQ (plain or gzip, four-line or multi-line records) and BAM (BGZF-compressed binary alignments, SAMv1 § 4) |
| [`src/reads/stats.rs`](src/reads/stats.rs) | Read statistics accumulated while streaming FASTQ or BAM records: counts, bases, lengths (N50), qualities (per read, per position, Q20/Q30 bases) and base composition |
| [`src/table.rs`](src/table.rs) | Small in-memory tables shared by the readers: named columns of numbers (or text, stored as codes into a category list), turned into `TableInfo` for `info` and sliced for… |

### Where variants branch

The assurance profile ([`src/assurance.rs`](src/assurance.rs)) observes these features (each value is looked up in the validated table below; a value never confirmed makes the file `unvalidated` for the feature's outputs) and reports these structures and assumptions:

- feature format_version `format!("CRAM {v}")`
- feature codec `k`
- feature layout `"reference embedded"`
- feature layout `"reference external"`
- feature layout `"reference not required"`
- feature writer `format!("writer {v}")`
- feature format_version `format!("{name} v{ver}")`
- feature instrument `m`
- feature writer_version `format!("RTA {}", major_minor(v))`
- feature codec `s`
- feature format_version `v`
- feature codec `c`
- feature layout `c`
- feature writer `w`
- feature writer `format!("{k} {v}")`
- feature format_version `format!("SAM {v}")`
- undecoded "bases (reference sequence not available)"

### Validated variants and the corpus files that pin them

| format | kind | value | outputs | confirmed files | read | example corpus files |
| --- | --- | --- | --- | --- | --- | --- |
| `bam` | codec | `compression bgzf` | tables | 11 | 11 | `seq-bam-htslib-scramble-range`, `seq-bam-igv-minimap2-long-cigar`, `seq-bam-igv-revio-hg002` |
| `bam` | format_version | `SAM 1.0` | descriptive | 1 | 1 | `seq-bam-igv-tophat-sam10` |
| `bam` | format_version | `SAM 1.4` | descriptive | 2 | 2 | `seq-bam-htslib-scramble-range`, `seq-bam-picard-na12891-markdup` |
| `bam` | format_version | `SAM 1.5` | descriptive | 5 | 5 | `seq-bam-pacbio-alz-ccs`, `seq-bam-pbcore-blasr-aligned`, `seq-bam-pbcore-rs-ccs` |
| `bam` | format_version | `SAM 1.6` | descriptive | 2 | 2 | `seq-bam-igv-revio-hg002`, `seq-bam-illumina-paired-sorted` |
| `bam` | instrument | `platform ILLUMINA` | tables | 4 | 4 | `seq-bam-htslib-scramble-range`, `seq-bam-illumina-paired-sorted`, `seq-bam-picard-na12891-markdup` |
| `bam` | instrument | `platform PACBIO` | tables | 5 | 5 | `seq-bam-igv-revio-hg002`, `seq-bam-pacbio-alz-ccs`, `seq-bam-pbcore-blasr-aligned` |
| `bam` | layout | `sort_order coordinate` | tables | 5 | 5 | `seq-bam-htslib-scramble-range`, `seq-bam-igv-revio-hg002`, `seq-bam-illumina-paired-sorted` |
| `bam` | layout | `sort_order queryname` | tables | 1 | 1 | `seq-bam-picard-unaligned-queryname` |
| `bam` | layout | `sort_order sorted` | tables | 1 | 1 | `seq-bam-igv-tophat-sam10` |
| `bam` | layout | `sort_order unknown` | tables | 3 | 3 | `seq-bam-pacbio-alz-ccs`, `seq-bam-pbcore-rs-ccs`, `seq-bam-pbcore-sequel-subreads` |
| `bam` | writer | `writer TopHat` | descriptive | 1 | 1 | `seq-bam-igv-tophat-sam10` |
| `bam` | writer | `writer bax2bam` | descriptive | 1 | 1 | `seq-bam-pbcore-sequel-subreads` |
| `bam` | writer | `writer bwa` | descriptive | 1 | 1 | `seq-bam-illumina-paired-sorted` |
| `bam` | writer | `writer ccs` | descriptive | 1 | 1 | `seq-bam-pacbio-alz-ccs` |
| `bam` | writer | `writer minimap2` | descriptive | 1 | 1 | `seq-bam-igv-minimap2-long-cigar` |
| `bam` | writer | `writer samtools` | descriptive | 2 | 2 | `seq-bam-igv-revio-hg002`, `seq-bam-picard-na12891-markdup` |
| `bam` | writer | `writer scramble` | descriptive | 1 | 1 | `seq-bam-htslib-scramble-range` |
| `bam` | writer | `writer update_bam.py` | descriptive | 2 | 2 | `seq-bam-pbcore-blasr-aligned`, `seq-bam-pbcore-rs-ccs` |
| `cram` | codec | `arith` | tables | 1 | 1 | `cram-specs-v31-level4` |
| `cram` | codec | `bzip2` | tables | 2 | 3 | `cram-specs-0902-bzip2`, `cram-specs-v31-level4` |
| `cram` | codec | `fqzcomp` | tables | 1 | 1 | `cram-specs-v31-level4` |
| `cram` | codec | `gzip` | tables | 29 | 30 | `cram-adam-artificial`, `cram-disq-valid-v21`, `cram-htslib-auxf-java` |
| `cram` | codec | `lzma` | tables | 2 | 2 | `cram-specs-0903-lzma`, `cram-specs-v31-level4` |
| `cram` | codec | `rans4x8` | tables | 20 | 20 | `cram-htslib-auxf-java`, `cram-htslib-ce5b-java`, `cram-htslib-range` |
| `cram` | codec | `ransNx16` | tables | 4 | 5 | `cram-rusthts-unmapped-v31`, `cram-samtools-chk2-v31`, `cram-specs-v31-level2` |
| `cram` | codec | `raw` | tables | 29 | 30 | `cram-htslib-auxf-java`, `cram-htslib-ce5b-java`, `cram-htslib-range` |
| `cram` | codec | `tok3` | tables | 2 | 2 | `cram-specs-v31-level2`, `cram-specs-v31-level4` |
| `cram` | format_version | `CRAM 2.0` | tables | 1 | 1 | `cram-samtools-ce5b-v20` |
| `cram` | format_version | `CRAM 2.1` | tables | 1 | 1 | `cram-disq-valid-v21` |
| `cram` | format_version | `CRAM 3.0` | tables | 28 | 28 | `cram-adam-artificial`, `cram-htslib-auxf-java`, `cram-htslib-ce5b-java` |
| `cram` | format_version | `CRAM 3.1` | tables | 4 | 5 | `cram-rusthts-unmapped-v31`, `cram-samtools-chk2-v31`, `cram-specs-v31-level2` |
| `cram` | instrument | `platform 10X` | tables | 1 | 1 | `cram-zenodo-ilmn-mt` |
| `cram` | instrument | `platform ILLUMINA` | tables | 7 | 8 | `cram-disq-valid-v21`, `cram-htslib-range`, `cram-jbrowse-volvox-rg` |
| `cram` | instrument | `platform ONT` | tables | 1 | 1 | `cram-nfcore-ont` |
| `cram` | instrument | `platform PACBIO` | tables | 1 | 1 | `cram-zenodo-pacbio-mt` |
| `cram` | layout | `reference embedded` | tables | 5 | 6 | `cram-samtools-chk2-v31`, `cram-specs-0600-embedded`, `cram-specs-v30-level1` |
| `cram` | layout | `reference external` | tables | 26 | 26 | `cram-adam-artificial`, `cram-disq-valid-v21`, `cram-htslib-auxf-java` |
| `cram` | layout | `reference not required` | tables | 8 | 9 | `cram-jbrowse-volvox-noref`, `cram-samtools-chk2-v31`, `cram-samtools-input1a` |
| `cram` | layout | `sort_order coordinate` | tables | 15 | 16 | `cram-adam-artificial`, `cram-disq-valid-v21`, `cram-htslib-range` |
| `cram` | layout | `sort_order unsorted` | tables | 2 | 2 | `cram-htslib-auxf-java`, `cram-rusthts-unmapped-noref` |
| `cram` | writer | `writer Hey!` | descriptive | 1 | 1 | `cram-disq-valid-v21` |
| `cram` | writer | `writer MarkDuplicates` | descriptive | 1 | 1 | `cram-zenodo-ilmn-mt` |
| `cram` | writer | `writer bwa` | descriptive | 1 | 1 | `cram-jbrowse-volvox-long` |
| `cram` | writer | `writer cow` | descriptive | 1 | 1 | `cram-samtools-input1a` |
| `cram` | writer | `writer cramtools` | descriptive | 2 | 2 | `cram-htslib-auxf-java`, `cram-htslib-ce5b-java` |
| `cram` | writer | `writer longranger` | descriptive | 1 | 1 | `cram-zenodo-lariat-mt` |
| `cram` | writer | `writer sambamba-6DEDF075` | descriptive | 1 | 1 | `cram-zenodo-pacbio-mt` |
| `cram` | writer | `writer samtools` | descriptive | 12 | 13 | `cram-igvrep-minigenome`, `cram-jbrowse-volvox-noref`, `cram-jbrowse-volvox-rg` |
| `cram` | writer | `writer scramble` | descriptive | 4 | 4 | `cram-htslib-range`, `cram-specs-v30-level1`, `cram-specs-v31-level2` |
| `fast5` | codec | `gzip` | traces | 2 | 2 | `seq-fast5-multi-zip`, `seq-fast5-single-read` |
| `fast5` | codec | `vbz` | traces | 1 | 1 | `seq-fast5-vbz-reads` |
| `fast5` | format_version | `1.0` | metadata, traces | 1 | 1 | `seq-fast5-multi-zip` |
| `fast5` | format_version | `2` | metadata, traces | 1 | 1 | `seq-fast5-single-read` |
| `fast5` | format_version | `2.0` | metadata, traces | 1 | 1 | `seq-fast5-vbz-reads` |
| `fast5` | layout | `multi-read` | traces | 3 | 3 | `seq-fast5-multi-zip`, `seq-fast5-single-read`, `seq-fast5-vbz-reads` |
| `fast5` | writer | `MinKNOW 3.4.0-rc3` | metadata | 1 | 1 | `seq-fast5-multi-zip` |
| `fast5` | writer | `MinKNOW 3.6.0-rc2` | metadata | 2 | 2 | `seq-fast5-single-read`, `seq-fast5-vbz-reads` |
| `fastq` | codec | `compression gzip` | tables | 4 | 4 | `seq-fastq-fastcat-dorado-tags`, `seq-fastq-illumina-test1`, `seq-fastq-pacbio-hifi` |
| `fastq` | codec | `compression none` | tables | 3 | 3 | `seq-fastq-biopython-454-wrapped`, `seq-fastq-chopper-ont`, `seq-fastq-seqkit-ont-pcs109` |
| `fastq` | sample_layout | `quality_encoding phred+33` | tables | 7 | 7 | `seq-fastq-biopython-454-wrapped`, `seq-fastq-chopper-ont`, `seq-fastq-fastcat-dorado-tags` |
| `fastq` | writer | `title_convention illumina 1.8+` | descriptive | 1 | 1 | `seq-fastq-seqkit-hiseqx` |
| `fastq` | writer | `title_convention nanopore` | descriptive | 1 | 1 | `seq-fastq-chopper-ont` |
| `fastq` | writer | `title_convention nanopore minknow` | descriptive | 1 | 1 | `seq-fastq-seqkit-ont-pcs109` |
| `fastq` | writer | `title_convention nanopore sam tags` | descriptive | 1 | 1 | `seq-fastq-fastcat-dorado-tags` |
| `fastq` | writer | `title_convention sra` | descriptive | 1 | 1 | `seq-fastq-pacbio-hifi` |
| `illumina-run` | codec | `bcl` | tables | 1 | 1 | `seq-nfcore-miseq-k9h97` |
| `illumina-run` | codec | `cbcl` | tables | 1 | 1 | `seq-nfcore-novaseq-hmtfydrxx` |
| `illumina-run` | field | `experiment.instrument.model` | descriptive | 2 | 2 | `seq-nfcore-miseq-k9h97`, `seq-nfcore-novaseq-hmtfydrxx` |
| `illumina-run` | format_version | `CorrectedIntMetricsOut v2` | tables, traces | 1 | 1 | `seq-nfcore-miseq-k9h97` |
| `illumina-run` | format_version | `CorrectedIntMetricsOut v4` | tables, traces | 1 | 1 | `seq-nfcore-novaseq-hmtfydrxx` |
| `illumina-run` | format_version | `ErrorMetricsOut v3` | tables, traces | 1 | 1 | `seq-nfcore-miseq-k9h97` |
| `illumina-run` | format_version | `ExtendedTileMetricsOut v3` | tables, traces | 1 | 1 | `seq-nfcore-novaseq-hmtfydrxx` |
| `illumina-run` | format_version | `ExtractionMetricsOut v2` | tables, traces | 1 | 1 | `seq-nfcore-miseq-k9h97` |
| `illumina-run` | format_version | `ExtractionMetricsOut v3` | tables, traces | 1 | 1 | `seq-nfcore-novaseq-hmtfydrxx` |
| `illumina-run` | format_version | `IndexMetricsOut v2` | tables, traces | 1 | 1 | `seq-nfcore-novaseq-hmtfydrxx` |
| `illumina-run` | format_version | `QMetricsOut v4` | tables, traces | 1 | 1 | `seq-nfcore-miseq-k9h97` |
| `illumina-run` | format_version | `QMetricsOut v7` | tables, traces | 1 | 1 | `seq-nfcore-novaseq-hmtfydrxx` |
| `illumina-run` | format_version | `TileMetricsOut v2` | tables, traces | 1 | 1 | `seq-nfcore-miseq-k9h97` |
| `illumina-run` | format_version | `TileMetricsOut v3` | tables, traces | 1 | 1 | `seq-nfcore-novaseq-hmtfydrxx` |

… 13 more values: the generated table in `src/assurance.rs` has all of them.

### Tests, fixtures, fuzz targets, snapshots

- integration tests: [`tests/cram_codecs.rs`](tests/cram_codecs.rs), [`tests/synthetic.rs`](tests/synthetic.rs)
- committed fixtures: 31 files in [`tests/fixtures/`](tests/fixtures) (malformed ones are replayed through every reader by `openreadout`'s `tests/fuzz_regressions.rs`; all are snapshotted by its `tests/golden.rs`)
- fuzz targets (`fuzz/fuzz_targets/`): `seq_cram_codecs`, `seq_interop`, `seq_vbz`, `whole_bam`, `whole_cram`, `whole_fast5`, `whole_fastq`, `whole_illumina_bcl`, `whole_illumina_cbcl`, `whole_pod5`
- corpus inputs by tier: smoke 58, standard 6
- golden snapshots: [`corpus/snapshots/illumina-run.jsonl`](../../corpus/snapshots/illumina-run.jsonl), [`corpus/snapshots/pod5.jsonl`](../../corpus/snapshots/pod5.jsonl), [`corpus/snapshots/fast5.jsonl`](../../corpus/snapshots/fast5.jsonl), [`corpus/snapshots/fastq.jsonl`](../../corpus/snapshots/fastq.jsonl), [`corpus/snapshots/bam.jsonl`](../../corpus/snapshots/bam.jsonl)

### Open new-variant intakes

None.
<!-- END GENERATED guide -->
