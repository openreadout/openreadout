# Provenance log — Intan RHD2000 (.rhd) and RHS2000 (.rhs)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

Intan Technologies publishes its data file formats as application notes. Header layout, data blocks and scaling therefore come from the vendor's public documentation (`Source::VendorImpl`).

## 2026-09-22 — initial derivation (Richard Zimring with Claude as assistant)

**Vendor documentation consulted** (public PDFs, no login, retrieved 2026-09-22; no license stated, used as documentation only):
- "RHD Application Note: Data File Formats" (19 September 2013, updated 28 April 2022), https://intantech.com/files/Intan_RHD2000_data_file_formats.pdf — magic number 0xC6912702, version, sample rate, DSP and bandwidth settings, notch mode, impedance-test frequencies, three notes as Qt `QString`s (u32 byte length, 0xFFFFFFFF = null, UTF-16LE), temperature-sensor count (v1.1+), board mode (v1.3+), reference channel (v2.0+), signal groups (name, prefix, enabled, channel count, amplifier count) with per-channel records (native and custom name, native and custom order, signal type 0–5, enabled, chip channel, board stream, Spike Scope settings, impedance); the traditional single-file layout: data blocks of N samples (N = 60 for the USB interface board, 128 for Recording Controllers and RHX) holding N int32 time indices, N amplifier samples per enabled amplifier channel, N/4 per auxiliary channel, one per supply-voltage channel and temperature sensor, N per board ADC channel and one N-sample digital-input word; scaling: amplifier (v − 32768) × 0.195 µV, auxiliary v × 37.4 µV, supply v × 74.8 µV, temperature v / 100 °C, board ADC by board mode (0: v × 50.354 µV; 1: (v − 32768) × 152.59 µV; 13: (v − 32768) × 312.5 µV).
- "RHS Application Note: Data File Formats" (2022), https://intantech.com/files/Intan_RHS2000_data_file_formats.pdf — magic number 0xD69127AC, settle bandwidths, amp-settle and charge-recovery modes, stimulation step size, charge-recovery limit and target, DC-amplifier-saved flag, board mode, reference channel; channel records with a command-stream field; data blocks of 128 samples: time indices, amplifier, DC amplifier ((v − 512) × 19.23 mV), stimulation words (bits 0–7 magnitude in step units, bit 8 sign, bit 13 amp settle, bit 14 charge recovery, bit 15 compliance limit), board ADC and DAC ((v − 32768) × 312.5 µV), digital inputs and outputs.

**Prior art consulted** (BSD-3): python-neo `neo/rawio/intanrawio.py` at commit `5f64a9e0` — N = 60 for RHD files before version 2.0 and 128 from 2.0 (the PDF ties N to the hardware, not the file version); time indices are unsigned before RHD 1.2; a digital-output word follows the digital-input word in RHD blocks when any output is enabled (the RHD PDF omits it); only enabled channels of enabled groups are stored, in header order. Neo 0.14.5 is also the oracle (`oracle/gen.py`, `intan` branch).

**Corpus files used** (`corpus/manifest.toml`; NeuralEnsemble ephy_testing_data on G-Node GIN at commit `bf392aa`, database ODbL-1.0, contents CC-BY-SA-4.0): `intan-rhd-test-1.rhd` (RHD 1.5), `intan-time-split-121054.rhd` (RHD 3.3), `intan-test-tetrode-163225.rhd` (RHD 3.3, board mode 13), `intan-rhs-test-1.rhs` (RHS 1.0), `intan-rhs-stim-intantestfile.rhs`, `intan-rhs-fpc-multistim-240514-082243.rhs` (RHS 3.3).

