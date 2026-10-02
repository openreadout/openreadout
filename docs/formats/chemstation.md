# Agilent ChemStation `.D`

Agilent ChemStation saves each HPLC or GC acquisition as a `.D` directory. OpenReadout returns the detector signals as traces, diode-array data as one trace per file with a channel per wavelength, single-quadrupole MS data as spectra, and the vendor's peak report as a table.

Derived from hex dumps of public corpus data sets, then checked against permissively licensed prior art (entab, MIT; Aston, BSD-3-Clause) and rainbow's public documentation pages; rainbow (LGPL-3.0) and Aston are used as reference readers. See `docs/provenance/chemstation.md`. Format id: `chemstation`; crate `openreadout-chrom` (`chemstation_*.rs`).

A ChemStation acquisition is a directory ending in `.D` (or `.d`). Each detector signal is a file of its own: `.ch` (one channel over time: FID, TCD, VWD, MWD/DAD single wavelengths, ADC), `.uv` (diode-array spectra over time), `.ms` (single-quadrupole mass spectra). The directory also holds `.REG` register dumps, `RUN.LOG`, `SAMPLE.XML`/`SAMPLE.MAC`, `acqmeth.txt`, `.ini` files and the method directory `*.M/`. OpenReadout opens either the directory or a single signal file.

We expose one **trace** per `.ch` file (one channel), one trace per `.uv` file (one channel per wavelength), and one **spectra run** per `.ms` file. Every other file is listed by `info --view structure` with its size and version string; text files (`.txt`, `.xml`, `.ini`, `.log`, `.mac`, `.mth`, `.csv`) are copied into the `info --view full` vendor tree (first 16 KiB).

## Version string (all files)

Byte 0 is a length (1–3), followed by that many ASCII digits: the file's **version**. Seen: `2` (`.ms`), `30`, `81`, `130`, `179`, `181` (`.ch`), `131` (`.uv`); register (`.REG`) files start with a version string as well. Detection (`looks_like_chemstation`): versions below 100 must have a length-prefixed type string ending in `DATA FILE` (`GC DATA FILE`, `LC DATA FILE`), or `MSD Spectral File`/`GC / MS Data File`, at 0x04 (or a zero byte after the digits, for register files); versions ≥ 100 repeat the version as UTF-16 at 0x146. A `.d` directory is ours when at least one top-level `.ch`/`.uv`/`.ms` file passes this test (Agilent MassHunter `.d` directories, which hold `AcqData/`, do not).

## Header

The header length is a big-endian u32 at 0x108 counted in 512-byte blocks plus one (`3` → 1,024 bytes, `13` → 6,144 bytes; `(n − 1) × 512`). For `.ms` files it is a big-endian u16 at 0x10A in 16-bit words plus one (`257` → 512 bytes).

### Strings

Versions below 100: one length byte, then single-byte characters (read as Latin-1, trimmed). Versions 130/131/179/181: one length byte (in characters), then UTF-16LE code units.

| our name | versions < 100 | 130/131/179/181 | example |
| --- | --- | --- | --- |
| `file_type` | 0x04 | 0x15B | `GC DATA FILE`, `LC DATA FILE`, `MSD Spectral File` |
| `sample_name` | 0x18 | 0x35A | `Cytochrome C`, `BB7125_3-spiropyrrolidine_cof` |
| `description` | 0x56 | – | (spaces in the corpus) |
| `operator` | 0x94 | 0x758 | `RJB`, `SYSTEM` |
| `acquired_text` | 0xB2 | 0x957 | `18-Nov-10, 15:48:06`, `8/20/20 2:00:32 PM`, `28 Jun 13  10:59 am -0500` |
| `instrument_model` | 0xD0 | 0x9BC | `HP G1530A`, `G1365B`, `GCI` (a generic GC module tag, not a model), `DAD1` |
| `separation` | 0xDA | 0x9E5 | `GC`, `LC` |
| `method` | 0xE4 | 0xA0E | `EVAL.M`, `RJB-TEST.M` |
| `instrument_name` | – | 0xC11 | `Asterix ChemStation` |
| `software` / `software_revision` | – | 0xE11 / 0xEDA | `A.01.15` / `Rev. C.01.07 SR2 [2...` |
| `units` | 0x244 | 0x104C (0xC15 in `.uv`) | `pA`, `mAU`, `25 uV` |
| `signal` | 0x254 (0x140 in `.ms`) | 0x1075 | `FID1A, Front Signal`, `MWD A, Sig=210,5 Ref=360,100`, `MSD1, Initial Scan Range=100.0-1000.0` |
| `vial_text` | – | 0xFD7 | `5` (equals the u16 at 0xFE) |

