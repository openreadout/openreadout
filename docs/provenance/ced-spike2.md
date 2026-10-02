# Provenance log — CED Spike2 `.smr`

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

The layout comes from Neo's `Spike2RawIO` (BSD-3), read as documentation, and from the corpus.

## 2026-09-26 — first reader (Richard Zimring with Claude as assistant)

**Prior art consulted** (permissively licensed; documentation and source read, no code copied):
- python-neo 0.14.5, BSD-3-Clause, https://github.com/NeuralEnsemble/python-neo —
  `neo/rawio/spike2rawio.py` (the copy installed in `oracle/.venv`): the 512-byte file header
  (system id, time units per tick `us_per_time`, `time_per_adc`, first data block, channel count,
  `dtime_base`, five 80-byte comments), the 140-byte channel headers at 512 + 140 × index (block
  chain start/end, block count, extra bytes per item, pre-trigger points, comment, divider
  `l_chan_dvd`, physical channel, title, ideal rate, kind; for waveform kinds scale, offset and
  unit, then `divide` (system id < 6) or `interleave`), the channel kinds (0 empty, 1 Adc int16,
  2/3/4 falling/rising/both-edge events, 5 marker, 6 AdcMark, 7 RealMark, 8 TextMark, 9 RealWave
  float32), the 20-byte block header (previous and next block, first and last time, channel,
  item count), item layouts per kind, block offsets counted in 512-byte units for system id 9,
  the sample interval (divide × `us_per_time` × `time_per_adc` µs before system id 6;
  `l_chan_dvd` × `us_per_time` × `dtime_base` from 6), int16 scaling (scale / 6553.6, then
  + offset), and pauses (a block that starts more than one interval after the previous block's
  last sample).
- Neo 0.14.5 is also run as the black-box oracle (`oracle/gen.py`, `spike2()`).

**Corpus files used** (`corpus/manifest.toml`): Zenodo 4985334 (`zenodo4985334-aa-mvc`, CC0),
20750122 (`zenodo20750122-sub3-transpai`), 15783208 (`zenodo15783208-cpp600-c7e-da`), 4437568
(`zenodo4437568-fig1-rec-f-control`), 10624872 (`zenodo10624872-21022013`), all CC-BY-4.0; and
the ephy_testing_data `spike2/` files (CC-BY-SA-4.0): `File_spike2_1/2/3.smr`,
`multi_sampling.smr`, `130322-1LY.smr`, `Two-mice-bigfile-test000.smr`, `m365_1sec.smrx`.

**Observed** (a Python walker written from Neo's layout, `struct` only):
- System ids 3, 4, 5, 6, 7 and 9 in the corpus; system id 9 (`Two-mice-bigfile`) counts block
  offsets in 512-byte units (`first_data` 10 → byte 5120).
- Every channel's block chain starts with a previous-block of −1, follows next-block offsets, ends
  at the header's last block with a next-block of −1 after exactly the header's block count.
  Block headers name the channel as index + 1 (all but 8 blocks of `File_spike2_1`, which belong
  to a derived channel).
- Waveform blocks satisfy last time = first time + (items − 1) × interval in every block, so a
  block is contiguous with the previous one when it starts exactly one interval after the
  previous last time; `multi_sampling` has 9 pauses on each waveform channel, the other files none.
- Bytes 52–59 of the file header: hundredths of a second, second, minute, hour, day, month (u8
  each) and the year (u16): `zenodo15783208-cpp600-c7e-da` (file name `…20220610…`) gives
  2022-06-10 10:29:23.17; the other files give plausible dates, `File_spike2_1` (system id 4)
  zeros. Neo reads only one byte there and ignores the date below system id 6.
- Channel titles, units and comments are Pascal strings (a length byte, then the text).

**Our choices (Inferred):**
- Each waveform channel (Adc, RealWave) is its own trace; pauses split sweeps; a channel's sweeps
  start at its blocks' first times. Values: int16 × scale / 6553.6 + offset, float32 as stored,
  in the channel's unit.
- Events, markers and text marks form one `events` table; AdcMark/RealMark waveforms one `spikes`
  table (the first marker byte is the unit), in channel order and file order.
- `recorded_at` from bytes 52–59 when they form a valid date (Inferred).
- `.smrx` (64-bit SON) files are recognized and refused: no reader the clean-room rules allow
  exists to derive or check them.

## 2026-09-26 — 64-bit `.smrx` files read (Richard Zimring with Claude as assistant)

The 2026-09-26 decision above (refuse `.smrx`) was taken because no permissive reader exists.
The layout below was instead derived **from the files alone**, checked against the Spike2
software's own exports of the same recordings (text and MATLAB files the depositors published
beside them). No CED documentation, SON64 library source or header, `sonpy` binary, Neo's use of
it, or any other reader of `.smrx` was consulted or run.

