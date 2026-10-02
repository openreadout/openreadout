# SpikeGLX (.bin + .meta)

SpikeGLX records Neuropixels probes and NI-DAQ boards, writing one `.bin` data file and one `.meta` text file per stream. OpenReadout returns a stream as one trace (probe channels in µV, analog channels in V), with probe, headstage and electrode-site metadata.

Derived from the SpikeGLX authors' public metadata documentation (Metadata help pages, imro-table help, ProbeTable) and checked against public streams (Neuropixels 1.0 AP/LF, 2.0, NI-DAQ analog and digital) with Neo (BSD-3) as a reference reader. See `docs/provenance/spikeglx.md`.

Crate: `openreadout-spikeglx`. A SpikeGLX run writes one `.bin`/`.meta` pair per stream (`<run>_g0_t0.imec0.ap.bin`, `.imec0.lf.bin`, `.nidq.bin`, `.obx0.obx.bin`). OpenReadout opens **one stream** — pass the `.bin` or the `.meta` (`stream_paths`) — as one trace with one sweep.

## Detection

No signature in the `.bin` (it is headerless). A `.bin` whose same-name `.meta` contains `nSavedChans=` and `typeThis=` (or `…SampRate=`) is definite (`looks_like_meta`); so is such a `.meta` itself. A `.bin` without its `.meta` is not claimed; opening it directly fails as unsupported (exit 6) with a hint to keep the `.meta` beside it.

## `.meta` (`Meta`, `parse_meta`, `entries`, `get`, `number`, `counts`)

One `key=value` per line (CRLF), split at the first `=`. Keys starting with `~` hold lists written `(header)(entry)(entry)…` (`list_elements`). Keys we use:

| key | use |
| --- | --- |
| `typeThis` | `StreamKind`: `imec` → `Imec`, `nidq` → `Nidq`, `obx` → `Obx` |
| `nSavedChans` | channels per sample |
| `imSampRate` / `niSampRate` / `obSampRate` | `sample_rate_hz` (calibrated, may be non-integer) |
| `firstSample` | `start_s` = firstSample / rate (sample index within the run) |
| `acqApLfSy` / `acqMnMaXaDw` / `acqXaDwSy` | acquired channel layout by kind |
| `snsSaveChanSubset` | acquired indices that were saved, `all` or inclusive ranges `a:b,c` (`parse_subset`) |
| `~snsChanMap` | channel names (`AP0;0:0` → `AP0`) |
| `~imroTbl`, `imDatPrb_type` | per-channel gains for selectable-gain probes |
| `imAiRangeMax`, `imMaxInt`, `imChan0apGain`, `imChan0lfGain` | imec scaling |
| `niAiRangeMax`, `niMaxInt`, `niMNGain`, `niMAGain` | NI scaling |
| `obAiRangeMax`, `obMaxInt` | OneBox scaling |
| `fileSizeBytes`, `fileTimeSecs` | declared size and duration (`check`) |
| `appVersion`, `fileCreateTime`, `imDatPrb_pn`, `imDatPrb_sn`, `imDatHs_sn`, `imDatBs_sn`, `fileName` | trace `extra`: `app_version` (also `format_version`), `created_at`, `probe_part_number`, `probe_serial_number`, `headstage_serial_number`, `basestation_serial_number`, `original_file_name` |

## Channels (`SavedChannel`, `saved_channels`)

Saved channel `i` is acquired channel `subset[i]` (`acquired`); its `ChannelKind` follows from the acquired layout: imec `Ap`, `Lf`, `Sync` (`acqApLfSy`); NI `Mn`, `Ma`, `Xa`, `Xd` (`acqMnMaXaDw`); OneBox `Xa`, `Xd`, `Sync`. `prefix()` gives `AP`, `LF`, `SY`, `MN`, `MA`, `XA`, `XD`; `is_bits()` is true for `Sync` and `Xd`. `name` comes from `~snsChanMap`, else prefix + running number.

### Scaling

The documentation's rule is `V = i × Vmax / Imax / gain`.

