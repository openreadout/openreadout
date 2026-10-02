# Provenance log — Neuralynx (NCS, NEV, NSE, NST, NTT)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

Neuralynx publishes its record formats. The structure therefore comes from the vendor's public documentation (`Source::VendorImpl`); header-key usage and segmenting come from the corpus and from Neo (BSD-3) read as documentation.

## 2026-09-22 — initial derivation (Richard Zimring with Claude as assistant)

**Vendor documentation consulted** (public, no login, retrieved 2026-09-22; no license stated, used as documentation only):
- "Neuralynx Data File Formats" web page, https://neuralynx.fh-co.com/article/neuralynx-data-file-formats/ — 16 KiB text header; the ADBitVolts worked example (raw × ADBitVolts = volts, no sign change for a file whose header says `-InputInverted True`); the Cheetah 6.4 header example.
- "NeuralynxDataFileFormats.pdf", Revision 1.1, 5/31/2013, https://neuralynx.fh-co.com/wp-content/uploads/2023/05/NeuralynxDataFileFormats.pdf — record layouts: CSC (u64 timestamp µs of the first sample, u32 channel number "NOT the A/D channel", u32 sampling frequency, u32 number of valid samples, i16[512]); Event (i16 reserved, i16 packet id, i16 data size, u64 timestamp µs, i16 event id, i16 TTL value, i16 CRC, i16 ×2 reserved, i32[8] extra, 128-byte NUL-terminated string); single electrode / stereotrode / tetrode spike records (u64 timestamp, u32 acquisition-entity number, u32 classified cell number, 8 × 32-bit features, i16[32, n] samples in [point, channel] order).
- "Cheetah Reference Guide" (2012), https://neuralynx.com/documents/CheetahReferenceGuide.pdf — meaning of `InputRange` (µV, symmetric) and `InputInverted` (Cheetah inverts the incoming A/D data itself when set).

**Prior art consulted** (permissively licensed; documentation and source read, no code copied):
- python-neo, BSD-3-Clause, https://github.com/NeuralEnsemble/python-neo at commit `5f64a9e0` — `neo/rawio/neuralynxrawio/{neuralynxrawio,nlxheader,ncssections}.py`: header lines are `-Key value` (tab or space separated, sometimes indented, `-FileType:` with a colon in Cheetah 5.4.0), `##` comment lines hold the open/close dates in several formats, the µ in `DspFilterDelay_µs` is Latin-1 or UTF-8; records with fewer than 512 valid samples end a section; a new section starts when a timestamp departs from the prediction; pre-Cheetah-5 files (`CscAcqEnt`) sample on a 1 MHz clock. Neo negates the gain when `-InputInverted True`; we do not (see below). Neo 0.14.5 is also run as a black-box oracle (`oracle/gen.py`, `neuralynx` branch).

**Corpus files used** (`corpus/manifest.toml`; NeuralEnsemble ephy_testing_data on G-Node GIN at commit `bf392aa`, database ODbL-1.0, contents CC-BY-SA-4.0): `nlx-cheetah-v4-0-2-csc14-trunc.ncs`, `nlx-bml-csc1-trunc.ncs`, `nlx-bml-unfilledsplit.ncs`, `nlx-cheetah-v5-4-0-csc5-trunc.ncs`, `nlx-cheetah-v5-5-1-tet3a.ncs`, `nlx-cheetah-v5-5-1-stet3a.nse`, `nlx-cheetah-v5-5-1-events.nev`, `nlx-cheetah-v5-7-4-csc1.ncs`, `nlx-cheetah-v5-7-4-events.nev`, `nlx-cheetah-v5-6-3-tt1.ntt`, `nlx-cheetah-v6-4-1dev-csc1-truncated.ncs`, `nlx-cheetah-v6-3-2-csc1-reduced.ncs`. Not added: the Cheetah 1.1.0 file (a truncated clinical recording from an epilepsy-monitoring study), the `two_streams_*` folders (obfuscated clinical data) and the Pegasus event files (a subject number in the stored path) — nothing clinical goes into the corpus even when pseudonymized. `nlx-bml-unfilledsplit.ncs` records a source path with a pseudonymous subject code (`s003`) and no other personal data.

**Method:** a Python walker printed each file's header text, record count and remainder, and per-record timestamp, channel number, sampling frequency, valid-sample count and first samples. Observed and now handled:
- Header text is Latin-1 (0xB5) or UTF-8 (0xC2 0xB5) for µ; NULs pad it to 16384 bytes; lines end in CRLF (Cheetah) or LF (BML); keys are indented with tabs in BML and Cheetah 4.x files; `-FileType: CSC` (Cheetah 5.4.0); quoted values (`-ApplicationName Cheetah "5.7.4 "`).
- The record's channel number differs from `-ADChannel` (13 vs 26, 0 vs 8): we report both, the header value as the A/D channel.
- The record's sampling frequency is the header value truncated to an integer (1017 vs 1017.375).
- Cheetah 4.0.2 (`nlx-cheetah-v4-0-2-csc14-trunc.ncs`): header 27789 Hz, but records are 17920 µs apart = 512 × 35 µs (1 MHz clock, 1e6/27789 truncated), i.e. 28571.4 Hz. We keep the header rate as `sample_rate_hz`, report the rate the timestamps imply as `extra.timestamp_rate_hz`, and `check` flags the difference (`rate_mismatch`).
- Partly filled records: `nlx-bml-unfilledsplit.ncs` (308 of 512, then a jump of 2^32 + 17408 µs), `nlx-cheetah-v5-7-4-csc1.ncs` (31/287/287 valid, then gaps of 3–4.5 s; last record 255). The samples after the valid count are stale data from an earlier record (not zeros), so we never return them.
- Event records: data size 0 in Cheetah 5.5.1 files and 2 in 5.7.4 (the PDF says "always 2").
- Spike features: negative values in the corpus; the PDF says UInt32. We read them as signed 32-bit (inferred).

