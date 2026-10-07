# Provenance log — Blackrock Neurotech NSx and NEV

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

Blackrock publishes the NEV/NSx file specification. The structure of spec 2.2–3.0 files therefore comes from the vendor's public document (`Source::VendorImpl`); spec 2.1 NSx files (`NEURALSG`) are not covered by any public Blackrock document we found and come from Neo (BSD-3) read as documentation plus the corpus.

## 2026-09-22 — initial derivation (Richard Zimring with Claude as assistant)

**Vendor documentation consulted** (public downloads, no login, retrieved 2026-09-22; no license stated, used as documentation only):
- Blackrock LB-0023 Rev 7.00, "Specification for the NEV and NSx file formats (FileSpec 3.0)", support-portal attachment https://support.blackrockneurotech.com/ (article 49959000005305587), also indexed at http://www.blackrockmicro.com/wp-content/ifu/LB-0023-7.00_NEV_File_Format.pdf — NEV basic header (`NEURALEV`/`BREVENTS`, version, flags, bytes in headers, bytes per packet, timestamp and sample resolution, UTC SYSTEMTIME origin, application, comment, extended-header count), extended headers (`NEUEVWAV`, `NEUEVLBL`, `NEUEVFLT`, `DIGLABEL`, `VIDEOSYN`, `TRACKOBJ`, `ARRAYNME`, `ECOMMENT`, `CCOMMENT`, `MAPFILE`), data packets (timestamp, packet id; id 0 digital/serial with insertion reason and digital input; ids 1–10000 spikes with unit class and waveform; 0xFFFF comments; video sync, tracking, button, configuration, log and recording events); NSx basic header (`NEURALCD`/`BRSMPGRP`, version, bytes in headers, label, comment, period in 1/30000 s, timestamp resolution, SYSTEMTIME, channel count), `CC` extended headers (electrode id, label, connector, pin, min/max digital and analog value, units, high/low filter corner, order, type) and data packets (0x01, timestamp, number of data points, int16 × channels per point). Revision history: 32-bit timestamps before spec 3.0, 64-bit after.
- Blackrock LB-0110 Rev 7.0, "TOC File Formats (File Spec 3.0)", https://blackrockneurotech.com/wp-content/uploads/LB-0110-7-TOC-File-Format.pdf — period counts 1/30000 s ticks; a new data packet follows a pause.
- Ripple "Trellis NEV Spec" R01838_07 (NEV/NSx 2.2 as written by Ripple), https://rippleneuro.s3-us-west-2.amazonaws.com/downloads/documentation/NEVspec2_2_v07.pdf — consulted only to confirm 32-bit data-packet timestamps and the 9-byte packet header in spec 2.2.

**Prior art consulted** (permissively licensed; documentation and source read, no code copied):
- python-neo, BSD-3-Clause, https://github.com/NeuralEnsemble/python-neo at commit `5f64a9e0` — `neo/rawio/blackrockrawio.py`: the spec 2.1 NSx layout (`NEURALSG`, 16-byte label, u32 period, u32 channel count, u32 electrode ids, then int16 samples with no packet header); spec 2.1 files carry no analog range, so the gain comes from the NEV `NEUEVWAV` digitization factor (nV per count) of the same electrode; the factor 21516 is replaced with 152592.547 nV ("a known overflow bug in old Cerebus systems"); channel labels `chan<id>` below 129 and `ainp<id − 128>` from 129 in spec 2.1; PTP files (timestamp resolution 1 GHz) store one sample per data packet; gain = (max analog − min analog) / (max digital − min digital), offset = min analog − min digital × gain. Neo 0.14.5 is also run as a black-box oracle (`oracle/gen.py`, `blackrock` branch).

**Corpus files used** (`corpus/manifest.toml`; NeuralEnsemble ephy_testing_data on G-Node GIN at commit `bf392aa`, database ODbL-1.0, contents CC-BY-SA-4.0): `brk-2-1-l101210-001.ns2` + `.nev`, `brk-test2-test.ns5`, `brk-pause-correct.ns2` + `.nev`, `brk-reset.ns2`, `brk-filespec2-3001.nev` + `.ns5`, `brk-file-spec-3-0.ns6` + `.nev`, `brk-ptp-20231027-125608-001.ns2`.