**Method:** a header walker written from the PDFs parsed every corpus file; the header ends where the data begins, and the data length is a whole number of blocks in every file under the block-size formula (RHD 1.5: 16226-byte header, 24372-byte blocks, 500 blocks; RHD 3.3 time-split: 9152-byte blocks, 47 blocks, time indices 138880–144895; RHS 1.0: 18432-byte blocks, 500 blocks; `intan-rhs-stim-intantestfile.rhs`: 69120-byte blocks, 144 blocks, where the 16 digital outputs share one word per sample — the RHS PDF says "for each channel", but only one word makes the blocks whole).

**Inferred, not yet corroborated by a corpus file:** RHD temperature sensors, RHD files before 1.2 (unsigned time indices), RHD with digital outputs enabled, RHS with DC-amplifier data in a single file.

**Differences from Neo, on purpose:** we report the time of the first sample (first time index / rate) as `start_s` (Neo starts at 0); temperature is °C = v / 100 as the PDF says (Neo uses 0.001).

**2026-09-22, stimulation sign (corpus: `intan-rhs-stim-intantestfile.rhs`, `intan-rhs-fpc-multistim-240514-082243.rhs`):** the first oracle comparison differed only in the hash of the stimulation-current channels. The values matched, but a zero-magnitude word with the sign bit set decoded to −0.0 A (we negated the float), while Neo produces +0.0 (it negates the integer magnitude). The hashes cover the f64 bytes, so they differed. We now negate the integer step count before scaling. Stimulation words decode to +0.0 when the magnitude is zero, whatever the sign bit, and every sweep × channel hash matches.

## 2026-09-23 — "one file per signal type" and "one file per channel" layouts (Richard Zimring with Claude as assistant)

**Vendor documentation consulted:** the same public Intan application notes ("RHD Data File Formats", "RHS Data File Formats", intantech.com), sections on the two headerless layouts: `info.rhd`/`info.rhs` holds the standard header, `time.dat` the time indices, one `.dat` per signal type (channels interleaved) or per channel; amplifier data stored as signed 16-bit values. **Prior art:** Neo 0.14.5 `intanrawio.py` (BSD-3): file names per signal type (`amplifier.dat`, `auxiliary.dat`, `supply.dat`, `analogin.dat`, `analogout.dat`, `digitalin.dat`, `digitalout.dat`, `dcamplifier.dat`, `stim.dat`) and the per-channel prefixes (`amp-`, `aux-`, `vdd-`, `board-ANALOG-IN-`, `board-ANALOG-OUT-`, `board-DIGITAL-IN-`, `board-DIGITAL-OUT-`, `dc-`, `stim-`); amplifier values = signed raw × 0.195 µV; per-type digital files pack the lines into one word, per-channel digital files hold one value per sample. Neo is also the oracle, through `info.rhd`/`info.rhs`. **Corpus:** `intan-fps-rhd-231117`, `intan-fps-rhs-240329`, `intan-fpc-rhd-multistim-240514`, `intan-fpc-rhs-stim-250327`, from NeuralEnsemble ephy_testing_data on G-Node GIN at commit `bf392aa` (database ODbL-1.0, contents CC-BY-SA-4.0). **Observed:** every `.dat` file in all four directories holds exactly one sample per `time.dat` index, auxiliary inputs included (Neo labels those at a quarter of the rate); in `intan-fpc-rhs-stim-250327` the amplifier files are absent while DC and stimulation files exist, and only four of the five DC channels have stimulation files. **Inferred (ours):** a stream's rate = sample rate × its samples ÷ time indices (the sample rate in every corpus directory); header channels without a file are listed as `missing_file`; per-channel names come from the native channel names (`A-000` → `amp-A-000.dat`).

## 2026-09-23 — byte-source merge integration

Adapted existing reader and session discovery to `Input`/`Fs`/`SourceFile`, preserving
parsing layouts and scaling. Evidence: existing manifest corpus ids for this format
and committed oracle JSON; source equivalence is checked by `tests/sources.rs`.
Prior art consulted: this repository (MIT OR Apache-2.0) only; no external sources or new
format-layout inferences. No corpus files changed or added.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the header version, chip family, directory layout, signal kinds and the scaling (applied). The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.
