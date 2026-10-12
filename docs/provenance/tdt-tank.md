# Provenance log — Tucker-Davis Technologies (TDT) tank blocks (`.tsq` + `.tev`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-10-12 — first reader (Richard Zimring with Claude as assistant)

**Prior art consulted** (permissively licensed; documentation and source read, no code copied):
- Neo 0.14.5 `neo/rawio/tdtrawio.py` (BSD-3-Clause, https://github.com/NeuralEnsemble/python-neo,
  the copy installed in `oracle/.venv`): a block is a directory of files sharing a stem (`.tsq`
  index, `.tev` data, `.Tbk` store headers, `.Tdx`, and per-channel `.sev` files in newer
  versions); a tank is a directory of blocks. The `.tsq` index is a run of 40-byte records:
  int32 size, int32 event type, four-character store name, uint16 channel, uint16 sort code,
  float64 time stamp in seconds, int64 offset into the data file, int32 data format and float32
  sampling rate. Event type codes (0x101 strobe on, 0x102 strobe off, 0x201 scalar, 0x8101
  stream, 0x8201 snippet, 0x8801 mark, mask 0xFF0F for streams), data format codes (0 float32,
  1 int32, 2 int16, 3 int8, 4 float64), the start and stop marks (store name bytes 1 and 2) and
  the `.Tbk` `NAME=…;TYPE=…;VALUE=…;` store header items.
- Neo is also run as the oracle (`oracle/gen.py`, `tdt()`), a black box.
- The `tdt` package on PyPI (0.7.6) is Tucker-Davis Technologies' own Python library. Its PyPI
  metadata declares no licence, so it was neither read nor run.

**Corpus files used** (NeuralEnsemble ephy_testing_data on G-Node GIN, commit bf392aa,
CC-BY-SA-4.0): `gin-tdt-aep-05` (a tank of two blocks, `Block-1` and `Block-2`);
`gin-tdt-dataset-0-single-block`, `gin-tdt-dataset-1-single-block`
(blocks whose `.tev` and `.sev` files the depositors cut short); `gin-tdt-epocs-with-offsets-1`,
`gin-tdt-epocs-with-offsets-2`, `gin-tdt-epocs-with-payload` (blocks with strobe stores only,
written from real event data by the depositors, as their README says).

**What was inferred from the files:**
- Record 0 of every `.tsq` has event type 0 and holds the `.tsq` file length in its size field;
  record 1 is the start mark and the last record the stop mark. Their time stamps are seconds
  since 1970-01-01 UTC: the start of `gin-tdt-dataset-0-single-block` is 2018-11-23 15:03:02 UTC,
  and its `StoresListing.txt` says 11/23/2018 10:03:03am (US Eastern time).
- The size field counts 4-byte words of the record plus its data: a stream or snippet record's
  data is size × 4 − 40 bytes (`pNeu`: 74 words, 128 int16 samples, matching `NumPoints=128` in
  the `.Tbk`; `LFPs` 42 words, 64 samples). Consecutive `eNeu` snippets in `.tev` are 248 bytes
  apart (size 72), so each holds 62 float32 samples although the `.Tbk` says 64; the reader uses
  the size field (Neo reads `NumPoints` and so takes 2 samples of the next snippet).
- Strobe records (0x101, 0x102) carry no data in `.tev`: their size is 10 words and the offset
  field holds the strobe value as float64 (`Tick`: 0, 1, 2 …; `PAB_` in
  `gin-tdt-epocs-with-payload`: 16, 2064, 0, the codes its README lists).
- A strobe-off store is named like its strobe-on store with `\` for `/` (`s1s\` and `s1s/`), and
  the channel and sort code bytes of its records spell the strobe-on store's name (`s1s/`,
  `PC0/`, `Pu1e`). Pairing the k-th onset with the k-th offset of that store gives the durations
  the README describes (about 1.0 s, 4.0 s and 0.25 s in `gin-tdt-epocs-with-offsets-1`).
- Stream records of one store and channel follow each other in time order, one chunk each; a
  channel's samples are its chunks back to back. Within a store every channel has the same number
  of chunks in all the corpus blocks.
- `gin-tdt-dataset-0-single-block` and `gin-tdt-dataset-1-single-block` index data past the end
  of their `.tev`; samples are read only up to the first chunk that is not wholly in the file.
- The `.sev` files of `gin-tdt-dataset-0-single-block` belong to a store (`Wav5`) with no `.tsq`
  records; they are listed, not read.
- Blocks of a tank are ordered by their start mark; `Block-2` of `gin-tdt-aep-05` starts
  65,136 s after `Block-1`.