**Method:** a Python walker printed every NSx file's basic header, `CC` entries and data-packet headers, and every NEV file's basic header, extended-header ids and first packets. Observed:
- `bytes in headers` = 314 + 66 × channels in every `NEURALCD`/`BRSMPGRP` file; 336 + 32 × extended headers in every NEV.
- Pauses: `brk-pause-correct.ns2` has two packets (ts 0 and 930261, 4000 points each); `brk-reset.ns2` has a second packet whose timestamp (96) is below the end of the first (a clock reset). Each packet is one sweep.
- PTP: `brk-ptp-20231027-125608-001.ns2` (`BRSMPGRP` 3.0, resolution 10⁹) has 2149 packets of exactly one sample; timestamps jitter by ±240 ns around 1 ms. We treat such a file as one sweep per gap-free run (a gap is a step that departs from the period by more than half a sample).
- Spec 2.1: `brk-2-1-l101210-001.ns2` holds 3641 whole samples of 6 channels after a 56-byte header; `brk-test2-test.ns5` holds 6 samples of 2 channels.
- NEV 3.0 (`brk-file-spec-3-0.nev`) contains packets with id 0x8001, which no source documents; they are counted, not decoded.
- Char arrays may carry bytes after the terminating NUL (NEV comment field of `brk-pause-correct.nev`); we cut at the first NUL.

**Differences from Neo, on purpose:**
- Neo returns one sample fewer than a spec 2.1 NSx file holds (3640 of 3641 in `brk-2-1-l101210-001.ns2`); we return every whole sample. The oracle hashes Neo's 3640 and states the file's 3641 (`hashed_samples`).
- Neo deletes NSx segments shorter than 2 samples and splits PTP files only when asked; we keep every packet (standard files) and split PTP files at gaps.

## 2026-09-22 — robustness (fuzzing)

**Scope:** an underflow guard only. The `whole_blackrock` fuzz target (3 min; local seeds from the CC-BY-SA corpus files, not committed) found an NSx 3.0 PTP file (one sample per packet, nanosecond timestamps) whose first packet is cut short: zero whole packets made `count - 1` underflow. Such a file now stops after the `truncated` finding. The regression fixture `crates/openreadout-blackrock/tests/fixtures/malformed/file-crafted-ptp-partial-first-packet.ns2` is written from scratch (394 bytes, our own header layout above), not cut from a share-alike file. **Prior art consulted:** none.

## 2026-09-23 — recording directories as one session (Richard Zimring with Claude as assistant)

**Scope:** composition only. **Corpus:** `brk-3-0-session` (`file_spec_3_0.ns6` + `.nev`) and `brk-2-1-session` (`l101210-001.ns2` + `.nev`, spec 2.1 scaled from the NEV), from NeuralEnsemble ephy_testing_data on G-Node GIN at commit `bf392aa` (database ODbL-1.0, contents CC-BY-SA-4.0). **Prior art:** none beyond the entries above; each file goes through the single-file oracle (Neo's `BlackrockRawIO`). **Inferred (ours):** each NSx file is a trace of its own (sampling groups have their own rate and clock), each NEV a table. **Found and left open:** `blackrock_ptp_with_missing_samples/Hub1-NWBtestfile_neural_wspikes.ns4` (PTP): `info` reports one sweep (its lazy first/last-timestamp test passes) while `check` finds seven gap-free runs and two clock resets, and Neo refuses the file without a gap tolerance; the directory was not added to the corpus until PTP segmentation in `info` matches `check`.

## 2026-09-23 — byte-source merge integration

Adapted existing reader and session discovery to `Input`/`Fs`/`SourceFile`, preserving
parsing layouts and scaling. Evidence: existing manifest corpus ids for this format
and committed oracle JSON; source equivalence is checked by `tests/sources.rs`.
Prior art consulted: this repository (MIT OR Apache-2.0) only; no external sources or new
format-layout inferences. No corpus files changed or added.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the NSx/NEV spec versions, PTP timestamps, paused recordings, packet sizes and the µV scaling (applied). The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-10-06 — PTP files whose first and last timestamps agree despite gaps

**Corpus files:** `gin-ephy-testing-data-blackrock-ptp-missing-samples` (NeuralEnsemble/ephy_testing_data on G-Node GIN, `blackrock/blackrock_ptp_with_missing_samples/`, CC-BY-SA-4.0): an NSx 3.0 PTP `.ns4` of 1000 one-sample packets at 10 kHz whose timestamps jump +1.1 ms, −1.9 ms and +1.1 ms twice, so the first and last timestamps are exactly 999 periods apart. **Prior art consulted:** none; Neo 0.14.5 (BSD-3) was run as a black box: it refuses the file unless given `gap_tolerance_ms` and then splits it at the forward jumps only (5 segments).
**What was inferred from what.** `info` read only the first and last PTP timestamps and reported one gap-free sweep when they agreed; `check`, which reads every timestamp, found 7 gap-free sweeps and 2 clock resets. So `info` called the file validated with one continuous time axis that is wrong for 60 of its samples. `info` now reads every timestamp when the packets take at most 64 MiB, as `check` does. Larger files are probed at 64 evenly spaced packets; when every probe sits on the expected time the file is still taken as one sweep, but the sweep layout is reported as assumed (`traces[].sweep_count`), so the file is at most partially validated. A probe off the expected time makes `info` read every timestamp. Our sweep rule (a step that departs from the period by more than half a sample, backwards steps included) is unchanged, so this file has 7 sweeps where Neo has 5.
