# Provenance log — Open Ephys recordings (binary and legacy formats)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

Open Ephys is open-source acquisition software; its data formats are described on its public
documentation site.

## 2026-09-26 — first reader (Richard Zimring with Claude as assistant)

**Vendor documentation consulted (public web pages):**
- "Binary Format — Open Ephys GUI Docs",
  https://open-ephys.github.io/gui-docs/User-Manual/Data-formats/Binary-format.html: the
  `Record Node` / `experiment<E>` / `recording<R>` directories, `structure.oebin` (JSON: continuous,
  events and spikes entries with folder names, sample rates, channel counts and per-channel
  `bit_volts`/units), `continuous.dat` (interleaved little-endian int16, ch1_samp1 … chN_samp1,
  ch1_samp2 …; × bitVolts gives µV for headstage channels and V for ADC channels),
  `sample_numbers.npy` (int64 per sample; `timestamps.npy` before GUI 0.6) and `timestamps.npy`
  (float64 seconds; `synchronized_timestamps.npy` before 0.6), TTL events (`states.npy` int16,
  `sample_numbers.npy`, `timestamps.npy`, `full_words.npy`), text events (`text.npy`).
- "Open Ephys Format — Open Ephys GUI Docs",
  https://open-ephys.github.io/gui-docs/User-Manual/Data-formats/Open-Ephys-format.html: the legacy
  per-channel `.continuous` files (1024-byte text header of `header.<key> = <value>;` lines:
  format, version, header_bytes, description, date_created, channel, channelType, sampleRate,
  blockLength, bufferSize, bitVolts), 2070-byte records of 1024 samples with a timestamp, a
  sample count, a recording number and a 10-byte marker, `.events` and `.spikes` files, `_2`,
  `_3` … suffixes for later experiments.
- The NumPy `.npy` format (NumPy documentation, BSD-3): magic `\x93NUMPY`, version, header
  length, a Python-literal dict with `descr`, `fortran_order`, `shape`.

**Prior art consulted** (permissively licensed; documentation and source read, no code copied):
- python-neo 0.14.5, BSD-3-Clause — `neo/rawio/openephysbinaryrawio.py` and
  `neo/rawio/openephysrawio.py`: how `structure.oebin` entries map to folders
  (`folder_name`, `sample_rate`, `channels[].channel_name`, `bit_volts`, `units`), that an empty
  unit means µV for neural and V for `ADC` channels, the npy file names Neo looks for in event
  folders, the legacy record dtype (int64 timestamp, u16 sample count, u16 recording number,
  1024 big-endian int16 samples, 10 marker bytes), the legacy event record (int64 timestamp,
  int16 sample position, u8 event type, processor id, event id, channel id, u16 recording number)
  and how later experiments are numbered by file-name suffix.
- Neo 0.14.5 is also run as the black-box oracle (`oracle/gen.py`, `openephys()`).

**Corpus files used** (ephy_testing_data on G-Node GIN at commit `bf392aa`, CC-BY-SA-4.0):
`openephysbinary/` (eleven recordings, GUI 0.4.4.1, 0.4.5, 0.5.0, 0.5.3, 0.6.0, 0.6.7, 1.0.1:
single and multiple Record Nodes, several experiments and recordings, Neuropixels with and
without SYNC channels, OneBox ADC streams, ONIX, missing stream folders) and `openephys/` (four
legacy directories: `OpenEphys_SampleData_1`, `_2 (multiple starts)`, `_3`,
`openephys_rhythmdata_test_nodes`).

**Observed** (Python walkers written from the above):
- `structure.oebin` channels carry `bit_volts` and `units`; units are empty in GUI 0.6 files and
  `uV` (even on ADC channels) in 0.4/0.5 files; ONIX writes `u` and `%`.
- `continuous.dat` sizes are whole multiples of 2 × channels in every recording.
- Before GUI 0.6 `timestamps.npy` holds int64 sample numbers; from 0.6 `sample_numbers.npy` holds
  them and `timestamps.npy` float64 seconds; 0.5 adds `synchronized_timestamps.npy`.
- TTL event folders: `states.npy` (±line) from 0.6; `channel_states.npy` + `channels.npy` before.
- Legacy: every record is 2070 bytes with the marker; `OpenEphys_SampleData_2` holds recording
  numbers 0 and 1 in one file with a 93,920-sample gap; `OpenEphys_SampleData_3/100_CH32*` has
  timestamps that step by 895–1205 samples per 1024-sample record and a partial trailing record
  (Neo refuses the whole directory for it).

**Our choices (Inferred):** binary streams → traces with recordings as sweeps (Neo: blocks and
segments); legacy runs split at gaps and recording-number changes (Neo zero-fills gaps);
irregular legacy channel files are refused per file; empty binary units follow the Open Ephys
docs (µV headstage, V ADC/analog input), unknown otherwise.

## 2026-09-26 — a second depositor (no parsing change)

**Corpus files:** `zenodo8343581-openephys` (Zenodo 8343581, CC-BY-4.0, Barth et al., CA1 recordings in the Open Ephys format: 100_CH1, 100_CH2, 100_ADC1 of the 81 channel files, with `all_channels.events`, `messages.events`, `settings.xml`, `Continuous_Data.openephys`; 30.4 M samples per channel at 20 kHz). No held-out file.
**Prior art:** unchanged (Neo, BSD-3, as the oracle and second opinion). The reader was not changed: a channel subset of a recording is read like the full set. The corpus test now reads long sweeps page by page (the reader caps one read at `MAX_OE_READ` samples), and the oracle hashes the first 2^24 samples of longer channels (`hashed_samples`) while the sample count is checked over the whole channel.

## 2026-09-26 (later) — directory detection limited to the recording's own folders

**Corpus files:** the whole development corpus (the `corpus_index` test). **What was found.** The first rule claimed any directory with a `structure.oebin` up to four levels down, so `index` of the corpus root read the entire share as one Open Ephys recording. A directory is now claimed only when all its entries belong to an Open Ephys recording (see the format note); no reading rule changed.
