# Intan RHD2000 (.rhd) and RHS2000 (.rhs)

Intan recording controllers and their software write `.rhd` (RHD2000) and `.rhs` (RHS2000, with stimulation) files. OpenReadout returns the recorded signals as traces, together with the header settings: sample rate, filter bandwidths and channel names. Derived from Intan's public application notes "RHD Data File Formats" and "RHS Data File Formats" and checked on public files (RHD 1.5 and 3.3, RHS 1.0 and 3.3, including stimulation), with Neo (BSD-3) as the reference reader. Provenance: `docs/provenance/intan.md`.

Crate: `openreadout-intan`. Three layouts are read: the **traditional single file** (a header followed by data blocks, `IntanDataset`) and the two **split layouts** that keep the same header alone in `info.rhd` / `info.rhs` next to raw `.dat` files, "one file per signal type" and "one file per channel" (`SplitDataset`, section below). Every stored signal kind is **one trace with one sweep**, in data-block order.

## Detection

Bytes 0–3 as a little-endian u32: `RHD_MAGIC` (0xC6912702) or `RHS_MAGIC` (0xD69127AC) → definite (`Family::Rhd` / `Family::Rhs`). The `.rhd`/`.rhs` extension without the magic number → extension only.

## Header (`IntanHeader`, `parse_header`, `Cursor`)

All values little-endian; strings are Qt `QString`s: u32 byte length (0xFFFFFFFF = null), then UTF-16LE.

| field | RHD | RHS | our name |
| --- | --- | --- | --- |
| magic, version (i16, i16) | ✓ | ✓ | `family`, `version` (→ `format_version` `major.minor`) |
| sample rate (f32) | ✓ | ✓ | `sample_rate_hz` |
| DSP enabled (i16), DSP cutoff (f32) | ✓ | ✓ | `dsp_enabled`, `dsp_cutoff_hz` |
| lower bandwidth, [lower settle bandwidth], upper bandwidth (f32) | ✓ | ✓ (settle) | `lower_bandwidth_hz`, `lower_settle_bandwidth_hz`, `upper_bandwidth_hz` |
| the same three/four "desired" values | ✓ | ✓ | skipped |
| notch mode (i16: 0 off, 1 50 Hz, 2 60 Hz) | ✓ | ✓ | `notch_mode` |
| desired and actual impedance-test frequency (f32) | ✓ | ✓ | `impedance_test_hz` (actual) |
| amp settle mode, charge recovery mode (i16) | | ✓ | `amp_settle_mode`, `charge_recovery_mode` |
| stim step, charge recovery limit, target voltage (f32) | | ✓ | `stim_step_a`, `charge_recovery_limit_a`, `charge_recovery_target_v` |
| notes 1–3 (QString) | ✓ | ✓ | `notes` |
| temperature sensors (i16, v1.1+) | ✓ | | `temperature_sensors` |
| DC amplifier data saved (i16) | | ✓ | `dc_saved` |
| board mode (i16; RHD v1.3+) | ✓ | ✓ | `board_mode` |
| reference channel (QString; RHD v2.0+) | ✓ | ✓ | `reference` |
| number of signal groups (i16) | ✓ | ✓ | |

Each signal group: name, prefix (QString), enabled (i16), channel count (i16), amplifier count (i16); when enabled and non-empty, one record per channel (`IntanChannel`): `native_name`, `custom_name` (QString), `native_order`, `custom_order`, signal type (`signal_code`), `enabled`, `chip_channel`, [RHS: `command_stream`], `board_stream`, four Spike Scope values (skipped), `impedance_ohm`, `impedance_phase_deg` (f32); `group` records the group name. `header_len` is where the data begins.

Signal types (`SignalKind::from_code`): RHD 0 amplifier, 1 auxiliary, 2 supply, 3 board ADC, 4 digital in, 5 digital out; RHS 0 amplifier, 3 board ADC, 4 board DAC, 5 digital in, 6 digital out. Only enabled channels are stored, in header order (`enabled`).

## Data blocks (`block_layout`, `BlockPart`)

