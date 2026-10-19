# Provenance log — CRAM

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + licence), what was inferred.

CRAM is an open specification of the GA4GH / samtools hts-specs project, so the layout comes from the specification documents and the normalized fields carry `Source::Spec`.

## 2026-10-19 — initial reader (Richard Zimring with Claude as assistant)

**Specifications consulted** (hts-specs at commit 510c107, each document stating Apache-2.0 on its title page):
- `CRAMv3.tex` (CRAM 3.0 and 3.1): data types (ITF8, LTF8, bit streams), the file definition, container and block structures with their CRC32, block content types and compression methods, the compression header (preservation map, data-series encodings, tag dictionary and tag encodings), slice headers, the record decode order and pseudocode (positions, names, mate data, tags, read features, mapped and unmapped reads), base substitution matrices, reference MD5 rules, the end-of-file container, and the core encodings (EXTERNAL, HUFFMAN with canonical codes, BYTE_ARRAY_LEN, BYTE_ARRAY_STOP, BETA, SUBEXP, GAMMA, Golomb, Golomb-Rice).
- `CRAMv2.1.tex`: the 2.x container header (ITF8 record counter, no CRC32) and blocks without CRC32.
- `CRAMcodecs.tex`: rANS 4x8 (order 0 and 1, the RLE frequency tables), rANS Nx16 (4 or 32 states, order 0 and 1, stripe, pack, run-length and CAT transforms, the order-1 table compression), the adaptive arithmetic coder (range coder, adaptive models, order 0/1, RLE, stripe, pack, the bzip2 extension), the name tokeniser (token types, the serialised streams) and fqzcomp (parameter blocks, `ReadArray` tables, contexts).

**Codec test vectors:** hts-specs `test/cram/codecs` (Apache-2.0 / BSD): all 197 rANS 4x8, rANS Nx16, arithmetic, fqzcomp and name-tokeniser files decode to their gzip originals (`crates/openreadout-seq/tests/cram_codecs.rs`, run with `CRAM_CODEC_VECTORS`). Inferred from them: in an fqzcomp parameter block, flag 4 marks a fixed read length, decoded with the first record only; the pseudocode in `CRAMcodecs.tex` reads it the other way round, and only the fixed-length reading decodes the vectors.

**Prior art:** pysam 0.24.1 (MIT, bundling htslib 1.24), run as the black-box oracle (`oracle/seq_oracle.py cram`) with the reference FASTA given explicitly and htslib's reference lookups switched off. No reader's source was consulted. Decoders for bzip2 and xz come from the `bzip2-rs` (MIT/Apache-2.0) and `lzma-rs` (MIT) crates; MD5 from `md-5` (MIT/Apache-2.0).

**Corpus files** (GitHub test data at pinned commits unless said otherwise; companions `<id>.fa` are the depositors' reference FASTA files): hts-specs conformance files `cram-specs-0300-unmapped`, `-0500-mapped`, `-0600-embedded`, `-0902-bzip2`, `-0903-lzma`, `-0905-rans1`, `-1000-names`, `-v30-level1`, `-v31-level2`, `-v31-level4` (Apache-2.0); htslib `cram-htslib-range`, `-ce5b-java`, `-auxf-java` (MIT); samtools `cram-samtools-chk2-v31`, `-ce5b-v20`, `-input1a`, `-mpileup1-v31` (MIT); nf-core/test-datasets `cram-nfcore-paired-end`, `-rnaseq`, `-scramble`, `-ont`, `-pacbio-unmapped` (MIT); JBrowse `cram-jbrowse-volvox-sorted`, `-noref`, `-rg`, `-long` (Apache-2.0); rust-htslib `cram-rusthts-unmapped-v31`, `-unmapped-noref` (MIT); ADAM `cram-adam-artificial` (Apache-2.0); igv-reports `cram-igvrep-minigenome` (MIT); Disq `cram-disq-valid-v21` (MIT); igv.js `cram-igvjs-ce5` (MIT, reference from hts-specs); Zenodo records 3582619, 3581178 and 3582131 (CC-BY-4.0) `cram-zenodo-lariat-mt`, `-ilmn-mt`, `-pacbio-mt`, with the nf-core rCRS FASTA as reference.

**Inferred from the corpus:**
- cramtools-written files (`cram-htslib-ce5b-java`, `cram-htslib-auxf-java`) store HUFFMAN encodings with an empty alphabet for series that hold no values.
- Two external blocks of one slice with the same content id, one holding read names and one the embedded reference (`cram-samtools-mpileup1-v31`, samtools `test/cram_size/mpileup.1.cram`): the reference is the block whose MD5 matches the slice. htslib takes the other block, so the file is not compared with pysam (`oracle_skip`).
