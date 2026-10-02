# Magritek Spinsolve benchtop NMR

Magritek Spinsolve benchtop NMR spectrometers save each acquisition as an experiment directory of parameter files and Prospa data files. OpenReadout returns the FIDs and the other data files as traces with the acquisition parameters, and can process FIDs into spectra ([NMR processing](nmr-processing.md)). Derived from nmrglue's Spinsolve reader (`nmrglue/fileio/spinsolve.py`, BSD-3-Clause), read as prior art and used as the reference reader, and from hex dumps of public experiment directories (Spinsolve 1.41.20 to 2.02.16; 43, 60, 62 and 80 MHz), the depositors' CSV exports and the spectra the Spinsolve software stored in its plot files. Provenance: `docs/provenance/magritek-spinsolve.md`. Format id `magritek-spinsolve`, family `nmr`, extensions `.1d`, `.2d`.

An **experiment directory** is one acquisition: `<yymmdd-hhmmss> <protocol> (<suffix>)/` with

| file | content | read |
| --- | --- | --- |
| `acqu.par` | acquisition parameters, `name = value` lines | yes (`ParFile`) |
| `proc.par` | processing parameters (Spinsolve Expert), same syntax | yes |
| `processing.script` | a MestReNova script: `Phase(p0, p1); Zoom(a, b);` | `Phase` only |
| `data.1d`, `fid.1d`, `data.2d`, … | Prospa data files: the FID or a series of FIDs | yes |
| other `*.1d` (`spectrum.1d`, `DiffusionPlot.1d`, …) | Prospa data files the protocol wrote | yes, one trace each |
| `*.pt1`, `*.pt2` | Prospa plot files (a display: fonts, labels, then the plotted arrays) | listed, not decoded |
| `ppCode/*.mac`, `*.par` | the protocol's macros | listed |
| `data.mnova`, `acqu.par.bak`, `*.csv`, `*.png` | MestReNova document, backups, exports | listed |

