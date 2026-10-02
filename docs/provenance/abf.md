# Provenance log — Axon Binary Format (ABF 1 and ABF 2)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

ABF is the file format of the Axon/Molecular Devices pCLAMP suite (Clampex, AxoScope). ABF 1 was once documented in an Axon SDK; ABF 2 (pCLAMP 10, 2006) was never publicly specified. The layout comes from Scott Harden's public, MIT-licensed pyABF documentation and source, and from our own inspection of the corpus files.

## 2026-09-22 — initial derivation (Richard Zimring with Claude as assistant)

**Prior art consulted** (permissively licensed; documentation and source read, no code copied):
- pyABF 2.3.8, Scott W Harden, MIT License, https://github.com/swharden/pyABF at commit `3ad9cd6f2cdfe79bc1978736ab46314866ca1c4c` (2024-10-15):
  - `website/content/abf2-file-format/index.md` ("Unofficial Guide to the ABF File Format", MIT, also published at https://swharden.com/pyabf/abf2-file-format/): ABF1/ABF2 signatures (`ABF ` / `ABF2`), the ABF2 fixed header, the ABF2 section map at byte 76 (18 entries of `u32 block, u32 entry size, i64 entry count`, blocks of 512 bytes), the strings section idea (NUL-separated strings referenced by index), interleaved int16 data, the scaling chain.
  - `website/content/abf1-file-format/index.md` ("The ABF1 File Format", MIT): ABF1 byte offsets of every field we read (groups 1–23 and the extended groups), operation-mode codes (1 event-driven variable length, 2 oscilloscope loss-free / fixed length, 3 gap-free, 4 high-speed oscilloscope, 5 episodic), date stored as YYMMDD with the 80–99 → 19xx rule, epoch type codes, synch array and tag structures (`i32 start, i32 length`; `i32 time, 56 chars, i16 type, i16 number`), telegraph fields.
  - `src/pyabf/abf.py`, `abf1/headerV1.py`, `abf2/*.py`, `abfReader.py`: ABF2 per-entry offsets of the protocol, ADC, DAC, epoch-per-DAC, epoch (digital), tag and synch-array sections; the per-channel scaling `gain = 1 / instrument_scale / signal_gain / programmable_gain [/ telegraph_gain if telegraph enabled] * adc_range / adc_resolution`, `offset = instrument_offset − signal_offset`, `value = raw × gain + offset`; ABF1 channel `i` maps to physical ADC `sampling_sequence[i]`; sample rate `1e6 / interval` (ABF2, per channel) and `1e6 / interval / channels` (ABF1, multiplexed); gap-free files have one sweep; a sweep count of 0 means 1; variable-length ABF2 sweeps come from the synch array; float32 data (`data format 1`) is not scaled; tag time conversion with the synch time unit.
  - `src/pyabf/waveform.py`: epoch type codes 0–5 and 7 (off, step, ramp, pulse train, triangle train, cosine train, biphasic train).
- pyABF is also run as a black-box oracle (`oracle/gen.py`, `abf` branch).

**Corpus files used** (all in `corpus/manifest.toml`, pinned to pyABF commit `3ad9cd6`, MIT): `pyabf-05210017-vc-abf1`, `-130618-1-12`, `-18425108-abf1`, `-file-axon-2`, `-file-axon-3`, `-multichannelabf1withtags`, `-pclamp11-4ch-abf1`, `-invaliddate-abf1`, `-sample-trace-0054` (ABF 1.3–1.84); `-14o16001-vc-pair-step`, `-16d22006-kim-gapfree`, `-171116sh-0011`, `-180415-aaron-temp`, `-18425108`, `-2018-12-09-pclamp11-0001`, `-2020-07-29-0062`, `-2015-09-10-0001`, `-file-axon-7`, `-user-list-durations`, `-2020-06-16-0000`, `-2018-11-16-sh-0006`, `-171117-hfmixfret` (ABF 2.0–2.9).

**Method:** a 70-line Python walker printed, for every corpus file, the signature, version, operation mode, sample interval, channel count, data pointer/format/count, synch array and tag pointers, the ABF2 section map, the protocol-section synch time unit and episode interval, and hex dumps of the strings section. What we inferred from the files (not stated in the prior art):
- **Strings section.** Its section-map entry is `(block, byte size of the whole string block, number of strings)`, not `count × size`: in `pyabf-14o16001-vc-pair-step` the block is 161 bytes, the count 15, and 15 × 161 bytes would overlap the next section (block 10). The block starts with `SSCH`, a u32 (1), a u32 equal to the number of strings, two further u32 values we do not interpret, and NUL padding (40 or 44 bytes in the corpus), followed by the strings, each NUL-terminated. String index 1 is the first string (`Clampex`); index 0 means "none". Strings are 8-bit; byte 0xB5 is the micro sign (`µA` in `pyabf-171117-hfmixfret`, `µm` in `pyabf-2015-09-10-0001`), so we decode Latin-1 (pyABF replaces it with `u`).
- **ABF1 header length.** Files written by ABF 1.3 (`pyabf-130618-1-12`, `pyabf-invaliddate-abf1`) have their data at block 4 (byte 2048); the "extended" ABF1 fields (telegraphs at 4512, protocol path at 4898, comment at 5154, GUID at 5282, creator version at 5798, the 20-entry epoch table at 2308) therefore lie inside the sample data. We read extended fields only when the data starts at or after byte 6144 (every 1.65–1.84 file; data at block 12 or 16); otherwise the group-9 epoch table (1444–1583) and the 56-byte comment at 310 are used. pyABF reads the extended offsets regardless; for the two 1.3 corpus files the bytes it reads as telegraph flags are not 1, so the scaling is the same.
- **ABF1 dates.** 1.3 files store `180618` (YYMMDD, per the guide); 1.65+ files store `20050210` (YYYYMMDD). Values ≥ 19000101 are read as YYYYMMDD, six-digit values as YYMMDD. `pyabf-invaliddate-abf1` stores −1 for both date and time → no creation time.
- **Sweep layout.** Fixed-length sweeps: per-channel samples per sweep = total samples / channels / sweeps (checked to divide exactly in every corpus file). `pyabf-2020-06-16-0000` (mode 1, event-driven variable length) has synch lengths 3540, 70040, 16040 summing to the data count 89620; those are its sweeps. The synch start of a sweep is in synch-time units (µs × unit) when the unit is non-zero (10 µs in `pyabf-14o16001-vc-pair-step`: starts 0, 400000, … = 4 s apart) and in multiplexed samples when it is zero (inferred; single-channel file only).
- **Data format.** `pyabf-file-axon-7` and `pyabf-user-list-durations` store float32 samples (data format 1, entry size 4); every other file int16.

**Differences from pyABF, on purpose:**
- pyABF scales in float32; we scale in float64 with the same operation order. The oracle therefore recomputes `raw × gain + offset` in float64 from pyABF's own raw samples and gains, and records how far pyABF's float32 `sweepY` is from that (`f32_max_abs_diff`, at most 0.001 in the corpus, i.e. float32 rounding of values up to ~16 000).
- pyABF truncates the sample rate to an integer (`int(1e6 / interval)`: 403 Hz for `pyabf-file-axon-7`, whose interval is 2480 µs); we report 403.2258… Hz.
- pyABF reads six-digit ABF1 dates as `%Y%m%d` (`180618` → 1806-01-08); we apply the documented YYMMDD rule (→ 2018-06-18).
- Empty channel names: pyABF reports `?`; we report `ch<index>`. Empty units: pyABF `?`, we omit the unit.

## 2026-09-22 — robustness (fuzzing)

**Scope:** a bound only. The `whole_abf` fuzz target (3 min; seeds cut from `pyabf-2018-12-09-pclamp11-0001.abf`, MIT) found headers declaring about 2^31 episodes over a few samples, which allocated tens of GB of sweep records. More sweeps than samples per channel can only be empty, so the sweep count is capped at the samples per channel, with a `sweep_count_exceeds_samples` warning. Fixture: `crates/openreadout-abf/tests/fixtures/malformed/file-fuzz-huge-sweep-count.abf`. **Prior art consulted:** none.

## 2026-09-23 — Axon Text File (ATF) (Richard Zimring with Claude as assistant)

**Prior art consulted:** pyABF 2.3.8 `pyabf/atf.py` (MIT, https://github.com/swharden/pyABF, the copy in `oracle/.venv`), read as documentation: the `ATF <version>` line, the header-record and column counts, `"key=value"` records, the tab-separated `"Signals="` record, the column-title line, then one row per sample with time first; pyABF also runs as the oracle (its structure; values re-read as float64 because pyABF parses the table as float32). **Corpus:** `pyabf-18702001-step-atf`, `pyabf-model-vc-ramp-atf`, `pyabf-model-vc-step-atf`, `pyabf-sine-sweep-magnitude-20-atf` (pyABF repository at commit `3ad9cd6`, MIT). **Inferred (ours):** channel order = first appearance in `Signals=` (pyABF uses an unordered set); the n-th column naming a signal is its n-th sweep; the time unit comes from the time column's title; a missing final line end is truncation. **Not done:** AxoGraph (`.axgd`/`.axgx`): public samples exist in ephy_testing_data (`axograph/`), not read yet.

## 2026-09-23 — byte-source merge integration

Adapted the existing reader and session discovery to `Input`/`Fs`/`SourceFile`, preserving
its parsing layout and scaling. Evidence: the existing manifest corpus ids for this format
and their committed oracle JSON; source equivalence is checked by `tests/sources.rs`.
Prior art consulted: only this repository (MIT OR Apache-2.0); no external sources or new
format-layout inferences. No corpus files were changed or added.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the ABF version and generation, acquisition mode, sample format and the ADC scaling (applied); ATF version and mode. The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-26 — command (DAC) waveforms synthesized from the epoch table

**Corpus files:** the ABF 2 development files with epoch-table waveforms — `pyabf-14o16001-vc-pair-step`, `pyabf-171116sh-0011`, `pyabf-171116sh-0018`, `pyabf-18425108`, `pyabf-2018-12-09-pclamp11-0001`, `pyabf-2015-09-10-0001`, `pyabf-2018-11-16-sh-0006`, `pyabf-2019-07-24-0055-fsi`, `pyabf-17o05024-vc-steps`, `pyabf-18808025-memtest`, `zenodo3243642-rabbit-at-0014`, `zenodo3539297-140107-1-1-48um`, `zenodo5093730-19204007`, `zenodo3837342-p3htflat-cell9`, `zenodo6367837-day35-c6-19n04045`, `zenodo18008291-haf-ap-15-day-cell-1` (synthesized); `pyabf-user-list-durations` (user list: refused). No held-out file.
**Prior art consulted:** pyABF 2.3.8 (MIT, https://github.com/swharden/pyABF) `pyabf/waveform.py` and `pyabf/stimulus.py` read as documentation for the sweep layout (1/64 pre-sweep holding, per-sweep level and duration increments, "use last epoch level" between sweeps, ramp and pulse-train shapes) and `pyabf/abf2/{dacSection,protocolSection,userListSection}.py` for the offsets of `nInterEpisodeLevel` (DAC +44), `nConditEnable` (DAC +60), `nAlternateDACOutputState` (protocol +182) and the user-list entry fields. pyABF's own synthesis is the oracle (`EpochTable(...).getWaveform()`, what `sweepC` returns).
**What was inferred from what.** Only the cases whose shape is fully fixed are synthesized: pyABF's triangle, cosine and biphasic trains leave samples undefined or depend on choices ClampEx does not document, its ABF 1 path takes the holding level from the epoch-level array and reads the user-list block at the wrong offset, and stimulus files live outside the ABF file; all of these are refused with a reason rather than approximated. All 16 synthesized files agree with pyABF sample for sample (xxh3 of every sweep).
