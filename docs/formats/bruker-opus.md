# Bruker OPUS

Bruker OPUS writes infrared measurements (VERTEX, TENSOR, HTS-XT and Vector-series instruments) as files with a numeric extension. OpenReadout returns one trace per stored spectrum, interferogram or phase, with the instrument and acquisition parameters.

Derived from brukeropus (MIT; read as prior art and used as a reference reader), hex dumps of the corpus files (OPUS 6.5 to 8.x) and comparison between them. brukeropusreader (GPL-3.0) is run as a reference reader (black box). Provenance: `docs/provenance/bruker-opus.md`. Format id `bruker-opus`, family `spectroscopy`, crate `openreadout-spectro` (`OpusReader`, `OPUS_FORMAT_ID`).

An OPUS file is one measurement: a directory of typed blocks — parameter blocks (instrument, optics, acquisition, Fourier transform, sample), data blocks (the spectra, interferograms and phases) each described by a *data-status* parameter block, and a history (file-log) block. Files are named with a numeric extension (`.0`, `.1`, …: OPUS numbers successive saves of one sample). **Detection:** the file starts with `0A 0A FE FE`.

## Header and directory

| offset | type | meaning |
| --- | --- | --- |
| 0 | 4 bytes | `0A 0A FE FE` |
| 4 | f64 | version (920622.0 in every corpus file; reported as `format_version`) |
| 12 | u32 | offset of the directory |
| 16 | u32 | directory slots |
| 20 | u32 | blocks in use |

