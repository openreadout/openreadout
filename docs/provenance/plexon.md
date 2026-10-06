# Provenance log — Plexon PLX and PL2

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

Plexon publishes neither format as an open specification. PLX ("Plexon 1") layout comes from Neo's `PlexonRawIO` (BSD-3), read as documentation, and from the corpus. PL2 structure is derived from hex dumps of corpus files.

## 2026-09-23 — PLX derivation (Richard Zimring with Claude as assistant)

**Prior art consulted** (permissively licensed; documentation and source read, no code copied):
- python-neo 0.14.5, BSD-3-Clause, https://github.com/NeuralEnsemble/python-neo — `neo/rawio/plexonrawio.py` (the copy installed in `oracle/.venv`): the file header (magic, version, comment, timestamp clock frequency, counts of spike, event and continuous channel headers, points per waveform and before threshold, recording date and time, waveform rate, last timestamp, and from version 103 the electrode-count, bits-per-sample and maximum-magnitude fields, from 105 the spike preamplifier gain, from 106 the application names, then three per-channel count arrays), the three fixed-size channel-header records (spike 1020 bytes, event 296, continuous 296) and the fields Neo reads from them (name, channel number, gain, preamplifier gain, sampling rate, enabled flag), the 16-byte data-block header (block type 1 spike, 4 event, 5 continuous; a 40-bit timestamp split into a u16 high part and a u32 low part; channel; unit; waveform count; words per waveform), and the gain formulas per version (continuous: 5000 / (2048 × gain × 1000) for versions 100–101, 5000 / (2048 × gain × preamp gain) for 102, maximum magnitude / (0.5 × 2^bits × gain × preamp gain) from 103; spike waveforms: 3000 / (2048 × gain × 1000) before 103, maximum magnitude / (0.5 × 2^bits × gain × 1000) for 103–104, × spike preamp gain instead of 1000 from 105).
- Neo 0.14.5 is also run as a black-box oracle (`oracle/gen.py`, `plexon` branch).

