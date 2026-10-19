# Provenance log — BrainVision Core Data Format (`.vhdr`, `.vmrk`, `.eeg`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-10-19 — first reader (Richard Zimring with Claude as assistant)

**Prior art consulted** (permissively licensed; documentation and source read, no code copied):
- Neo 0.14.5 `neo/rawio/brainvisionrawio.py` (BSD-3-Clause, https://github.com/NeuralEnsemble/python-neo,
  the copy installed in `oracle/.venv`): the header's `[Common Infos]` keys (`DataFile`,
  `MarkerFile`, `DataFormat`, `DataOrientation`, `NumberOfChannels`, `SamplingInterval` in µs),
  `[Binary Infos] BinaryFormat` (`INT_16`, `INT_32`, `IEEE_FLOAT_32`), `[Channel Infos]
  Ch<n>=name,reference,resolution,unit` with `µV` as the default unit, `[Coordinates]`, and
  `[Marker Infos] Mk<n>=type,description,position,size,channel`; the data file next to the header
  under the header's own stem when the named one is missing.
- MNE-Python 1.13.2 `mne/io/brainvision/brainvision.py` (BSD-3-Clause,
  https://github.com/mne-tools/mne-python): the identification lines of version 1 and 2 headers
  and marker files, the `Codepage` (UTF-8, or ANSI as Windows-1252), `[Comment]` as free text
  after the keys, `VECTORIZED` data stored channel by channel with `DataPoints` samples each,
  `ASCII` data with `[ASCII Infos]` (`DecimalSymbol`, `SkipLines`, `SkipColumns`), `\1` for a
  comma in names, an empty resolution read as 1, marker positions counting from 1, and the
  `New Segment` marker's `YYYYMMDDhhmmssuuuuuu` date as the recording start.
- pybv 0.8.1 `pybv/io.py` (BSD-3-Clause, https://github.com/bids-standard/pybv): the writer
  divides every format's values, `IEEE_FLOAT_32` included, by the channel's resolution, so the
  reader multiplies every format by it, as MNE does. (Neo leaves float values unscaled; the files
  here with float data and a resolution other than 1 decide between the two.)
- MNE-Python and Neo are also run as the oracle (`oracle/gen.py`, `brainvision()`), as black boxes.

**Design choices:**
- Value = stored sample × resolution, in the channel's unit; one trace whose channels are the
  header's.
- `SegmentationType=MARKERBASED` with `SegmentDataPoints` that divides the data evenly: each
  segment is a sweep (the data of such files is a run of equal epochs).
- The markers form a table with onsets in seconds after the first sample (position − 1 divided by
  the sample rate).

**Corpus files used** (13 development recordings from 8 sources): `gin-brainvision-*`
(NeuralEnsemble ephy_testing_data on G-Node GIN, commit bf392aa, CC-BY-SA-4.0; BrainVision
Analyzer exports in INT_16, INT_32 and IEEE_FLOAT_32, and a FieldTrip export); OpenNeuro
ds003190, ds005262, ds005530, ds007541 and ds007720 (CC0; BrainVision Recorder and pybv 0.7.5);
Zenodo record 8407716 (CC-BY-4.0, VECTORIZED); MNE-Python's `test.vhdr` (BSD-3-Clause).

**What was inferred from the files and the reference readers' output:**
- The three pybv files (OpenNeuro ds005262, ds007541, ds007720) store IEEE_FLOAT_32 samples with
  resolution 0.1. MNE multiplies them by 0.1, Neo does not, and pybv's writer divides by the
  resolution before it writes; the reader multiplies, and matches MNE on every file.
- The GIN Analyzer exports are `MARKERBASED` with `SegmentDataPoints` 294 (9,996 samples, 34
  segments) and 200 (8,000 samples, 40 segments), each segment opened by a `New Segment` marker;
  they are sweeps.
- MNE gives the unit `n/a` for channels whose unit it does not map (`BS`, `ARU`, `uS` in
  `mnepython-bv-test`), and Neo gives `u` when a channel line has no unit field; the oracle takes
  Neo's unit where it read one, and the reader keeps the unit as written, `µV` when absent.
- `SamplingInterval` is written as `3.906250e+03` in ds003190; it parses as a float.