Every other length-prefixed string in the header is found by a scan (a length byte after a zero byte, followed by printable characters and a zero) and listed in `info --view full` under its hex offset. `parse_date` turns `acquired_text` into ISO-8601 (`acquired_at`); two-digit years < 70 are 20xx; a trailing `±hhmm` becomes the zone.

`wavelengths` reads the detector wavelength and bandwidth from `Sig=210,5` and the reference from `Ref=360,100` (`Ref=off` gives none) → trace `extra.wavelength_nm`, `bandwidth_nm`, `reference_wavelength_nm`, `reference_bandwidth_nm`.

### Numbers (big-endian)

| offset | our name | type | notes |
| --- | --- | --- | --- |
| 0xF8 | – | u32 | the version again as a number (81, 179, 181) |
| 0xFC / 0xFE / 0x100 | `sequence_line` / `vial` / `replicate` | u16 | names from entab (prior art); `vial` corroborated by the 0xFD7 string |
| 0x116 | `declared_records` | u32 | scan count in `.uv` files and LC-MS `.ms` files (GC-MS `.ms`: little-endian u16 at 0x142); not a count in `.ch` files |
| 0x11A / 0x11E | `first_time_ms` / `last_time_ms` | f32 (81, 179, 181), i32 (30, 130) | retention time in milliseconds |
| 0x27C / 0x284 | `offset` / `scale` | f64 | versions 30/81: `value = raw × scale + offset` (offset 0 in every corpus file) |
| 0x127C | `scale` | f64 | versions 130/179/181 |
| 0xC0D | `scale` | f64 | version 131 (`.uv`) |

Scales seen: 1/7,680 (FID pA), 1,000/2²¹ (MWD/DAD mAU).

## Bodies (`BodyEncoding`)

| encoding | versions | layout |
| --- | --- | --- |
| `DeltaRecords` | 30, 130 | records: tag byte `0x10`, count byte `n`, then `n` big-endian i16 first differences; `0x8000` is followed by a big-endian i32 absolute value. The body ends with two zero bytes (`Terminator`). |
| `SecondDifference` | 81, 181 | big-endian i16 second differences (`d += w; value += d`); `0x7FFF` is followed by a 48-bit big-endian absolute value (i32 high word, u16 low word: `high × 65,536 + low`) and resets `d` to 0. Version 181 ends with one `0x0000` word, which is a terminator, not a sample. |
| `Float64` | 179 | little-endian f64 values to the end of the file |
| `SpectrumRecords` | 131 | per scan: u16 tag 67, u16 record length, u32 time (ms), u16 first/last/step wavelength × 20, 8 unknown bytes, then first differences (little-endian i16, `0x8000` → i32 absolute), starting from 0 in each record; or (OpenLab CDS spectra parts) tag 70 with little-endian f64 values after the same 22-byte header, value = f64 × scale × scale |
| `MassSpectrumRecords` | 2 | per scan: u16 length in 16-bit words, u32 time (ms), 6 bytes, u16 pair count, 4 bytes, pairs of u16 m/z × 20 and u16 packed intensity (14-bit mantissa × 8^(top two bits)), then 10 bytes whose last u32 is the scan's TIC; all big-endian |
| `NotDecoded` | others | listed only |

`BodyEnd` records how a walk ended: `Terminator`, `EndOfFile`, `Truncated { at }` or `BadRecord { at, detail }`.

### Time axis

Traces are regular: `start_s` = first time; the interval is `(last − first) / (n − 1)` for versions 30/130/131/179/181 and `(last − first) / n` for version 81 (both corpus version-81 files span exactly `n` intervals of 200 ms or 100 ms). For `.uv` files the times come from the records. `extra.axis` = `{quantity: retention_time, unit: min, first, step}`; `extra.x_start_min` / `x_end_min` are the header times in minutes.

## Mapping