- **imec AP/LF** (µV): `scale = (imAiRangeMax / Imax) × (1 / gain) × 10⁶` (`imec_scale`). `gain`: for selectable-gain probes (`has_selectable_gain`: types 0, 1020, 1030, 1100, 1120–1123, 1200, 1300) the `~imroTbl` entry of the channel (field 3 for AP, field 4 for LF; LF entry index = acquired − number of AP channels); else `imChan0apGain`/`imChan0lfGain`; else, for type 1110, the imro header gains; else `fixed_gain` (80 for types 21/24, 100 otherwise). `Imax` = `imMaxInt`, or when absent `default_max_int` (512 for 10-bit NP 1.0, 8192 for types 21/24, 2048 otherwise — the last from the ProbeTable, `Source::Inferred`).
- **NI MN/MA/XA** (V): `scale = (1 / gain) × (niAiRangeMax / niMaxInt)` (`ni_scale`), `gain` = `niMNGain`, `niMAGain`, or 1; `niMaxInt` defaults to 32768.
- **OneBox XA** (V): `obAiRangeMax / obMaxInt` (32768 by default).
- **SY and XD** words: raw (scale 1, no unit, channel `extra.bits`).

Channel `extra`: `kind`, `acquired_index`, `gain`, `bits`, and the electrode site below.

### Probe sites (`SiteMap`, `ChannelSite`, `site_map`)

From the SpikeGLX metadata help (imec section). `~snsGeomMap` (SpikeGLX 20230202 and later; preferred) or the older `~snsShankMap` (imec, and NI streams with MN channels) hold one entry per saved neural channel (imec AP and LF, NI MN), in saved order, after a header:

| map | header | entry | our fields |
| --- | --- | --- | --- |
| `~snsGeomMap` | `(part-number,shanks,shank spacing µm,shank width µm)` | `(s:x:z:u)` | trace `extra.probe_geometry` {`source`, `part_number`, `shanks`, `shank_pitch_um`, `shank_width_um`}; channel `extra.shank`, `x_um` (electrode centre from the shank's left edge), `z_um` (from the centre of the bottom-most row), `used` |
| `~snsShankMap` | `(shanks,columns,rows)` (grid maxima) | `(s:c:r:u)` | trace `extra.probe_geometry` {`source`, `shanks`, `columns`, `rows`}; channel `extra.shank`, `col`, `row`, `used` |

Shank 0 is left-most with the tips pointing down; each shank has its own (x, z) origin. `used` is the map's u-flag (drawn in the viewers, included in spatial averages). A map whose entry count differs from the saved neural channels, or with an unparsable entry, a shank beyond the header's count or a flag other than 0/1, is ignored: no site is reported rather than a misaligned one. Grid indices are not converted to µm (that needs per-probe electrode pitches); LF streams written before the geometry map carry no map. Checked against probeinterface's `read_spikeglx` (MIT), which places contacts from its own probe tables and the imro table: shank and z equal, x equal up to one constant per probe (probeinterface measures x from the left-most column plus shank × pitch).

## Samples

The `.bin` is headerless int16 little-endian, interleaved by sample (all saved channels of sample 0, then sample 1, …) in saved order. Samples = file size / (2 × nSavedChans), so a stubbed or cut `.bin` still opens; a remainder → `partial_sample`. One `read_trace` returns at most 4 Mi samples per channel and `MAX_READ_VALUES` (64 Mi) values in all — about 174 k samples of a 385-channel probe stream; callers page with `first_sample`.

## Run directories (sessions)

A SpikeGLX run directory (`<run>_g0/`, with the probe streams one level down in subdirectories named `…_imec<N>`, normally `<run>_g0_imec0/`; other subdirectories are ignored) is one recording (`session_files`, `open_session`) when it holds `.bin` files with their `.meta` and nothing else but `SESSION_COMPANIONS` (`.meta`, `.txt`, `.log`, `.json`, `.md`, `.csv`). Every stream — imec AP and LF bands, NI-DAQ, OneBox — is a trace of its own, in path order (probe subdirectories sort before the NI stream of the same run), each keeping its own rate and sample clock: streams are **not** aligned on their sync pulses. Several triggers or gates in one directory are separate traces. Nothing is resampled or realigned; every trace, channel and table names its file in `extra.source_file` / `extra.source_files`, `info --view structure` lists the member files and `check` checks each one (findings prefixed with the file). The composition itself is `openreadout_core::session` (`SessionDataset`).

## `check` finding codes

`truncated` (the `.bin` is shorter than `fileSizeBytes`), `bad_rate` (errors); `size_mismatch`, `partial_sample`, `no_declared_size`, `no_scaling`, `no_samples` (warnings). A `.meta` whose saved-channel list disagrees with `nSavedChans` or the acquired layout fails to open as corrupt (exit 4). Most corpus `.bin` files are stubs cut from longer recordings, and `check` reports them as truncated.

## Observed corpus values

| stream | probe / device | channels | rate | Imax, gain | notes |
| --- | --- | --- | --- | --- | --- |
| `sglx-noise4sam-g0-t0.imec0.ap` | NP 1.0 (type 0) | 384 AP + SY | 30000 | 512 (absent), 500 | 2019 metadata, stub |
| `sglx-noise4sam-g0-t0.imec0.lf` | NP 1.0 | 384 LF + SY | 2500 | 512, 125 | subset `384:768` |
| `sglx-noise4sam-g0-t0.nidq` | NI | 8 XA + 1 XD | 11574.074074 | 32768 (absent) | range 2 V |
| `sglx-test-20210920-0-g0-t0.imec0.ap` | NP 2.0 (type 24) | 384 + SY | 30000 | 8192, 80 | no imChan0apGain |
| `sglx-np2-with-sync.imec0.ap` | NP2013 | 384 + SY | 30000 | 2048, 100 | range 0.62 V |
| `sglx-np2-subset-with-sync.imec0.ap` | NP2013 | 120 + SY | 30000 | 2048, 100 | subset `0:35,72:95,192:227,264:287,384` |
| `sglx-np2-no-sync.exported.imec0.ap` | NP2013 | 384, no SY | 30000 | 2048, 100 | `snsApLfSy=384,0,0` |
| `sglx-5-19-2022-ci1-g0-t0.imec0.ap/lf` | NP 1.0 | 384 + SY | 30000.061088 / 2500.005 | 512, 500/250 | calibrated rates |
| `zenodo5899237 Pt03.imec0.lf` | NP 1.0 (type 0), human intraoperative | 384 LF + SY | 2500 | 512, 250 | SpikeGLX 20201024, 1.1 GB, 1.47 M samples, `~snsShankMap` absent from the LF meta |
| `zenodo21908762 sub-001_ses-001_g0_t0.imec0.ap` | NP2010 (type 24, four shanks) | 384 + SY | 30000 | 8192, 80 | SpikeGLX 20230411, `~snsGeomMap` and `~snsShankMap` both present, all sites on shank 2 |

## Vocabulary (every public identifier in `openreadout-spikeglx` must appear here)

| identifier | meaning |
| --- | --- |
| `session_files`, `open_session`, `SESSION_COMPANIONS` | run directories (sessions) |
| `SpikeGlxReader`, `SpikeGlxDataset`, `FORMAT_ID`, `open`, `meta`, `stream_kind`, `stream_paths`, `looks_like_meta`, `MAX_META_LEN` | entry points |
| `Meta`, `entries`, `get`, `number`, `counts`, `parse_meta`, `list_elements`, `parse_subset` | `.meta` parsing |
| `StreamKind` { `Imec`, `Nidq`, `Obx` }, `name` | stream kinds |
| `ChannelKind` { `Ap`, `Lf`, `Sync`, `Mn`, `Ma`, `Xa`, `Xd` }, `prefix`, `is_bits` | channel kinds |
| `SavedChannel`, `index`, `acquired`, `kind`, `gain`, `scale`, `unit`, `saved_channels` | saved channels |
| `has_selectable_gain`, `fixed_gain`, `default_max_int`, `imec_scale`, `ni_scale` | scaling rules |
| `MAX_READ_VALUES` | read size limit |
| `SiteMap`, `source`, `part_number`, `shanks`, `shank_pitch_um`, `shank_width_um`, `columns`, `rows`, `sites`, `site_map` | probe site maps |
| `ChannelSite`, `shank`, `x_um`, `z_um`, `col`, `row`, `used` | one channel's electrode site |

### Performance / robustness merge (2026-09-23)

Both stream detection and open read at most 16 MiB plus one byte of the `.meta` sidecar, rejecting oversize sidecars without loading them whole.

### Byte-source integration

The existing layouts are read through `Input`/`Fs`/`SourceFile`, including directory
sessions and companions where applicable. Path, memory and callback namespaces use
the same parser and scaling. See [byte sources](../../book/src/guides/wasm.md).