The input may be the directory or any of these files. A folder holding several experiment directories (up to four levels down) is recognised and listed (usage error naming them). Sub-experiments (the T1–T2 protocol's `1D_T1IRT2/0000/…`) are opened one by one.

## Parameters (`ParFile`, `parse_par`, `parse_par_value`)

One `name = value` per line; values are a quoted text, a number, a `[a,b]` list of numbers, or else the text as written (`2.00.27 Alpha`). A line without `=` is reported by `check` (`parameter_syntax`). The parameters used:

| name | meaning | ours |
| --- | --- | --- |
| `bandwidth` | spectral width, kHz | `spectral_width_hz` = × 1000 (nmrglue) |
| `b1Freq` | frequency of 0 ppm, MHz | `reference_frequency_mhz` (nmrglue's observe frequency; the software's ppm axis is (`lowestFrequency` + k·width/n)/`b1Freq`, verified on its plot files) |
| `lowestFrequency` | lowest frequency of the spectrum, Hz from `b1Freq` | `carrier_offset_hz` = `lowestFrequency` + width/2 (nmrglue), `carrier_offset_ppm`; `spectrometer_frequency_mhz` = `b1Freq` + offset·10⁻⁶ (the carrier) |
| `b1Freq1H` | proton frequency of the magnet, MHz | `proton_frequency_mhz` |
| `nucleus`, `rxChannel` | observed nucleus (`1H`, `19F`), receiver channel | `nucleus`, `receiver_channel` |
| `nrPnts`, `nrScans`, `nrSteps` | points per FID, scans, series steps | `time_domain_size`, `scans`, `steps` |
| `dwellTime`, `acqTime`, `repTime`, `duration` | µs, ms, ms, s | `dwell_time_us`, `acquisition_time_ms`, `repetition_time_ms`, `duration_s` |
| `rxGain`, `rxPhase` | receiver gain, phase (degrees) | `receiver_gain`, `receiver_phase_deg` |
| `experiment`, `expName` | protocol (the `ppCode/<experiment>.mac` pulse sequence), folder name | `pulse_program`, `experiment_name` |
| `specType`, `specID` | instrument model (`C43`, `C60Ultra`, `C80Ultra`, `P60Grad`), serial (`SPA…`) | `instrument`, `instrument_serial` (inferred: they track the magnet frequency of each file) |
| `softwareVersion` | software version | `software_version`; `software` = `Spinsolve`; `format_version` = `Spinsolve <version>` |
| `filter`, `filterType` | apodization on (`yes`/`no`) and its function (`exp:0.5`) | `apodization` when `filter` is `yes` |
| `zf` | zero-filling factor | transform size `nrPnts` × `zf` (1 verified) |

`expName` (`split_exp_name`) gives `acquired_at` (`20yy-mm-ddThh:mm:ss`, the spectrometer's local clock, no zone) and `sample_name` (the suffix in parentheses, when not empty). Inferred from the folder rule visible in the protocol macros' text; every public file follows it.

## Prospa data files (`ProspaHeader`, 32 bytes, little-endian)

| offset | bytes | our name | meaning |
| --- | --- | --- | --- |
| 0 | 8 | `DATA_MAGIC` | `SORPATAD`: the words `PROS`, `DATA` stored as little-endian u32 (plot files: `PROS`, `PLD1`/`PLD2`; `PROSPA_MAGIC` = `SORP`) |
| 8 | 4 | `version` | `V1.1`, byte-reversed the same way (`version_text`) |
| 12 | 4 | `data_type` | below |
| 16 | 4 × 4 | `dims` | x, y, z, q sizes (`points`, `rows` = y·z·q with 0 counted as 1) |

`HEADER_BYTES` = 32. Values follow as float32, row after row (`row_bytes`, `declared_len`):

| type | our name | row layout | seen in |
| --- | --- | --- | --- |
| 501 | `TYPE_COMPLEX` | n interleaved (real, imaginary) pairs, no x values | every `data.1d`/`data.2d` of software 2.0x, `1D_T1IRT2/*/data.1d` |
| 503 | `TYPE_XY_REAL` | n x values, then n real values | `DiffusionPlot.1d` (equal to the depositor's `DiffusionPlot.csv`) |
| 504 | `TYPE_XY_COMPLEX` | n x values, then n interleaved pairs | `data.1d` of software 1.41 (x = time in ms), `DiffusionSpectrumStacked.1d` (x = ppm; equal to its CSV) |

nmrglue takes the first third of every `.1d` file as the x axis; that holds for type 504 only (a 501 file is exactly 32 + 8·n bytes). Other type codes are listed with a note and not decoded (`problems`); so are multi-row files with an x block (not seen; their layout is unknown).

**The x block** has no unit in the file. It is recognised (`XAxis`) when evenly spaced and its step equals `dwellTime`/1000 (`TimeMs`) or its span equals `bandwidth`·1000/`b1Freq` (`Ppm`); it then becomes `extra.axis` and is not returned as a channel. Otherwise it is channel `x` (unit unknown), e.g. the CPMG echo times of `T2Bulk`'s `data.1d` and the gradient axis of `DiffusionPlot.1d`.

## Traces and `extra`

One trace per decodable data file (`DataFile`), named by the file: the FID first (`FID_NAMES` in order: `data.1d`, `fid.1d`, `data.2d`, `data.3d`, `data.4d`, when complex and x size = `nrPnts`), then the others by name.

- **FID**: `kind` `time_domain`, channels `real`, `imag` (float32 as stored), `sweep_count` = rows (the `nrSteps` series of relaxation, SLIC and diffusion protocols, in file order), `sample_rate_hz` = width, `axis` time in s stepping 1/width, and the parameters above. `data_type`, `dimensions`, `file` on every trace.
- **ppm spectra** (`spectrum.1d`, `DiffusionSpectrumStacked.1d`): `kind` `spectrum`, `axis` `{quantity: chemical_shift, unit: ppm, first, last, step, size, spectrometer_frequency_mhz}` (= `b1Freq`), channels `real`, `imag`.
- **Other arrays**: channels `x` (when not an axis), `real`/`imag` or `y`.

Values are returned as stored. The frequency sense is the vendor's: the Spinsolve software's spectrum at ascending ppm point k is the FFT of the **conjugated** FID at frequency index k − n/2.

## FID processing (`openreadout-signal`, `docs/formats/nmr-processing.md`)

Conjugate the FID; no group delay; first point × 0.5; no apodization when `filter` is `no` (else `exp:X` as X Hz exponential, unverified); size `nrPnts` × `zf`; stored phases `p0Phase`, `p1Phase` of `proc.par`, else −`p0`, −`p1` of `Phase(p0, p1)` in `processing.script` (`script_phase`; the software multiplies by e^{+i·p0}); reference `b1Freq`. Validated on 135 FID rows against the software's own processed spectra (below).

## Validation

- Raw values: `oracle/spinsolve.py` (nmrglue for the parameters, the header and single-row type 504 data; the documented layout for 501/503, cross-checked against the depositors' CSV exports) → `corpus/oracle/<id>.json`, 26 inputs, every sweep's xxh3 equal (`tests/corpus/`).
- Processing (`tests/nmr_processing.rs`, `spinsolve_fid_processing_matches_the_software`): 135 rows of 9 experiments whose directory holds the software's processed spectra (`spectrum.pt1`, `*-Spectra.pt1`; the arrays are located by `oracle/spinsolve.py` from their ppm axis): the ppm axis point for point within 1e-5 ppm (measured ≤ 9·10⁻⁶, float32), correlation of the real part with the software's ≥ 0.9999999 on every row with the software's phase (1D proton: largest difference 2·10⁻⁷ of the maximum; series rows ≤ 1.2·10⁻²); automatic phasing ≥ 0.95 (signal points) on phased single-sign rows, rows near the inversion null reported only.

## `check` finding codes

`bad_header`, `truncated` (errors); `parameter_syntax`, `extra_bytes`, `rows_mismatch` (nrSteps against the FID's rows), `unreadable_data`, `missing_parameters` (warnings); `missing_parameter`, `no_fid` (info).

## Observed corpus values

| id prefix | software | instrument | data |
| --- | --- | --- | --- |
| `spinsolve-zenodo15131439-*` (10) | 2.01.19 | C43, 43.45 MHz | `data.1d` 501, 4096 points, 5 kHz |
| `spinsolve-zenodo20597567-*` (10) | 2.02.16 | C60Ultra, 60.2–62.4 MHz | `data.2d` 501, 16384–32768 × 16–20, `*-Spectra.pt1` |
| `spinsolveproc-proton` | 1.41.20 | C80Ultra, 80.49 MHz | `data.1d` 504, 8192 points, `spectrum.pt1`, `proc.par` |
| `spinsolveproc-t1`, `-t2`, `-t2bulk`, `-t1irt2-0000` | 1.41.20 | C80Ultra | series 501; CPMG echoes 504 (x channel), 501 |
| `spinsolveproc-pgste` | 2.00.27 Alpha | P60Grad, 61.94 MHz | `data.2d` 512 × 512, `DiffusionPlot.1d` 503, `DiffusionSpectrumStacked.1d` 504 (ppm) |

## Vocabulary (every public identifier in `crates/openreadout-nmr/src/spinsolve.rs` must appear here)

| identifier | meaning |
| --- | --- |
| `SpinsolveReader`, `SpinsolveDataset`, `SPINSOLVE_FORMAT_ID`, `open` | reader entry points: format reader, opened directory (core `Dataset`), the id `magritek-spinsolve` |
| `spectral_width_hz`, `reference_frequency_mhz`, `carrier_offset_hz` | acquisition axis values of an opened directory |
| `resolve_spinsolve_dir_in`, `find_spinsolve_experiments_in` | the experiment directory of a path; experiment directories under a folder |
| `ProspaHeader`, `DATA_MAGIC`, `PROSPA_MAGIC`, `HEADER_BYTES`, `parse`, `version`, `data_type`, `dims`, `version_text`, `points`, `rows`, `point_bytes`, `is_complex`, `has_x`, `row_bytes`, `declared_len`, `problems` | the 32-byte data file header |
| `TYPE_COMPLEX`, `TYPE_XY_REAL`, `TYPE_XY_COMPLEX` | data type codes 501, 503, 504 |
| `FID_NAMES` | FID file names in order of preference |
| `ParFile`, `params`, `issues`, `get`, `num`, `text`, `to_json`, `parse_par`, `parse_par_value` | `acqu.par`/`proc.par` |
| `script_phase` | `Phase(p0, p1)` of `processing.script` |
| `split_exp_name` | start time and suffix of `expName` |
| `XAxis` { `TimeMs`, `Ppm` }, `first`, `step` | a recognised x block |
| `DataFile`, `name`, `header`, `len`, `fid`, `axis` | one data file of the directory |