`block_samples()` = N: 128 for RHS and for RHD version 2.0 or later, 60 for older RHD files (the notes tie N to the hardware; the version rule is Neo's and holds for every corpus file). A block holds N time indices (i32; u32 before RHD 1.2, `signed_timestamps()`), then the parts below in this order; each `BlockPart` has `kind`, `offset` (within the block), `channels` and `samples` (per channel per block). Within a part, channel-major: all samples of channel 0, then channel 1, …

| `SignalKind` | family | samples per block | stored | value (`scaling`) | unit |
| --- | --- | --- | --- | --- | --- |
| `Amplifier` | both | N | u16 | raw × 0.195 − 6389.76 (= (raw − 32768) × 0.195) | µV |
| `DcAmplifier` | RHS, if `dc_saved` | N | u16 | raw × 19.23 − 9845.76 (= (raw − 512) × 19.23) | mV |
| `Stimulation` | RHS | N | u16 | `stim_steps`: ±(bits 0–7), bit 8 = negative, × `stim_step_a` | A |
| `Auxiliary` | RHD | N / 4 | u16 | raw × 37.4 × 10⁻⁶ | V |
| `Supply` | RHD | 1 | u16 | raw × 74.8 × 10⁻⁶ | V |
| `Temperature` | RHD | 1 per sensor | i16 | raw × 0.01 | °C |
| `BoardAdc` | both | N | u16 | RHD mode 0: raw × 50.354 × 10⁻⁶; mode 1: (raw − 32768) × 152.59 × 10⁻⁶; mode 13 and RHS: raw × 312.5 × 10⁻⁶ − 10.24 | V |
| `BoardDac` | RHS | N | u16 | raw × 312.5 × 10⁻⁶ − 10.24 | V |
| `DigitalIn` / `DigitalOut` | both | N | one u16 word | bit `native_order` of the word, 0 or 1, one channel per enabled line | – |

A trace's rate is sample rate × samples per block / N (auxiliary at a quarter, supply and temperature once per block). The stimulation part holds one word per amplifier channel; its amp-settle (bit 13), charge-recovery (bit 14) and compliance (bit 15) flags are not returned. The RHD digital-output word is inferred from Neo (the RHD note lists only digital inputs); no corpus RHD file has digital outputs. A zero stimulation magnitude with the sign bit set decodes to +0.0.

Trace `extra`: `signal`, `samples_per_block`, `first_time_index`; the first trace also carries `family`, `board_mode`, `dsp_enabled`, `dsp_cutoff_hz`, `bandwidth_hz`, `lower_settle_bandwidth_hz`, `notch`, `impedance_test_hz`, `reference`, `notes`, `stim_step_a`, `charge_recovery_limit_a`, `charge_recovery_target_v`, `amp_settle_mode`, `charge_recovery_mode`. Channel `extra`: `native_name`, `custom_name`, `native_order`, `chip_channel`, `board_stream`, `command_stream`, `impedance_ohm`, `impedance_phase_deg`, `bit`, `decoding`, `group`. Channel `name` is the custom name, else the native name. `start_s` = first time index / sample rate (time indices start above zero in split recordings and can be negative before a trigger).

## Split layouts (`SplitDataset`, `SplitLayout` { `PerSignalType`, `PerChannel` }, `name()`)

Derived from Intan's public "RHD/RHS Data File Formats" notes and Neo's `IntanRawIO` (BSD-3, read as documentation), checked against four corpus directories. The header file is the traditional header with no data blocks. `open` takes the directory or its `info.rhd`/`info.rhs` (`is_split_header`: a file named `info.*` with `.dat` files or `time.dat` next to it); a directory is claimed (`session_files`) only when it holds `info.rhd`/`info.rhs` and nothing but `.dat`/`.rhd`/`.rhs` files and companions (`SESSION_COMPANIONS`: `settings.xml`, notes; `info_file` finds the header). `time.dat` holds one time index per sample (i32; u32 before RHD 1.2) → `extra.first_time_index`, `start_s`.

Layout (`SplitStream`: `kind`, `channels`, `files`, `samples`; `StreamFiles` { `Interleaved` { `path`, `words`, `len` }, `PerChannel` }): when any per-type file exists the directory is one file per signal type, else one file per channel.

| `SignalKind` | per signal type (`signal_file`) | per channel (`channel_file`: prefix + native name) | stored (`split_scaling`) |
| --- | --- | --- | --- |
| `Amplifier` | `amplifier.dat` | `amp-A-000.dat` | **int16**, × 0.195 µV (no offset) |
| `DcAmplifier` (RHS) | `dcamplifier.dat` | `dc-A-000.dat` | u16, as in the traditional file |
| `Stimulation` (RHS) | `stim.dat` | `stim-A-000.dat` | u16 stimulation word, as in the traditional file |
| `Auxiliary` (RHD) | `auxiliary.dat` | `aux-A-AUX1.dat` | u16, × 37.4 µV |
| `Supply` (RHD) | `supply.dat` | `vdd-A-VDD1.dat` | u16, × 74.8 µV |
| `BoardAdc` | `analogin.dat` | `board-ANALOG-IN-1.dat` | u16, per board mode |
| `BoardDac` (RHS) | `analogout.dat` | `board-ANALOG-OUT-1.dat` | u16 |
| `DigitalIn` / `DigitalOut` | `digitalin.dat` / `digitalout.dat`: one u16 word per sample, bit `native_order` per line | `board-DIGITAL-IN-01.dat`: one u16 (0 or 1) per sample | – |

Per-type files interleave channels sample by sample (channel 0 sample 0, channel 1 sample 0, …) for the enabled channels in header order. Per-channel directories list a channel only when its file exists (RHS software may omit amplifier or stimulation files); header channels without a file are reported (`missing_file`). Every stream holds one sample per time index — auxiliary and supply inputs are written at the full rate in these layouts (the corpus files hold as many auxiliary samples as time indices; Neo labels them with the traditional quarter/block rates) — so a trace's rate is sample rate × samples ÷ time indices, which is the sample rate in every corpus directory. Temperature sensors are not saved in the split layouts. Trace `extra` adds `layout`, `data_file` (per-type) and channel `extra.data_file` (per-channel).

## `check` finding codes

Split layouts: `truncated` (a `.dat` or `time.dat` length that is not a whole number of samples), `length_mismatch` (a stream with a different number of samples than `time.dat`) (errors); `missing_file`, `no_time_file`, `data_after_header`, `time_index_gap` (warnings).

Traditional files: `truncated` (a partial data block), `bad_rate` (errors); `time_index_gap` (time indices that do not advance by one sample), `no_samples`, `unknown_board_mode` (warnings). A header that cannot be parsed to its end, or that runs past the end of the file, fails to open as corrupt (exit 4).

## Observed corpus values

| file | family, version | N | blocks | signals |
| --- | --- | --- | --- | --- |
| `intan-rhd-test-1.rhd` | RHD 1.5 | 60 | 500 | 192 amplifier, 6 supply, 8 board ADC (mode 0), 8 digital in |
| `intan-time-split-121054.rhd` | RHD 3.3 | 128 | 47 | 32 amplifier, 3 auxiliary, 3 digital in; first time index 138880 |
| `intan-test-tetrode-163225.rhd` | RHD 3.3 | 128 | 353 | 4 amplifier; board mode 13; first time index 5400192 |
| `intan-rhs-test-1.rhs` | RHS 1.0 | 128 | 500 | 32 amplifier + stimulation, 3 ADC, 2 DAC, 2 digital in |
| `intan-rhs-stim-intantestfile.rhs` | RHS 1.0 | 128 | 144 | 128 amplifier + stimulation (pulses), 16 digital out in one word |
| `intan-rhs-fpc-multistim-240514-082243.rhs` | RHS 3.3 | 128 | 726 | 8 amplifier + stimulation, 2 ADC, 2 DAC, 2 digital in, 2 digital out |
| `intan-fps-rhd-231117/` | RHD 3.3, one file per signal type | – | – | 64 amplifier, 6 auxiliary (full rate), 1 ADC, 1 digital in; 24320 samples |
| `intan-fps-rhs-240329/` | RHS 3.3, one file per signal type | – | – | 64 amplifier + DC + stimulation, 2 ADC, 1 DAC, 1 digital in, 1 digital out; 57088 samples |
| `intan-fpc-rhd-multistim-240514/` | RHD 3.3, one file per channel | – | – | 8 amplifier, 6 auxiliary, 2 ADC, 2 digital in, 2 digital out; 98048 samples |
| `intan-fpc-rhs-stim-250327/` | RHS 3.3, one file per channel | – | – | no amplifier files; 5 DC, 4 stimulation, 1 digital in; 68352 samples |

## Vocabulary (every public identifier in `openreadout-intan` must appear here)

| identifier | meaning |
| --- | --- |
| `IntanReader`, `IntanDataset`, `FORMAT_ID`, `open`, `header`, `MAX_HEADER_LEN`, `RHD_MAGIC`, `RHS_MAGIC` | entry points |
| `SplitDataset`, `SplitLayout` { `PerSignalType`, `PerChannel` }, `layout`, `SplitStream`, `kind`, `files`, `samples`, `StreamFiles` { `Interleaved`, `PerChannel` }, `path`, `words`, `len`, `signal_file`, `channel_file`, `info_file`, `split_scaling`, `session_files`, `is_split_header`, `SESSION_COMPANIONS` | split layouts |
| `Family` { `Rhd`, `Rhs` } | chip family |
| `IntanHeader`, `family`, `version`, `sample_rate_hz`, `dsp_enabled`, `dsp_cutoff_hz`, `lower_bandwidth_hz`, `lower_settle_bandwidth_hz`, `upper_bandwidth_hz`, `notch_mode`, `impedance_test_hz`, `amp_settle_mode`, `charge_recovery_mode`, `stim_step_a`, `charge_recovery_limit_a`, `charge_recovery_target_v`, `notes`, `temperature_sensors`, `board_mode`, `dc_saved`, `reference`, `channels`, `header_len`, `block_samples`, `enabled`, `signed_timestamps`, `parse_header` | header |
| `Cursor`, `block`, `at` | header reader |
| `IntanChannel`, `native_name`, `custom_name`, `native_order`, `custom_order`, `signal_code`, `chip_channel`, `command_stream`, `board_stream`, `impedance_ohm`, `impedance_phase_deg`, `group` | channel records |
| `SignalKind` { `Amplifier`, `DcAmplifier`, `Stimulation`, `Auxiliary`, `Supply`, `Temperature`, `BoardAdc`, `BoardDac`, `DigitalIn`, `DigitalOut` }, `name`, `from_code` | signal kinds |
| `BlockPart`, `kind`, `offset`, `samples`, `block_layout`, `scaling`, `stim_steps` | data blocks and scaling |

### Byte-source integration

The existing layouts are read through `Input`/`Fs`/`SourceFile`, including directory
sessions and companions where applicable. Path, memory and callback namespaces use
the same parser and scaling. See [byte sources](../../book/src/guides/wasm.md).