The directory holds 12-byte records: a 32-bit **type word**, the block size in 4-byte words, the block offset. A record with offset ≤ 0 ends it. The type word splits into six fields (brukeropus's layout, our names): bits 0–1 `part` (1 real part, 2 imaginary part, 3 plain data), 2–3 `role` (1 sample, 2 reference, 3 result/ratio), 4–9 `params` (0 data; 1 data status; 2 instrument; 3 acquisition; 4 Fourier transform; 5 display; 6 optics; 7 GC; 8 library search; 9 communication; 10 sample; 11 lab/process), 10–16 `data` (low 5 bits the data kind, higher bits an extra channel number), 17–18 `derivative`, 19–21 `extended` (2 series, 4 compact, 5 history/report). Bit 30 is set on some blocks and not others of one file; we found no meaning and mask it off. Blocks of type 0 (all fields zero) hold stale copies of parameter blocks in some files and are listed, not used.

**Parameter blocks** are records of a 3-character name (NUL-padded to 4 bytes), a 16-bit type (0 int32, 1 float64, 2–4 text), a 16-bit size in 2-byte words and the value, up to an `END` record. **Data blocks** hold float32 values (`DPF` = 2: int32), at least `NPT` of them (often one or two more); *compact* blocks (`extended` 4) hold a header before the values: the last `NPT` values are the data. **Series** blocks (`extended` 2, 3D data) start with six int32 (version, spectrum count, offset of the first spectrum, spectrum bytes, record bytes, stored-range count) and store each spectrum followed by a 152-byte record (8 int32, 4 f64, 2 int32, 7 f64, 2 int32, 2 f64: scans … start and end time) — layout from brukeropus; no public series file was available, so this path is covered by synthetic tests only.

**Pairing.** A data block is described by the data-status block whose type word equals its own except for `params` = 1. When several status blocks match (a file with two absorbance blocks), the one whose `MNY`/`MXY` equal the block's own minimum and maximum (× `CSF`) wins, then file order.

## Traces

One trace per decoded data block, one sweep (a series block: one sweep per stored spectrum), one channel. **Order:** result spectra (`role` 3), then sample, then reference blocks; within a role, plain spectra before interferograms and phases, then by `extended`, `derivative`, `part`, and **the later block first**: OPUS appends a processed copy of a spectrum after the original, and in the corpus the vendor exports (OPUS data-point table of `opus-orange-peach-juice`) match the later copy.

- **x axis** (`extra.axis`): `linspace(FXV, LXV, NPT)`; `DXU` gives the quantity and unit: `WN` → `wavenumber` in `1/cm`, `MI` → `wavelength` in `µm`, `NM` → `wavelength` in `nm`, `LGW` → `log_wavenumber`, `MIN` → `time` in `min`, `PNT` → `points` (interferograms); other or missing codes → `x`.
- **y values** = stored value × `CSF` (float64 arithmetic); the channel records `scale` = `CSF` and `dtype` `float32` (or `int32`).
- **Names** (trace `name` and channel name/`y_quantity`), by the `data` kind: 1 `single channel` / `single_beam`, 2 `interferogram`, 3 `phase`, 4 `absorbance` (unit `AU`), 5 `transmittance`, 6 `Kubelka-Munk` / `kubelka_munk`, 7 `trace` / `intensity`, 8 `GC interferograms`, 9 `GC spectra`, 10 `Raman` / `raman_intensity`, 11 `emission`, 12 `reflectance`, 14 `power`, 15 `log reflectance`, 16 `ATR` / `atr`, 17 `photoacoustic`, 18/19 arithmetic results, 22 `match`; prefixed `sample`/`reference` by role; suffixes for an extra channel (`(channel 2)`), derivatives, real/imaginary parts and compact copies.
- **`extra.data_type`** (the JCAMP-DX `##DATA TYPE=`): `INFRARED SPECTRUM`, `INFRARED INTERFEROGRAM` for interferograms, `RAMAN SPECTRUM` for Raman blocks, `UV/VIS SPECTRUM` for `DXU` = `NM`.

### `traces[].extra` (our vocabulary)

| key | from | notes |
| --- | --- | --- |
| `axis`, `data_type`, `y_quantity` | data status, block type | above |
| `spectrum_role` | type word | `sample`, `reference`, `result`, `other` |
| `block_type` | type word | the six fields |
| `data_block` | directory | index of the data block |
| `acquired_at` | `DAT` + `TIM` | ISO 8601 with the offset `TIM` records (`08:59:44.322 (GMT+12)` → `+12:00`); `DAT` is `dd/mm/yyyy` or `yyyy/mm/dd` |
| `resolution_cm1` | `RES` | acquisition parameters (reference blocks: the reference acquisition) |
| `scans` | `NSS` (sample/result), `NSR` (reference) | |
| `background_scans` | `NSR` of the reference acquisition | on sample and result traces |
| `laser_wavenumber_cm1` | `LWN` | HeNe or diode reference laser |
| `apodization`, `apodization_name` | `APF` | code as stored; the name for `BX` boxcar, `TR` triangular, `B3`/`B4` Blackman-Harris 3/4-term, `HG` Happ-Genzel, `NBW`/`NBM`/`NBS` Norton-Beer weak/medium/strong, `TP` trapezoidal |
| `zero_filling_factor` | `ZFF` | |
| `phase_correction`, `phase_resolution_cm1` | `PHZ`, `PHR` | |
| `beamsplitter`, `source`, `detector`, `aperture`, `accessory` | `BMS`, `SRC`, `DTC`, `APT`, `ACC` | optics |
| `scanner_velocity` | `VEL` | as stored (text) |
| `measurement_channel` | `CHN` | |
| `instrument`, `instrument_serial`, `instrument_firmware` | `INS`, `SRN`, `VSN` | |
| `sample_name`, `sample_form`, `operator`, `experiment` | `SNM`, `SFM`, `CNM`, `EXP` | sample parameters |
| `result_spectrum` | `PLF` | `AB`, `TR`, `RFL`… |
| `range_high_cm1`, `range_low_cm1` | `HFW`, `LFW` | requested range |
| `duration_s` | `DUR` | |
| `stored_y_min`, `stored_y_max` | `MNY`, `MXY` | |
| `x_units_code` | `DXU` | |
| `series` | series block | `true`; the per-spectrum table (`spectrum`, `scans`, `start_time`, `end_time`) is `tables[]` |

**Experiment.** `sample.id` from `SNM`, `instrument` (vendor Bruker, model `INS`, serial `SRN`, software OPUS), `method.name` from `EXP`, `method.parameters`: `resolution` (cm⁻¹), `scans`, `background_scans`, `apodization` (name and code), `zero_filling_factor`, `laser_wavenumber`, `beamsplitter`, `source`, `detector`, `aperture`, `accessory`, `phase_correction`; `acquisition.started_at` from the first trace's `DAT`/`TIM`, `acquisition.operator` from `CNM`.

**Software version.** The history block's text names the OPUS release that wrote it (`Version 8.1 Build: 8, 1, 29 20180416`; `Version 8.5(SP1) Build: …`); the token after the first `Version ` that is followed by ` Build` becomes trace `extra.software_version` and the experiment's `instrument.software_version` (`history_version`). Files without a history block (or whose history names no version) have none.

**Listing and vendor tree.** `info --view structure` lists every block (`directory`, `parameters` by group, `data`, `data_series`, `history`, `block`) with its offset, size and the trace it feeds. `info --view full` holds the header, every block with its type fields and decoded parameters under their three-letter names, and the history text.

## Validation

- **brukeropus 1.4.3** (MIT) on all 16 fetched OPUS files: every trace's values bit for bit (xxh3 of the float64 values), sample counts, x-axis ends, and the parameters it decodes (`RES`, `LWN`, `APF`, `BMS`, `SRC`, `DTC`, `INS`, `SRN`, `SNM`, `CNM`, `APT`, `ACC`, scans) — 65 traces (`oracle/spectro.py`, `cargo test --features corpus`).
- **Vendor exports:** OPUS's CSV export of `chrysene_003.1` (1869 points, 7 significant digits) and OPUS's data-point table of `peach_juice.0` (1816 points, 5 decimals) agree point by point to the printed precision (`tests/spectro.rs`).
- **brukeropusreader** (GPL-3.0, black box) agrees on the absorbance of files with one absorbance block; where a file has several (a compact copy, match results), it picks a different block (recorded in the oracle JSON, not a failure of either reader).

## Known gaps

3D series (no public file), report blocks (quant, search, multi-evaluation) and embedded bitmaps (`extended` 3/4 blocks without data status: `peach_juice.0` holds a 1 MB microscope image) are listed, not decoded. OPUS files with the directory at a non-default place and more than 4096 directory slots are read up to 4096 blocks.

## Vocabulary (public API of `openreadout-spectro`)

| identifier | meaning |
| --- | --- |
| `OpusReader` | the reader (`FormatReader`): detection by the first bytes, `open`/`open_input` |
| `OPUS_FORMAT_ID` | the format id, `bruker-opus` |
| `SpectroDataset` | an opened file (the `Dataset` the four spectroscopy readers share): traces, tables, map images and attachments read lazily |

Everything else — trace names, channel names, `extra` keys and their values — is listed in the tables above in our own words.
