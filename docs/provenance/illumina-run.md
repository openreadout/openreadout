# Provenance log — Illumina run folders (RunInfo.xml, RunParameters.xml, InterOp, BCL/CBCL)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + licence), what was inferred.

## 2026-10-15 — before the run-folder reader was written (Richard Zimring with Claude as assistant)

**Corpus files used:**

- `seq-nfcore-miseq-k9h97`: MiSeq run `220422_M11111_0222_000000000-K9H97` from nf-core/test-datasets
  (branch `demultiplex`, `testdata/MiSeq/`, MIT licence of the repository). One lane, one 110-cycle
  read, InterOp for 19 tiles × 2 surfaces, BCL files trimmed to tile 1101.
- `seq-nfcore-novaseq-hmtfydrxx`: NovaSeq 6000 run `200624_A00834_0183_BHMTFYDRXX` from the same
  repository (`testdata/NovaSeq6000/`). Two lanes, reads 36 + 10 + 10 cycles, RTA 3.4.4, CBCL files
  trimmed to tile 1_2101, by-cycle InterOp directories (`InterOp/C<n>.1/`).

**Documentation consulted:**

- Illumina InterOp binary-format pages (documentation pages of the GPL-3.0 InterOp project; the
  source code was not read): https://illumina.github.io/interop/binary_formats.html and the pages
  it links for tile v2/v3, Q v4–v7, error v3–v6, extraction v2/v3, corrected intensity v2–v4 and
  extended tile v1–v3 (fetched 2026-10-15). Taken from them: every file starts with a version byte
  and a record-size byte; some versions add an extended header (tile v3: a float32 tile area; Q
  v5/v6: a has-bins flag, a bin count and the low, high and value arrays; Q v7: bin triples;
  extraction v3: a channel count; error v6: adapter count and bases); records start with lane
  (u16), tile (u16, u32 from tile v3 / Q v7 / error v4 / extraction v3 / corrected v4 on) and,
  for by-cycle metrics, cycle (u16). The record fields and their order are as those pages list.
- illuminate (Naomi Most and contributors, MIT, https://github.com/nthmost/illuminate),
  `illuminate/tile_metrics.py`, read as documentation for the tile-metric v2 codes: 100 cluster
  density (k/mm²), 101 PF cluster density, 102 cluster count, 103 PF cluster count,
  200 + 2(N−1) phasing and 201 + 2(N−1) prephasing of read N, 300 + N − 1 percent aligned of read
  N, 400 control lane. The file cites Illumina's RTA theory-of-operations technical note.
- Picard (Broad Institute, MIT, https://github.com/broadinstitute/picard), read as documentation:
  `CbclReader.java` (CBCL header: u16 version, u32 header size, bits per base call, bits per
  quality score, u32 bin count and (from, to) u32 pairs, u32 tile count, per tile u32 tile number,
  u32 clusters, u32 uncompressed and u32 compressed block size, then a non-PF-excluded flag byte;
  the tile blocks are gzip members in tile order; with 2+2 bits a byte holds two clusters, low
  nibble first, base in bits 0–1 (A, C, G, T) and the quality bin index in bits 2–3),
  `BaseBclReader.java` (BCL byte: base in bits 0–1, quality in bits 2–7, a zero byte is a no-call),
  `FilterFileReader.java` (filter header: u32 0, u32 version 3, u32 cluster count; one byte per
  cluster, bit 0 = passed filter), `LocsFileReader.java` (u32 1, float32 1.0, u32 count, then
  float32 x, y per cluster). Picard is also run as a black box to make the FASTQ ground truth.

**Ground-truth readers (run only):** the `interop` Python package (Illumina, GPL-3.0) for the run
summary; Picard `IlluminaBasecallsToFastq` (MIT) for FASTQ from the base calls.

**Inferred from the files** (scratch scripts, not committed):

- MiSeq InterOp versions: tile v2, Q v4, error v3, extraction v2, corrected intensity v2, control
  v1 (not decoded); `IndexMetricsOut.bin` is empty. NovaSeq: tile v3 (area 1.38 mm² in the
  extended header; records `t` = cluster count and PF count, `r` = read and percent aligned),
  Q v7 with three bins written as (low, high, value) triples (2–17 → 11, 18–29 → 25, 30–40 → 37),
  `QMetricsByLaneOut.bin` v6 with the same bins as three arrays, error metrics absent, extraction
  v3 with two channels, corrected intensity v4, extended tile v3. By-cycle copies in
  `InterOp/C<n>.1/` repeat the per-cycle files.
- CBCL of the NovaSeq run: header 65 bytes, 2 bits per base and per quality, bins
  0 → 0, 1 → 11, 2 → 25, 3 → 37, one tile per file (`L001_2.cbcl` = lane 1, surface 2), non-PF
  clusters included (flag 0), 4,091,904 clusters = the filter file's count = `s.locs` count (one
  locations file per flow cell on patterned flow cells). MiSeq BCL: u32 cluster count 673,257,
  one byte per cluster; filter v3 with the same count, `L001/s_1_1101.locs` with x, y in pixels.