- `traces[]`: `.ch` → name = the `signal` string (else the file stem); one channel named after the file stem (`FID1A`, `mwd1A`), `unit` from the header, `dtype` = raw storage (`int32` for delta records, `int64` for second differences, `float64`), `scale`/`offset` from the header. `.uv` → name `<stem> spectra`, one channel per wavelength (`200 nm`, …, `extra.wavelength_nm`), `extra.wavelength_range_nm` = `[first, last, step]`; records on another wavelength grid are placed by wavelength (`NaN` elsewhere) and `extra.wavelength_grid_varies` is set.
- `extra` (both): `file`, `format_version`, `encoding`, `file_type`, `sample_name`, `description`, `method`, `operator`, `acquired_at`, `acquired_text`, `instrument`, `instrument_name`, `separation`, `software`, `software_revision`, `signal`, `vial`, `detector` (letters that start the file stem's last part after `-`, `_`, `.` or a space: `FID`, `MWD`, `DAD`, `VWD`, `TCD`), `result_modules` (from `Result.xml`), wavelengths, `x_start_min`, `x_end_min`, `axis`.
- `extra.method_file` (both, `.D` directories with an `acqmeth.txt`): the method report ChemStation writes, parsed by `parse_acqmeth` (§ Acquisition method text).

### Acquisition method text (`acqmeth.txt`)

Latin-1 text with CRLF lines (read up to `MAX_METHOD_TEXT`, 1 MiB). Only `mtbls75-x-fsfa-hl-gc-o7c-1-d` (GC-MSD) has one in the corpus; no LC `.D` does, so no pump timetable is parsed. Recognized lines, anywhere in the text:

| text | `method_file` key |
| --- | --- |
| `INSTRUMENT CONTROL PARAMETERS:    GCMSD_1` | `instrument_name` |
| a line holding only a path ending in `.M` (`C:\MSDCHEM\1\METHODS\PNNL_METABOLOMICS.M`) | `method_path` |
| `Oven Program On`, then `60 °C for 1 min`, `then 10 °C/min to 325 °C for 10 min`, … | `oven_program[]` {`temperature_c`, `hold_min`, `rate_c_per_min` (ramps)} |
| the `Run Time 37.5 min` line right after the oven program | `run_time_min` |
| `Injection Volume 1 µL` | `injection_volume_ul` |
| `Mode Splitless` right after an `… Inlet …` heading | `inlet_mode` |
| `Column #1`, its name line, `325 °C: 30 m x 250 µm x 0.25 µm` | `column` (`HP-5MS 5% Phenyl Methyl Silox, 30 m x 250 µm x 0.25 µm`) |
| `Solvent Delay : 6.50 min`, `Low Mass : 50.0`, `High Mass : 600.0`, `Acquistion Mode : Scan` (sic) | `solvent_delay_min`, `low_mass`, `high_mass`, `ms_acquisition_mode` |
| `TUNE PARAMETERS for SN: US92032548` | `ms_serial` (the MSD's serial number) |

The experiment model maps `oven_program`, `run_time_min` (as `method_length`), `injection_volume_ul`, `column`, `inlet_mode` and `ms_serial` (`instrument.serial`). The instrument model skips names that are configured instrument names, not models: the default `Instrument 1` (cut to `Instrumen` in `.MS` headers, `entab-carotenoid-extract-d`) and module instances named like their signal files (`DAD1`, `zhulong-001-1-sm-d`); the next candidate (the `.uv` header's `G1315B`) is used, or none. The first `Result.xml` module's name (`Agilent 7890A`) comes before the signal headers' string and its serial number is `instrument.serial`; the generic GC tag `GCI` is never a model.
- `spectra[]`: `.ms` → `scan_count`, `rt_range_s`, `ms_levels` `[1]`, `extra.max_pairs_per_scan`; `read_spectrum` returns the pairs in ascending m/z (files store them descending), `total_ion_current` from the record trailer, `polarity` `unknown`, `centroided` false.

## Vendor peak reports (`tables[0]` `vendor_peaks`, 2026-09-26)

Read from the top level of a `.D` directory (at most `MAX_REPORT_BYTES`, 4 MiB each):

- **`Result.xml`** (LC/GC ChemStation's XML export, UTF-16; preferred over `Report.TXT`, which prints the same integration): root `ChemStationResult`; `Chromatograms/Signal` (`Description` `FID1 A, Front Signal` → signal `FID1 A`, `YUnits` → area unit `pA*s`, height unit `pA`) with one `IntegrationResults` per peak (`RetTime`, `Area`, `AreaPercent`, `Height`, `Width`, `Symmetry`, `TimeStart`, `TimeEnd`, `BaselineStart`, `BaselineEnd`: the integration limits and the vendor's baseline value at each), and `Results/ResultsGroup/Peak` compound results (`SignalDesc`, `MeasRetTime`, `PeakType`, `Name`, `Amount` with its `Unit`, `CompoundID`), joined to the integration result of the same signal and retention time (a compound result with none is counted in `rows_skipped`). `CalibrationInformation` (levels, curves) stays in the file.
- **`Report.TXT`** (LC/GC ChemStation's printed report; UTF-16LE with a BOM, or 8-bit): after each `Signal <n>: <detector> <letter>, …` line, a table of two header lines, a separator of dash runs each closed by `|`, and rows up to `Totals :` or a blank line. The separator gives the column extents (the `|` belongs to the column on its left); the header lines cut at the same extents give each column's name and its unit in brackets. Columns used by name: `Peak #`, `RetTime [min]`, `Type`, `Width [min]`, `Area [unit]`, `Height [unit]`, `Area %`, `Amount`, `Name`; others stay in `extra.reports[].columns`. The signal `FID1 A` names the trace of `FID1A.ch` (spaces removed, case ignored).
- **`RESULTS.CSV`** (MSD ChemStation): INI-like; each `[INT <signal>]` section has `Header=,"Peak","R.T.","First","Max","Last","PK  TY","Height","Area","Pct Max","Pct Total"` and `<n>=,` rows. `First`/`Max`/`Last` are 1-based scan numbers. `INT TIC: <run>\data.ms` names the TIC of that `.ms` file (`TIC data.ms`).

Columns: `signal` (code into `extra.categories`), `peak`, `rt_min`, `peak_type` (code: `BB`, `BB S`, `M2`, …), `width_min`, `area`, `height` (unit when every table states the same one: `pA*s` / `pA`), `area_pct`, `amount` (its unit when all amounts share one, e.g. `% v/v`), `compound` (code), `first_scan`, `max_scan`, `last_scan`, `start_min`, `end_min`, `baseline_start`, `baseline_end`, `symmetry` (NaN where the report has no such column). `extra.source` `vendor`; `extra.reports[]`: `file`, `signal`, `signal_text`, `area_unit`, `height_unit`, `columns` (as written), `peaks`, `rows_skipped` (rows that did not parse as peaks).

`check` finds every reported peak above 1 % of its table's largest on our decoded signal: our maximum within half the reported width (at least one sample) of the retention time, not at the window edge, and our height within 10 % of the reported one — above the vendor's baseline when `Result.xml` gives it, else above a line through the trace 3 widths each side; with `Result.xml` also the area, our trace integrated between the vendor's limits above its baseline, within 10 %; `RESULTS.CSV` — the apex scan's time within one scan of the retention time and its TIC within 10 % of the reported height. Peaks whose type carries a flag after its two baseline letters (`BV E`, `VV R`: skimmed or special baselines) are not compared. When more than a quarter of the compared peaks differ (a wrong scale, unit or signal moves them all) `check` warns `vendor_peak_mismatch`; fewer are reported as `vendor_peak_outliers` / `vendor_area_outliers` (info: a neighbour inside the window in a crowded region). The MSD `Area` is the integrator's corrected area (about 3.6× our sum of TIC counts, depending on the peak type) and is not compared.

Checked on the corpus (`tests/chemstation_reports.rs`): 13 `Report.TXT` peaks (4 GC runs, FID and TCD), 131 `Result.xml` peaks (2 GC runs, FID and TCD; 22 named compounds with amounts) and 46 `RESULTS.CSV` peaks (12 GC-MSD runs) read and found on our signals — the `Result.xml` areas recomputed between the vendor's limits agree to a median ratio of 0.9999 (66 peaks above 1 % of the largest area; the few that differ are skimmed or in a crowded region and are reported as info); the runs' `tic_front.csv` TIC exports equal our TIC at every one of 24,144 scans.

## `check` finding codes

`bad_header_length`, `truncated`, `bad_record` (errors); `unknown_version`, `no_samples`, `bad_time_range`, `scan_count_mismatch`, `time_not_monotonic`, `vendor_peak_mismatch` (warnings); `no_terminator`, `time_range_mismatch`, `wavelength_grid_changes`, `vendor_report_rows_skipped`, `vendor_peak_outliers`, `vendor_area_outliers` (info).

## Vocabulary (every public identifier in `chemstation_*.rs` must appear here)

| identifier | meaning |
| --- | --- |
| `ChemStationReader`, `ChemStationDataset`, `CHEMSTATION_ID` | format reader, opened `.D` directory or file, the id `chemstation` |
| `open`, `signals`, `files`, `is_chemstation_dir`, `count` | open a directory/file; its decodable signal files; every file; directory test; samples or scans in a body |
| `DirFile`, `rel`, `size`, `version` | one file of the directory: relative path, bytes, version string |
| `SignalFile`, `path`, `header` | one decodable file: absolute path and parsed header |
| `MAX_METHOD_TEXT`, `parse_acqmeth`, `method_file` | `acqmeth.txt` method report |
| `ReportPeak`, `number`, `rt_min`, `peak_type`, `width_min`, `area`, `height`, `area_pct`, `amount`, `first_scan`, `max_scan`, `last_scan` | one peak of a vendor report (§ Vendor peak reports) |
| `ReportTable`, `source`, `area_unit`, `height_unit`, `columns`, `peaks`, `skipped`, `reports` | one signal's table of a report (`Report.TXT` / `RESULTS.CSV`); a data set's reports |
| `ResultModule`, `name`, `serial`, `firmware`, `part_number`, `parse_result_modules`, `result_modules` | one instrument module of `Result.xml` (`ModuleInformation/Module`: `ModuleName`, `SerialNumber`, `FirmwareRevision`, `PartNumber`); given as `extra.result_modules` |
| `parse_report_txt`, `parse_results_csv`, `parse_result_xml`, `decode_text`, `MAX_REPORT_BYTES` | report parsers; UTF-16 (BOM) or 8-bit text; largest report read |
| `start_min`, `end_min`, `baseline_start`, `baseline_end`, `symmetry`, `amount_unit`, `compound_id`, `blank`, `flagged` | `Result.xml` peak fields; an empty peak; a peak type with a flag (skimmed/special baseline) |
| `SignalHeader`, `kind`, `encoding`, `header_len`, `file_type`, `sample_name`, `description`, `operator`, `acquired_text`, `acquired_at`, `instrument_model`, `separation`, `method`, `instrument_name`, `software`, `software_revision`, `units`, `signal`, `vial`, `sequence_line`, `replicate`, `first_time_ms`, `last_time_ms`, `scale`, `offset`, `declared_records`, `strings` | header fields (tables above) |
| `parse`, `wavelengths`, `to_json`, `name` | parse a header; `Sig=`/`Ref=` wavelengths; header as JSON; an encoding's JSON name |
| `version_of`, `looks_like_chemstation`, `parse_date`, `DECODED_VERSIONS` | version string at byte 0; detection; ChemStation dates to ISO-8601; versions whose body is decoded |
| `SignalKind` { `Chromatogram`, `Spectra`, `MassSpectra`, `Other` } | what a file holds |
| `BodyEncoding` { `DeltaRecords`, `SecondDifference`, `Float64`, `SpectrumRecords`, `MassSpectrumRecords`, `NotDecoded` } | body layouts (table above) |
| `BodyEnd` { `Terminator`, `EndOfFile`, `Truncated`, `BadRecord` }, `at`, `detail` | how a body walk ended |
| `DecodedSignal`, `values`, `escapes`, `records`, `end` | decoded `.ch` body: raw values, absolute-value escapes, delta records walked, how it ended |
| `decode_delta_records`, `decode_second_difference`, `decode_float64` | `.ch` body decoders |
| `ScanRecord`, `len`, `time_ms`, `wavelength_raw`, `tic`, `float` | a located `.uv`/`.ms` record: file offset, bytes, time, wavelength range × 20, values or pairs, stored TIC, f64 values (tag 70) |
| `UV_TAG_DIFFS`, `UV_TAG_FLOAT` | `.uv` record tags: 67 (16-bit differences), 70 (f64 values, scaled by the header scale twice; OpenLab CDS spectra parts) |
| `UV_RECORD_HEADER`, `MS_RECORD_HEADER`, `MS_RECORD_TRAILER` | record header/trailer sizes (22, 18, 10 bytes) |
| `uv_record`, `uv_values`, `ms_record`, `ms_pairs`, `ms_intensity` | record header parsers and value decoders; packed MS intensity |
