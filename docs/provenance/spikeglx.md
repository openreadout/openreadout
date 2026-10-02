# Provenance log — SpikeGLX (.bin + .meta)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

SpikeGLX is open acquisition software by Bill Karsh (HHMI Janelia). Its data format is documented by its authors: a `key=value` text `.meta` file beside a headerless interleaved int16 `.bin`. Structure and scaling therefore come from the authors' public documentation (`Source::VendorImpl`).

## 2026-09-22 — initial derivation (Richard Zimring with Claude as assistant)

**Author documentation consulted** (public; the SpikeGLX site states "Use is subject to Janelia Research Campus Software Copyright terms", http://license.janelia.org/license, BSD-3):
- SpikeGLX metadata help, https://billkarsh.github.io/SpikeGLX/Sgl_help/Metadata_Help.html (and the version pages `Metadata_20.html`, `Metadata_3A.html`, `Metadata_3B1.html`, `Metadata_3B2.html`), retrieved 2026-09-22: `key=value` lines, `~`-prefixed list keys written `(header)(entry)(entry)…`; `typeThis` (imec, nidq, obx); `nSavedChans`, `fileSizeBytes`, `firstSample`, `fileCreateTime`, `imSampRate`/`niSampRate`/`obSampRate`, `snsApLfSy`/`acqApLfSy`, `snsMnMaXaDw`/`acqMnMaXaDw`, `snsXaDwSy`/`acqXaDwSy`, `snsSaveChanSubset` (`all` or `a:b,c` lists of acquired channels, inclusive), `~snsChanMap` (`name;acquired:order`), `~imroTbl`; conversion `V = i × Vmax / Imax / gain` with imec `Vmax = imAiRangeMax`, `Imax = imMaxInt` (512 in the 10-bit example), `gain = imChan0apGain`, or for older metadata 80 for probe types 21/24, 100 for 2003/2013, the imro-table AP/LF entry for type 0 and other selectable-gain probes; NI `Vmax = niAiRangeMax`, `Imax = 32768` (`niMaxInt`), `gain = niMNGain` / `niMAGain`; OneBox `Vmax = obAiRangeMax`, `Imax = 32768`; the SY word is status bits, not a voltage.
- SpikeGLX imro-table help, https://billkarsh.github.io/SpikeGLX/help/imroTables/ — per probe type, which imro entry fields hold the AP and LF gains (`chan bank refId APgain LFgain APhipass` for NP 1.0-style tables).
- SpikeGLX "Datafile Tools" `readSGLX.py`, https://github.com/jenniferColonell/SpikeGLX_Datafile_Tools (linked from the SpikeGLX site; no license file) — read only as the authors' description of the same conversion (order: imro gains for selectable-gain probes before `imChan0apGain`); no code copied.
- ProbeTable, https://github.com/billkarsh/ProbeTable (`Tables/probe_features.json`, HHMI BSD-3) — ADC bit depth per probe part number, used for the fallback `imMaxInt` of probes whose metadata omits it.

**Prior art consulted** (BSD-3): python-neo `neo/rawio/spikeglxrawio.py` at commit `5f64a9e0` — stream naming (`imec0.ap`, `imec0.lf`, `nidq`, `obx0`), channel names from `~snsChanMap`, gain grouping `(Vmax / Imax) × (1 / gain) × 10⁶` µV. Neo 0.14.5 is also the oracle (`oracle/gen.py`, `spikeglx` branch).

**Corpus files used** (`corpus/manifest.toml`; NeuralEnsemble ephy_testing_data on G-Node GIN at commit `bf392aa`, database ODbL-1.0, contents CC-BY-SA-4.0): `sglx-noise4sam-g0-t0.imec0.ap/.lf`, `sglx-noise4sam-g0-t0.nidq`, `sglx-test-20210920-0-g0-t0.imec0.ap`, `sglx-np2-with-sync.imec0.ap`, `sglx-np2-with-sync.nidq`, `sglx-np2-subset-with-sync.imec0.ap`, `sglx-np2-no-sync.exported.imec0.ap`, `sglx-digitalchanneltest-g0-t0.nidq`, `sglx-5-19-2022-ci1-g0-t0.imec0.ap/.lf` (each `.bin` with its `.meta`, same stem).

**Observed and now handled:**
- Most `.bin` files in the corpus are stubs: shorter than `fileSizeBytes` (e.g. 889350 of 121625350 bytes in `sglx-noise4sam-g0-t0.imec0.ap`). Sample counts come from the real size; `check` reports the shortfall as `truncated` (exit 4) — a recording cut short looks exactly like this.
- `snsSaveChanSubset` with ranges and single channels (`0:35,72:95,192:227,264:287,384`; `384:768` for an LF file whose acquired channels 384–767 are LF and 768 is SY).
- `imMaxInt` missing from 2019 metadata (NP 1.0, 10-bit → 512); `niMaxInt` missing (→ 32768).
- NP 2.0 exported without the SY channel (`snsApLfSy=384,0,0`, `acqApLfSy=384,0,1`).
- Calibrated, non-integer rates (`imSampRate=30000.061088`, `niSampRate=11574.074074`).

**Differences from Neo, on purpose:** SY and XD channels are status/digital words, returned raw (scale 1, no unit); Neo scales them like voltages. For `sglx-np2-no-sync` Neo gives the last AP channel the SY gain (it assumes the last channel is SY); the oracle uses the AP gain there.

## 2026-09-22 — robustness (fuzzing)

**Scope:** bounds only. The `whole_spikeglx` fuzz target (3 min; local seeds from the corpus `.meta`/`.bin` pairs, not committed) found `.meta` files whose acquired-channel counts (`acqApLfSy` and siblings) or `snsSaveChanSubset` ranges listed billions of channels, sizing multi-GB tables. A stream may now acquire or save at most 65,536 channels (the existing `nSavedChans` bound); more is a clean error. Regression: `crates/openreadout-spikeglx/tests/fixtures/malformed/bundle-whole_spikeglx-crafted-huge-acquired-count.bin` (written from scratch) and the unit test `channel_counts_are_bounded`. **Prior art consulted:** none.

## 2026-09-23 — performance / robustness merge

Reconciled the existing performance branch with main; no new format layout was inferred.
Inputs: existing committed synthetic and malformed regression fixtures and the shared public
corpus (ids/licences unchanged in `corpus/manifest.toml`). Prior art: repository code and notes;
weezl 0.2.1 public buffer API (https://docs.rs/weezl/0.2.1/weezl/decode/struct.IntoStream.html,
MIT OR Apache-2.0) for a reusable bounded LZW staging buffer. Retained file-relative allocation
limits, checked geometry, bounded decoder output and capped sidecar reads alongside parallel
decoding and copy avoidance. The common plane-size guard also enforces the 4 GiB ceiling.

## 2026-09-23 — run directories as one session (Richard Zimring with Claude as assistant)

**Scope:** composition only. **Corpus:** `sglx-noise4sam-g0-run` (imec0 AP + LF in the probe subdirectory, NI-DAQ) and `sglx-np2-with-sync-run` (NP 2.0 AP with sync, NI-DAQ), from NeuralEnsemble ephy_testing_data on G-Node GIN at commit `bf392aa` (database ODbL-1.0, contents CC-BY-SA-4.0), laid out as SpikeGLX writes them. **Prior art:** Neo 0.14.5 `spikeglxrawio.py` (BSD-3) run on the run directory as the oracle (it scans the directory tree for `.meta` files and names streams `imec0.ap`, `imec0.lf`, `nidq`). **Inferred (ours):** one trace per stream in path order; no alignment on sync pulses (that would change sample timing; the metadata documentation at https://billkarsh.github.io/SpikeGLX/Sgl_help/Metadata_Help.html describes the sync inputs, which stay available as channels).

## 2026-09-23 — byte-source merge integration

Adapted existing reader and session discovery to `Input`/`Fs`/`SourceFile`, preserving
parsing layouts and scaling. Evidence: existing manifest corpus ids for this format
and committed oracle JSON; source equivalence is checked by `tests/sources.rs`.
Prior art consulted: this repository (MIT OR Apache-2.0) only; no external sources or new
format-layout inferences. No corpus files changed or added.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the SpikeGLX release year, stream kind, probe type, max int and the µV/V scaling (applied). The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-26 — probe sites from ~snsGeomMap / ~snsShankMap; a second depositor

**Corpus files:** `zenodo21908762-sub-001-ses-001-g0-t0-imec0-ap-bin` (Zenodo 21908762, CC-BY-4.0, NIU Open Software Summer School course data: NP2010 four-shank AP stream, SpikeGLX 20230411) — new; `sglx-np2-with-sync-imec0-ap-bin`, `sglx-np2-subset-with-sync-imec0-ap-bin`, `sglx-np2-no-sync-exported-imec0-ap-bin` (geometry maps), `sglx-noise4sam-g0-t0-imec0-ap-bin`, `sglx-test-20210920-0-g0-t0-imec0-ap-bin` (shank maps). No held-out file.
**Author documentation consulted:** SpikeGLX metadata help, https://billkarsh.github.io/SpikeGLX/Sgl_help/Metadata_Help.html (retrieved 2026-09-26): `~snsShankMap` header `(shanks,cols,rows)` and entries `(s:c:r:u)`; `~snsGeomMap` header `(part-number,shanks,shank spacing,shank width)` and entries `(s:x:z:u)` with x from the shank's left edge and z from the centre of the bottom-most row, per-shank origins; "there are electrode entries only for saved channels"; the geometry map replaces the shank map for imec probes as of 20230202.
**Prior art run as an oracle:** probeinterface 0.4.0 (MIT, https://github.com/SpikeInterface/probeinterface) `read_spikeglx`, black-box: contact positions and shank ids, compared in the corpus test (`channel_extra`; x up to one constant because probeinterface measures from the left-most column and adds shank × pitch).
**What was inferred from what.** The documentation fixes the map layouts; that entries follow the saved neural channels in saved order (imec AP then LF; the SY channel has none) is from the documentation's "entries only for saved channels" and the entry counts in all eight corpus metas (385 elements for 384 saved neural channels, 121 for the 120-channel subset). A count mismatch drops the geometry rather than guessing an alignment.

**Addendum (same day):** `zenodo5899237-pt03-imec0-lf-bin` (Zenodo 5899237, CC0-1.0, Paulk et al., human Neuropixels LF band, SpikeGLX 20201024, 1.1 GB) added as a third depositor. `read_trace` now also caps one read at `MAX_READ_VALUES` = 64 Mi values in all (a 385-channel stream returned up to 4 Mi samples × 385 channels = 12 GiB of f64 before), and the signal analyses page their reads (`openreadout_core::trace::read_channels`). The oracle hashes the first 2^17 samples of streams longer than 2^20 samples (`hashed_samples`); the sample count is checked over the whole stream.
