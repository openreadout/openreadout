# Provenance log — Oxford Nanopore POD5 and FAST5

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + licence), what was inferred.

## 2026-10-15 — before the POD5 and FAST5 readers were written (Richard Zimring with Claude as assistant)

**Corpus files used:**

- `seq-pod5-hg002-10reads`: `HG002_PAW70337_giab_10reads.pod5` from nf-core/test-datasets (branch
  `modules`, MIT licence of the repository), a 10-read subset of the ONT open data release
  `giab_2025.01` (R10.4.1, 5 kHz; ONT open data is CC BY 4.0).
- `seq-pod5-multi-fast5-zip-v6`: `test_data/multi_fast5_zip_v6.pod5` of
  nanoporetech/pod5-file-format (MPL-2.0). Its `_v0` sibling (pod5 0.0.16) stores pore,
  calibration and end reason as dictionaries of structs; the reader refuses that layout.
- `seq-fast5-multi-zip`: `test_data/multi_fast5_zip.fast5` of the same repository (multi-read
  FAST5 1.0, gzip-compressed signal).
- `seq-fast5-vbz-reads`: `test/data/vbz_reads/vbz_reads.fast5` of nanoporetech/ont_fast5_api
  (MPL-2.0), multi-read FAST5 2.0 with VBZ-compressed signal.

**Documentation consulted:**

- POD5 format documents of nanoporetech/pod5-file-format (MPL-2.0; documents only, no source
  read): `docs/SPECIFICATION.md` (signature `\x8BPOD\r\n\x1A\n` at both ends, 16-byte section
  markers, Arrow IPC file per table padded to 8 bytes, `FOOTER\0\0`, a FlatBuffer footer with
  `file_identifier`, `software`, `pod5_version` and a list of embedded files (offset, length,
  format, content type: reads table, signal table, read-id index, other index), the footer length
  as a little-endian i64 before the last section marker) and `docs/tables/reads.toml`,
  `signal.toml`, `run_info.toml` (column names, types and meanings: calibration offset and scale
  turn ADC values into pA, `start` is in samples since acquisition start, `signal` lists signal
  table rows, `minknow.vbz` is VBZ-encoded signal and `samples` the count per row).
- vbz_compression README (nanoporetech, MPL-2.0; README only): VBZ is variable-byte integer
  encoding with StreamVByte followed by zstd; HDF5 filter id 32020 with parameters (version,
  integer size, zig-zag flag, zstd level).
- StreamVByte (Daniel Lemire and contributors, Apache-2.0, https://github.com/lemire/streamvbyte),
  README and paper (Lemire, Kurz, Rupp 2017): a control byte per four integers, two bits each for a
  length of 1–4 bytes, control bytes before the data bytes, values little-endian.
- FAST5 layout: the group and attribute names are read from the files themselves (h5py listing).
- Apache Arrow IPC file format and FlatBuffers documentation (Apache-2.0): https://arrow.apache.org/docs/format/Columnar.html,
  https://flatbuffers.dev/internals/.

**Ground-truth readers (run only):** the `pod5` Python package (ONT, MPL-2.0) and h5py (BSD-3)
with `vbz_h5py_plugin` (ONT, MPL-2.0).

**Inferred by running pod5 and h5py as black boxes** (scratch scripts, not committed):

- POD5 signal rows: `pod5.signal_tools.vbz_compress_signal` on chosen int16 arrays, then zstd
  decompression of its output, shows the inner encoding: the difference of each sample from the
  previous one (from 0 for the first), zig-zag encoded, then one control bit per value (least
  significant bit first, eight values per control byte) choosing one byte (0) or two bytes (1,
  little-endian); all control bytes come first, then the data bytes. The row's `samples` gives the
  count. Example: `[0,1,2,3,-1,100,1000,-300,5…]` → control `c0 01 00`, data
  `00 02 02 02 07 ca 08 07 27 0a 62 02 00…`.
- FAST5 VBZ chunks (filter parameters `(1, 2, 1, 1)`): a little-endian u32 with the decoded size
  in bytes, then a zstd frame holding standard StreamVByte (two bits per value) of the zig-zag
  encoded differences. Decoding `vbz_reads.fast5` this way gives h5py's values.
- POD5 layout of the development files: embedded files are the signal table (columns `read_id`
  16-byte UUID, `signal` large binary, `samples` u32), the run-info table and the reads table
  (`read_id`, `signal` list of u64 rows, `read_number`, `start`, `median_before`,
  `num_minknow_events`, `num_samples`, `channel`, `well`, `pore_type`, `calibration_offset`,
  `calibration_scale`, `end_reason`, `end_reason_forced`, `run_info` dictionary, and in newer
  files `open_pore_level`); the footer length sits 32 bytes before the end of the file.
- FAST5 (from `h5ls`-style listings): multi-read files hold `read_<uuid>` groups with
  `Raw/Signal` (int16), `Raw` attributes `read_id`, `read_number`, `start_time` (samples),
  `duration`, `median_before`, `start_mux`; `channel_id` attributes `channel_number`,
  `digitisation`, `offset`, `range`, `sampling_rate`; `context_tags` and `tracking_id` attribute
  sets; `Analyses/Basecall_1D_*/BaseCalled_template/Fastq`. pA = (raw + offset) × range /
  digitisation, which reproduces pod5's conversion of the same reads.

## 2026-10-16 — damaged POD5 row counts

- Input: a fuzzer-made POD5 (`crates/openreadout-seq/tests/fixtures/malformed/file-fuzz-pod5-declared-rows.pod5`). No new prior art.
- A table's row count is what its record batches declare, capped at its longest decoded column. A
  damaged file that declares billions of rows no longer makes the reader allocate and loop over
  them.