**Corpus files used** (`corpus/manifest.toml`; NeuralEnsemble ephy_testing_data on G-Node GIN at commit `bf392aa`, database ODbL-1.0, contents CC-BY-SA-4.0): `plexon-file-plexon-1` (version 101, 4 spike channels, no continuous data), `plexon-file-plexon-2` (101, 16 spike, 28 event and 64 continuous headers but no continuous blocks), `plexon-file-plexon-3` (106, 31 spike channels, one continuous channel), and, held out of the fetched tiers (`tier = "hold"`, see the manifest), `plexon-4chdemoplx` (106; Plexon's four-channel tetrode demo recording as re-hosted by ephy_testing_data).

**Method:** a Python walker (`struct`, no Neo) printed the file header, every channel header and every data-block header. Observed:
- Magic `PLEX` (0x58454C50 little-endian) at byte 0; versions 101 and 106 in the corpus; data blocks follow the last continuous channel header (7504 + 1020 × spike + 296 × event + 296 × continuous headers) and tile the file exactly to its end in every corpus file.
- Continuous blocks carry one waveform of 1–120 samples; consecutive blocks of a channel start exactly `samples × clock / rate` ticks after the previous one (no gap in any corpus file). Channels at the same rate can start at different ticks (`plexon-4chdemoplx`: field potentials at tick 4, the auxiliary input at tick 244) and so hold different sample counts.
- Continuous channels whose header exists but that have no blocks (`plexon-file-plexon-2`: 64 headers, 0 blocks) hold no data; Neo leaves them out, and so do we.
- Event blocks put the event value (a strobed word, or 0) in the unit field and carry no samples.

**Our choices (Inferred):**
- Continuous channels on an identical sample grid (rate, and the start tick and length of every gap-free run) form one trace; a jump between blocks of more than half a sample period starts a new sweep. Neo instead groups channels by the alphabetic prefix of their names and ignores block timestamps (every stream starts at 0); on the corpus both groupings coincide.
- Values are in mV (the gain formulas above give millivolts at the electrode; Neo reports no unit).
- Spikes are one table (one row per waveform block, in file order) and events another (one row per event block); timestamps in clock ticks and seconds.

**Oracle defect worked around:** Neo 0.14.5's continuous chunk reader keeps a running sample index per block that is right only while earlier blocks have equal lengths; a final block longer than the room that index leaves (the 86-sample last block of `plexon-file-plexon-3`) comes back truncated and zero-padded (52 of 600,806 samples). The oracle (`plexon()` in `oracle/gen.py`) therefore concatenates Neo's own parsed blocks (Neo's positions and sizes, bytes through Neo's memory map) and records that in `oracle_note`. Every other value comes from Neo's API.

## 2026-09-23 — PL2 derivation from hex dumps (Richard Zimring with Claude as assistant)

**Prior art consulted:** none for the layout. The ephy_testing_data `plexon/README.md` (the corpus's own description of the two PL2 files: channel counts, which channels were enabled and recorded, the one-minute length, the strobe value range) was read.

**Corpus files used:** `plexon-nc16fpspkevt-1m` (CC-BY-SA-4.0, contributed by Nikhil Chandra) and, held (`tier = "hold"`), `plexon-4chdemopl2`. `plexon-4chdemoplx` turned out to be a different recording (different date, first samples not found in the PL2 file), so no PLX/PL2 differential comparison of samples was possible; only the scale (below) could be compared.

**Method:** `xxd`/`strings` dumps and a Python walker (`struct` only) written from the observations below; every conclusion was checked on both files.
- Every structure after the file header is a record: byte 0 a record type, byte 1 a source (device) number, bytes 2–3 a length in 16-bit words; the record occupies 16 + (2 × words rounded up to 16 bytes). Walking the data region record by record lands exactly on the next record header in both files, from the first data record to an end-of-recording record (type 0x59) followed directly by the footer.
- File header (1152 bytes): `PLEXON` at byte 10; u64 file offsets at 0x20 (end of channel headers), 0x28 (first data record), 0x30 (first footer record), 0x38 (footer index); u64 at 0x40 (a start count) and 0x48 (the recording length in clock ticks: 2402348 = 60.06 s at 40 kHz for the one-minute file, repeated in the end-of-recording record); comment at 0xE0; application name at 0x1E0 and version at 0x220; nine u32 calendar fields at 0x230 (second, minute, hour, day, month from 0, year − 1900, weekday, day of year, daylight flag: 2024-09-04 15:11:11 for the one-minute file); f64 clock (40000.0) at 0x258; u32 channel-header counts at 0x260 (total, spike, —, analog, —, digital): 110 = 16 + 48 + 46 and 334 = 64 + 224 + 46.
- Channel headers follow at 0x480: spike records (type 0xD5, 2592 bytes), then analog (0xD4, 512), then digital (0xD6, 368). Common fields: u32 channel number at 4, name at 16 (64 bytes), u32 source, channel, enabled and recording-enabled flags at 0x50–0x5C (the README's four enabled/recording combinations appear on channels 1–4 of each kind in the one-minute file, and only channels with both flags set have data records). Spike and analog: unit text at 0x60 (`Volts`), f64 sampling rate at 0x70, f64 volts per count at 0x78 (6.1035e-7 V in the demo, the same scale the demo PLX file gives in mV). Spike: u32 samples per waveform at 0x80, i32 threshold at 0x84, u32 samples before the threshold at 0x88, u64 spike counts per unit from 0xA0 (they sum to the spikes found in the data records of every channel of both files). Analog: device name at 0xD8, electrode type at 0x118.
- Data records: analog (0x42): u16 sample count at 2 (full records hold 65535 samples in 65536 slots), u16 channel at 4, the count again at 6, u64 timestamp of the first sample at 8, then int16 samples; consecutive records of a channel start exactly count × clock / rate ticks apart (a gap would start a sweep). Spike (0x31): u16 channel at 4, u16 samples per waveform at 6, u32 spike count at 8, then the u64 timestamps, the u16 unit numbers, and the waveforms spike after spike (the mean waveform crosses the header threshold at the pre-threshold sample; the other order gives noise). Digital (0x5A): u16 channel at 4, u16 event count at 6, then u64 timestamps and u16 values (1 for single-bit inputs; the demo's strobed channel carries 0xFF01–0xFFFF, the README's 32513–32767 with bit 15 set). Channel numbers count within the source (the one-minute file's event records are source 9, channels 17–32).
- The footer repeats the device settings and holds an index (per channel: counts, record offsets, first timestamps); the reader does not need it and does not read it.

**Our choices (Inferred):** analog channels on one sample grid form a trace (as for PLX); values in the header's unit (V) = raw × volts per count; spikes and events are tables with a `source` column; timestamps are clock ticks from the recording start.

**Not validated against an independent reader:** no permissively licensed oracle exists. Validation is internal (record walk ends exactly on the footer; per-unit counts in the headers equal the spike records; sample totals and first timestamps agree across the channels of a grid; the README's channel, length and strobe facts hold) plus a fixture test. PL2 support is marked low confidence in the format notes.

## 2026-09-23 — byte-source merge integration

Adapted the existing reader and session discovery to `Input`/`Fs`/`SourceFile`, preserving
its parsing layout and scaling. Evidence: the existing manifest corpus ids for this format
and their committed oracle JSON; source equivalence is checked by `tests/sources.rs`.
Prior art consulted: only this repository (MIT OR Apache-2.0); no external sources or new
format-layout inferences. No corpus files were changed or added.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the PLX/PL2 version and the voltage scaling (applied). The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-26 — PL2 from a second depositor, first independent PL2 oracle (Richard Zimring with Claude as assistant)

**Corpus files used** (Zenodo 11586428, CC0-1.0, "Adaptive processing and perceptual learning in
visual cortical areas V1 and V4"): `zenodo11586428-bisver-170908100-mrg-pl2`,
`zenodo11586428-bisver-170907100-mrg-pl2`, `zenodo11586428-bisver170808100-mrg-pl2` and, as that
file's oracle, the PLX of the same recording (`bisver170808100_mrg.plx`, PLX version 107,
1.3 GB, `role = "oracle-export"`, tier `full`).

**Prior art consulted:** Neo 0.14.5 `plexonrawio.py` (BSD-3), as before; no Plexon software,
SDK or document.

**Observed** (the Python walker of the 2026-09-23 entry, `struct` only):
- These PL2 files were written offline (the depositor's "merged" files: sorted units only): the
  application name and version are empty, the header's footer offset (0x30) and headers-end
  offset (0x20) are 0, the index offset (0x38) points at the last 640 bytes. Walking records
  from the data offset to the end of the file lands exactly on the end of the file.
- Record types: spikes (0x31, up to 1337 spikes each; the u16 word count is meaningless, as
  already noted), events (0x5A), one 0x59 record whose word count covers a 21,424-byte body, then
  per-channel index records 0xDE (spike channels), 0xDF (digital channels) and 0xE0. The 0x59
  record here does **not** hold the recording length at byte 24 (124795 ticks against a header
  duration of 119411219); OmniPlex's 0x59 record is 48 bytes (word count 10).
- Only channels with sorted units have records; the header's per-unit counts are filled for one
  channel only.

**Independent check (first for PL2).** Neo's PlexonRawIO on the paired PLX (its data-block parse;
its `parse_header` refuses this PLX because the continuous channels of one prefix differ in
length): all 20,858 PL2 events (four channels: three single-bit inputs and the strobed word) have
the same timestamps and values as the PLX's; every one of the 354,626 PL2 spike timestamps exists
on the same channel of the PLX, and every PL2 waveform equals the PLX's raw samples at that
timestamp times the PL2 header's volts per count, which equals Neo's PLX waveform gain formula
(version ≥ 105) / 1000 on every channel. The PLX's start/stop event channels (258, 259) have no
records in the PL2.

**Our choices:** the end-of-recording duration is read only from a 0x59 record of OmniPlex's
length (word count 10); in other files the record is walked over and the `duration_mismatch`
check is skipped (no false warning on offline-written files).

## 2026-09-26 (later) — the offline-written PL2 files leave the development corpus

Held-out draw C (merged into main after the entry above) reserved the depositor record those three PL2 files and their paired PLX came from. Per the held-out rule (a record is held out entirely or not at all) they were removed from the development corpus and their oracles deleted; the reading rules inferred from them stay. Before the draw, the paired PLX read by Neo's `PlexonRawIO` matched the PL2 spike and event records of all three files exactly (waveforms, units, event channels and timestamps), so a PL2 oracle that reports no events on an offline-written file (footer offset 0) is walking from the footer, not from the data offset — the oracle's `pl2()` in this branch walks to the end of the file in that case. The PL2 development files are again the two GIN recordings, checked for internal consistency only.

## 2026-10-06 — compact block index and batched trace reads

No change to what is parsed. Corpus file `zenodo11586428-bisver170808100-mrg.plx` (1.3 GB, PLX 107) holds about 31 million continuous blocks of 9 to 11 samples, with spike blocks between them, so `info` kept a 24-byte entry per block (1.1 GB). `BlockList` now stores each block as changes in the offset step, timestamp step and sample count, a few bytes per block, and `read_trace` reads a sweep's stretch of the file once for all channels instead of one read per block. `info`, `check` and CSV exports of this file are byte-identical before and after. No prior art consulted.
