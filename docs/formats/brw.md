# 3Brain BrainWave raw data (`.brw`, BRW 3.x and 4.x)

BioCAM high-density MEA systems of 3Brain record into BrainWave's `.brw` files, HDF5 files of
up to 4096 channels per well sampled at about 18–20 kHz. BRW 3.x is written by BrainWave 3 and 4,
BRW 4.x by BrainWave 5.

Derived from Neo's BiocamRawIO (BSD-3), read as documentation, and the corpus files
(`docs/provenance/brw.md`). Neo is also the reference reader the corpus compares with.

Crate: `openreadout-ephys`, format id `brw` (`BRW_FORMAT_ID`), extension `brw`, reader
`BrwReader`, dataset `BrwDataset` (`open`, `open_input`, `brw`). Detection: an HDF5 file with the
`.brw` extension, or whose first 64 KiB name `3BRecInfo` or `BRW-File` → definite.

## Layout (`parse_brw` → `BrwFile`)

- **BRW 3.x** (`BrwGeneration::V3`): `/3BRecInfo/3BRecVars` holds `BitDepth`, `MaxVolt`,
  `MinVolt` (µV), `NRecFrames`, `SamplingRate` and `SignalInversion` (1 or −1);
  `/3BRecInfo/3BMeaStreams/Raw/Chs` the row and column of each stored channel;
  `/3BRecInfo/3BMeaSystem` the firmware and hardware versions. `/3BData/Raw` holds the samples
  as uint16: a frames × channels matrix when `/3BData` has `Version` 100 (`matrix`), else one
  vector of interleaved frames (versions 101, 102 or none). `gain` = (max − min) / 2^bit depth,
  `offset` = min; with signal inversion −1 a stored level v stands for 2^bit depth − v
  (`inverted_levels`).
- **BRW 4.x** (`BrwGeneration::V4`): `/ExperimentSettings` is JSON (at most `MAX_BRW_SETTINGS`
  bytes; `settings`): `ValueConverter` (min and max analog and digital values, scale factor),
  `TimeConverter.FrameRate`, `MeaSystem` versions, `MeaPlate.Model`, `ExperimentDateTime`.
  Each `/Well_<name>` group holds `StoredChIdxs` (the stored channels) and `Raw` (interleaved
  uint16 frames). `gain` = scale factor × (max − min analog) / (max − min digital), `offset` =
  min analog. Wells that keep only `EventsBasedSparseRaw` (samples around detected events) are
  refused (exit 6).
- `BrwStream` (`name`, `data_path`, `matrix`, `channels`, `frames`) is one trace;
  `data_version` the `3BData` or well version attribute; `sample_rate_hz`, `firmware`,
  `hardware`, `recorded_at`, `model`; `version_text` (`BRW 3.x`, `BRW 4.x`) and `layout`
  (`frame matrix (3BData 100)`, `interleaved (3BData <v>)`, `interleaved per well`).

## Mapping

- One trace per stream (`Raw` for BRW 3.x, the well name for BRW 4.x), one sweep, channels in
  stored order: BRW 3.x names `ch<row>-<col>`, BRW 4.x names `ch<stored index>` (their place on
  the chip is not derived). Unit µV, dtype uint16, value = level × gain + offset (with inversion,
  scale −gain and offset levels × gain + offset).
- Trace `extra`: `layout`, `firmware_version`, `hardware_version`, `signal_inversion`; on trace
  0 `recorded_at` and `model`.
- Reads are capped at `MAX_BRW_READ` frames and `MAX_BRW_READ_BYTES` per call.

## `check` finding codes

`trailing_samples` (warning).

## Oracle mapping

`oracle/gen.py` `brw()`: Neo's BiocamRawIO raw chunk × gain + offset. Neo 0.14 computes
2^BitDepth in the stored uint8 type (it wraps to 0), so for BRW 3.x the oracle recomputes the
gain from the same recording variables. BRW 4.x channel names come from `StoredChIdxs` (h5py).

## Vocabulary (every public identifier in `crates/openreadout-ephys/src/brw/` must appear here)

| identifier | meaning |
| --- | --- |
| `BrwReader`, `BRW_FORMAT_ID`, `BrwDataset`, `open`, `open_input`, `brw` | entry points |
| `BrwFile`, `parse_brw`, `file_len`, `generation`, `data_version`, `sample_rate_hz`, `gain`, `offset`, `inverted_levels`, `firmware`, `hardware`, `recorded_at`, `model`, `streams`, `settings`, `findings`, `version_text`, `layout` | the file |
| `BrwGeneration`, `V3`, `V4` | generations |
| `BrwStream`, `name`, `data_path`, `matrix`, `channels`, `frames` | a trace |
| `MAX_BRW_READ`, `MAX_BRW_READ_BYTES`, `MAX_BRW_SETTINGS` | limits |