**Corpus files used:** `spike2-m365-1sec-smrx` (ephy_testing_data, 16 × 30 kHz channels, 1 s);
figshare 10.25377/sussex.26177569 (CC-BY-4.0, SSSort 2.0 validation data): `SSSort_singleA`,
`SSSort_singleB`, `SSSort_doubleAB` `.smrx` with the Spike2 9.12 text exports of all their
channels (`.txt`, 10 kHz, 5 decimals) and the 32-bit `.smr` exports of channels 5–9
(`SSSort_*_asym0N.smr`, read by this project's `.smr` reader); figshare 25112837 (CC-BY-4.0,
"Pan-cortical 2-photon mesoscopic imaging…", Figure 2): `Fig_2_2367_200303_E210_aud_0.smrx` with
its Spike2 MATLAB export (`.mat`, HDF5: per channel `values`, `start`, `interval`, `scale`,
`offset`, `times`, `title`, `units`, `comment`).

**Observed** (Python `struct` walkers; offsets in bytes, little-endian):
- File header: `S64pl\0` at 0; two version bytes at 6–7 (`01 01`, `00 01`); an 8-character
  creator code at 16 (`S2091349`, `S2083226`, empty in imported files); the recording date at 24
  (hundredths, second, minute, hour, day, month as u8, year as u16: `Fig_2…aud_0` gives
  2020-03-03 11:32:57.69, its MATLAB export was written on 2020-03-04); f64 seconds per tick at
  32 (1e-6, 2e-6, 3.7e-6 — each file's channel intervals and event times in its exports are
  multiples of it); u32 at 44 the channel-table offset (2048), 52 the string-table offset, 56 the
  number of channel slots (32, 100, 399), 60 the channel-record length (272); u32 file-comment
  string indices at 64–83 (the SSSort files: the import note and the source path); at 996 the
  number of extra header blocks, 1000 the file length (u64), 1016 the largest time (u64 ticks),
  1024 the offsets of the extra header blocks (u64 each).
- The header is a byte stream: the first 64 KiB of the file, then each extra header block minus
  its 16-byte block header (channel field 0xFFFF). With 399 channel slots the channel table runs
  past 64 KiB and the string table's stated offset 110576 lands 16 bytes before where its bytes
  are in the file; mapping through the stream puts it exactly on them.
- String table: u32 byte count, u32 string count, then per string a u32 reference count and
  NUL-terminated Latin-1 text padded to 4 bytes. Channel records refer to strings 1-based (0 =
  none): the titles, units and comments this gives equal every `title`/`units`/`comment` of the
  MATLAB export and every column name of the text exports.
- Channel record (272 bytes): u64 root index block, u64 last time, u64 live data blocks, u64
  blocks left from a deleted channel, u32 item bytes (2 Adc, 4 RealWave, 8 events, 16 markers, 16
  + n for marker kinds with data), u16 marker points, u16 marker traces, u16 pre-trigger points,
  u16 generation (increments when the slot is re-created), u8 kind (0 deleted, 1 Adc, 3 rising
  events, 5 markers, …; the byte after it keeps a deleted channel's kind), i32 physical port (−1
  when imported), u32 title/units/comment string indices, u64 sample interval in ticks, f64 ideal
  rate, f64 scale, f64 offset, f64 y-range. Adc values are int16 × scale / 6553.6 + offset: equal
  (all 3,152,721 samples, bit for bit) to the MATLAB export's `values`, whose `scale` is
  scale / 6553.6 and `interval` the ticks × tick.
- Blocks: 4 KiB index blocks and 64 KiB data blocks. Every block starts with a u64 whose top
  bits are the parent block's offset and whose low 12 bits are the level (bits 8–11: 0 data, 1
  index of data blocks, 2 index of index blocks) and the position in the parent (bits 0–7), then
  u16 channel (0-based), u16 generation, u32 count. Index entries: (u64 first time, u64 block
  offset), at most 255 per block; `Fig_2…aud_0`'s 33 kHz channel has 321 data blocks under a
  two-level index. Waveform data blocks hold `count` runs of (u64 first time, u64 samples,
  samples); event blocks hold `count` u64 times; marker items are a u64 time then the marker
  bytes and the item's data.
- Deleted and re-created channels leave blocks of the old generation in the slot's index: the
  SSSort event channels list 9 old waveform blocks after their live block. Taking only blocks
  whose channel and generation equal the record's, up to its live-block count, reproduces every
  exported channel.

**Checks** (all channels of all four files): the SSSort text exports agree with every waveform
sample within one unit of their fifth decimal and put each event in the exported sample bin
(210 + 672, 490 + 932, 354 + 628 events); the 32-bit `.smr` export `SSSort_singleB_asym03.smr`
holds the same 288,000 int16 samples as channel 5 (read with Neo); the MATLAB export of
`Fig_2…aud_0` equals 9 waveform channels bit for bit (incl. the 10.5-million-sample channel under
the two-level index) and the 15,764 frame-trigger times exactly — its `walk` and `Pupil` values are
channel-processed in Spike2 (a fit against the stored int16 leaves residuals of 0.76 and 4e-4) and
are compared for length, start and interval only.

**Not observed** (no public file): RealWave, falling/level events, AdcMark/RealMark/TextMark
items in a live channel (deleted AdcMark and TextMark channels in the SSSort files show the item
layout: 16 + 24 × 2 and 16 + 88 bytes), three-level indexes, multiple runs in one waveform block.
These are decoded by the same rules and reported as unvalidated by the assurance profile.