**Segmenting rule (ours):** the per-sample interval `dt` is the median of Δtimestamp/512 over consecutive full records (fallback 1e6 / header rate); a new segment (sweep) starts where the next record's timestamp departs from `timestamp + valid × dt` by more than `dt / 2` (less than half a sample cannot be a missing sample). Neo uses rate-dependent tolerances (0 µs for BML/ATLAS, 20 % of a sample for Digital Lynx); on the corpus files both give the same segments (see the corpus harness).

**Scaling:** µV = raw × ADBitVolts × 10⁶, no sign change. The vendor page's worked example and the Cheetah guide both say stored samples are already inverted when `-InputInverted True`; Neo 0.14.5 negates the gain for such files. The oracle compares against |Neo gain|; `extra.input_inverted` reports the flag.

## 2026-09-23 — recording directories as one session (Richard Zimring with Claude as assistant)

**Scope:** composition only; no record layout changed. **Corpus:** `nlx-cheetah-v5-7-4-session` (five CSC channels + events), `nlx-cheetah-v5-5-1-session` (two continuous channels, two `.nse`, events, Cheetah logs), `nlx-cheetah-v6-4-1dev-session` (three channels at three rates), all from NeuralEnsemble ephy_testing_data on G-Node GIN at commit `bf392aa` (database ODbL-1.0, contents CC-BY-SA-4.0); a two-channel directory with 1–2 µs timestamp jitter (`neuralynx/two_streams_with_small_time_differences`) was examined and left out of the corpus (below). **Prior art:** Neo 0.14.5 `neuralynxrawio.py` (BSD-3), read for how a directory is scanned (`.ncs` files grouped into streams by sampling rate, input range and DSP filter settings; header-only `.ncs` files skipped; streams with incompatible segment structures refused) and run as the directory oracle. **Inferred (ours):** channels join one trace only on an identical sample grid (rate, sweep count, sweep lengths, sweep start timestamps), since values are scaled per channel and a shared grid is what makes one multi-channel trace meaningful; input range and filters stay per-channel metadata. Header-only `.ncs` files are skipped as in Neo. Where Neo refuses a directory, the oracle falls back to Neo on each file alone. **Difference from Neo:** Neo splits `two_streams_with_small_time_differences/LAHC1.ncs` into three segments at 1–2 µs timestamp jitter (at 2 kHz, a 500 µs sample period); our single-file reader keeps jitter under half a sample period in one segment, so that directory was not added as a bit-exact oracle case.

## 2026-09-23 — byte-source merge integration

Adapted existing reader and session discovery to `Input`/`Fs`/`SourceFile`, preserving
parsing layouts and scaling. Evidence: existing manifest corpus ids for this format
and committed oracle JSON; source equivalence is checked by `tests/sources.rs`.
Prior art consulted: this repository (MIT OR Apache-2.0) only; no external sources or new
format-layout inferences. No corpus files changed or added.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the -FileVersion, file kinds, segmentation and the ADBitVolts scaling (applied). The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-26 — video-tracker (.nvt) records; a new depositor

**Corpus files:** `figshare25325560-vt1-nvt` (Cheetah 6.4.1 video tracker), `figshare25325560-csc12-ncs`, `figshare25325560-csc37-ncs`, `figshare25325560-events-nev` (Figshare article 25325560, CC BY 4.0, Stout, George, Kim, Hallock, Griffin). The reader was first written against a video-tracker file from a second article of the same authors; held-out draw C then reserved that article, so its file left the development corpus and the article above supplies the development `.nvt`. No held-out file. Same lab, different record: draw C holds out another figshare article of these authors; records are the unit (docs/benchmark/heldout.md, rule 3), so this article stays development data.
**Vendor documentation:** "NeuralynxDataFileFormats.pdf", Revision 1.1, 5/31/2013, https://neuralynx.fh-co.com/wp-content/uploads/2023/05/NeuralynxDataFileFormats.pdf, page 5 "Video Tracker Record" (field order and sizes, 0x800 record start, extracted x/y/angle, 50 targets, 0 = no target) and page 6 (bitfield layout, not decoded here).
**Prior art run as an oracle:** nept 0.1.0 (vandermeerlab, MIT, https://github.com/vandermeerlab/nept) `loaders_neuralynx.load_nvt(remove_empty=False)`, black-box: timestamps (seconds, compared after × 10⁶ and rounding), x and y column by column. Neo 0.14.5 lists `.nvt` as not yet supported.
**What was inferred from what.** The record size 1828 = 6 + 8 + 1600 + 2 + 12 + 200 bytes follows from the documented field list; the corpus file's `-RecordSize 1828` and `-FileType Video` agree. Nothing else is inferred: x, y and angle are reported as stored.
