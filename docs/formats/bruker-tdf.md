# Bruker timsTOF (TDF / TSF `.d`)

Bruker timsTOF instruments write each run as a `.d` directory: TDF for trapped-ion-mobility data, TSF for runs without mobility (including MALDI). OpenReadout returns the spectra (MS1 frames, DDA-PASEF precursors, DIA-PASEF windows), TIC and BPC traces and a run summary, with m/z and 1/K0 converted by the file's own calibration models (`MzCalibration`, `TimsCalibration`). prm-PASEF frames are returned whole.

Derived from the public SQLite file-format document and the documentation and source of timsrust (Apache-2.0, crate; MIT, repository). Frames and spectra are validated bit for bit against timsrust run as a black box; m/z and 1/K0 against vendor-calibrated conversions (ProteoWizard with Bruker's library, used as a black box) in `crates/openreadout-corpus-tests/tests/mz_agreement.rs`. No Bruker SDK, header or library was used (`docs/provenance/bruker-tdf.md`). Format id `bruker-tdf`.

## Layout

A timsTOF acquisition is a directory, conventionally `NAME.d`:

| file | content |
| --- | --- |
| `analysis.tdf` | SQLite database: `GlobalMetadata`, `Frames`, `Precursors`, `PasefFrameMsMsInfo`, `DiaFrameMsMsInfo`, `DiaFrameMsMsWindows`, `MzCalibration`, `TimsCalibration`, `Segments`, `Properties`, … |
| `analysis.tdf-wal` | SQLite write-ahead log, present when the acquisition software did not checkpoint (seen with `ClosedProperly = 0`): it holds committed pages newer than the database file |
| `analysis.tdf_bin` | frame blobs; `Frames.TimsId` is the byte offset of each |
| `analysis.tsf`, `analysis.tsf_bin` | the same for TSF data (no mobility dimension, one line spectrum per frame) |

The reader accepts the directory itself or any of these files (it opens the directory around it). Detection: a directory (or a file named `analysis.tdf`/`analysis.tsf`/`…_bin`) that holds `analysis.tdf` or `analysis.tsf`.

## SQLite (our reader, `sqlite.rs`)

Read-only, from the SQLite file-format document: 100-byte header (page size, reserved bytes, UTF-8 only), table b-trees (interior `0x05`, leaf `0x0d`) and index b-trees (`0x02`, `0x0a`; how `WITHOUT ROWID` tables such as `DiaFrameMsMsWindows` and real-data `PasefFrameMsMsInfo` are stored), record serial types, overflow pages, `sqlite_schema` on page 1, column names from each table's `CREATE TABLE` text, `INTEGER PRIMARY KEY` as the rowid, `WITHOUT ROWID` records stored as primary-key columns first. A `-wal` file is applied when its header checksum and page size are valid: frames are accepted while their salts match and the cumulative checksum (big- or little-endian words by the header magic) verifies, and only frames up to the last commit frame count; the commit frame's database size replaces the file's page count. The database is never written (SQLite itself would checkpoint the log; this reader cannot). Table names are matched case-insensitively (`GlobalMetaData` in synthetic data).

## Frame blobs (TDF, `TimsCompressionType` 2)

At `TimsId`: `u32` total byte count (header included), `u32` scan count, then one zstd frame. The decompressed bytes are four byte planes of `n` little-endian `u32` values (value `i` = bytes `i`, `n+i`, `2n+i`, `3n+i`). Value 0 is the scan count `S`; values 1…S−1 are twice the peak count of scans 0…S−2 (the last scan takes the rest); then `(TOF delta, intensity)` pairs. Within a scan the TOF index is the running sum of deltas minus one. A blob with no payload is a frame without peaks. The peak and scan counts must equal `Frames.NumPeaks` and `Frames.NumScans` (`check`).

## Frame blobs (TDF, `TimsCompressionType` 1: first-generation timsTOF, LZF)

At `TimsId`: `u32` total byte count, `u32` scan count, then `scans + 1` `u32` offsets counted from the blob start (header included); scan `k`'s LZF stream runs between offsets `k` and `k + 1` (equal offsets: an empty scan). A decompressed scan is a run of little-endian `i32`: a positive value is the intensity at the current TOF index (starting at 0), which then advances by one; zero or `−k` skips `k` indices. The blob holds more scans than `Frames.NumScans` in both corpus files (985 vs 350, 2,742 vs 274); `NumPeaks` and `SummedIntensities` count all of them, so every stored scan is read (and `check` does not compare the scan count for type 1). LZF is the public liblzf scheme (`openreadout_codecs::lzf_decode`).

**TSF blobs:** `u32` total byte count, `u32` byte length `L` of the line-spectrum zstd frame, that frame (`NumPeaks` `f64` fractional TOF indices, then `NumPeaks` `f32` intensities: line spectra, `HasLineSpectra = 1`), then, when `HasProfileSpectra = 1`, a second zstd frame to the end of the blob: `DigitizerNumSamples` `u32` intensities in four byte planes, sample `i` at TOF index `i` (the profile spectrum). Validated on two TSF files (TimsCompressionType 3; ESI and MALDI).

## m/z and ion mobility

Each frame names its `MzCalibration` and `TimsCalibration` rows (`Frames.MzCalibration`, `Frames.TimsCalibration`) and records the temperatures `Frames.T1`, `Frames.T2`. Our models (fitted on vendor-calibrated conversions; `docs/provenance/bruker-tdf.md`):

- **m/z, model type 1.** Flight time `t = index · DigitizerTimebase + DigitizerDelay` (ns). With `s = sqrt(m/z + C4)`: `t = C0 + β·s + C2·s² + C3·s³`, `β = sqrt(10^12 / (C1 · f))`, `f = 1 + 10^-6 · (dC1 · (T1 − T1_frame) + dC2 · (T2 − T2_frame))` (`T1`, `T2`, `dC1`, `dC2` of the row). Solved for `s` (quadratic in closed form; Newton steps when `C3 ≠ 0`), `m/z = s² − C4`. Inverse (TOF index of an m/z): evaluate `t` and `index = (t − DigitizerDelay) / DigitizerTimebase` (`MzModel::index`).
- **m/z, model type 2** (columns to `C14`). `C3`, `C4` repeat `C0`, `C2`; `m/z₁` from the type-1 formula with `C4 = 0`; then `m/z = m/z₁ − P(m/z₁) · exp(−d²)` with `P(x) = Σ C(8+k)·x^k` (`C7` coefficients) evaluated at `m/z₁` clamped to `[C5, C6]` and `d` the distance of `m/z₁` outside that range (0 inside).
- **1/K0, TimsCalibration model type 2.** `1/K0 = 1 / (C6 + C7 / (C2 + (C3 − C2)/C1 · (scan − C0 − C4)))`, `scan` the zero-based scan index (fractional for a precursor's `ScanNumber`).

Validated variants: m/z type 1 with `C3 = 0`, `dC2 = 0` (any `C2`, `C4`); m/z type 2 with `C7 = 7` and `C3 = C0`, `C4 = C2`; 1/K0 type 2. Any other value of those parameters is applied by the same formula and flagged (`notes`, `check` warning `calibration_unvalidated`, `mz_calibration.unvalidated` in `info`). A frame whose row is missing or of another model type falls back to the acquisition-range approximations timsrust uses, flagged in `notes`, `check` (`calibration_not_applied`) and each spectrum's `extra.mz_calibration`:

- `sqrt(m/z)` linear in the TOF index: intercept `sqrt(MzAcqRangeLower)`, slope `(sqrt(MzAcqRangeUpper) − intercept) / DigitizerNumSamples`; both bounds widen by 5 when `AcquisitionSoftware` is `Bruker otofControl` (typically tens of ppm off).
- 1/K0 linear in the scan index: `OneOverK0AcqRangeUpper` at scan 0, `OneOverK0AcqRangeLower` at scan `max(NumScans) − 1`.

A spectrum summed over several PASEF frames (a DDA precursor) is converted with its first frame's model (frames seconds apart differ by far less than 0.1 ppm). The raw TOF indices remain available through the inverse above and `TimsDataset::read_frame` (library).

## Spectra

Spectra are listed in frame order; `index` is the position, `scan_number` = `index + 1`, `native_id` names the source:

| frame `MsMsType` | spectra | `native_id` | fields |
| --- | --- | --- | --- |
| 0 (MS1) and other non-PASEF types | one per frame, all scans summed | `frame=ID` | `ms_level` 1 (2 for non-zero types), `total_ion_current` = `SummedIntensities`; `FrameMsMsInfo` (MS/MS without PASEF) gives precursor, isolation window, charge, collision energy |
| 8 (DDA-PASEF) | one per precursor, at its first PASEF frame: the scans `ScanNumBegin`…`ScanNumEnd` (end exclusive) of every frame listed for it in `PasefFrameMsMsInfo`, summed | `precursor=ID` | `precursor_mz` = `MonoisotopicMz` (else `LargestPeakMz`), `precursor_charge`, `precursor_intensity`, `inverse_reduced_mobility` from `ScanNumber` (truncated to an integer scan, as timsrust does), isolation window and collision energy of the first selection, `extra.parent_frame`, `extra.pasef_frames` |
| 9 (DIA-PASEF) | one per window of the frame's window group (`DiaFrameMsMsInfo` → `DiaFrameMsMsWindows`), its scan range summed | `frame=ID window=K` | `isolation_window_mz`, `collision_energy`, `extra.scan_range`, `extra.window_group`, `extra.inverse_reduced_mobility_range` |
| TSF | one per frame: the profile spectrum when the file stores one (`HasProfileSpectra`), else the line spectrum; the `centroid` view (`spectra --centroid`, mzML export `--centroid`) is the line spectrum | `frame=ID` | `centroided` false for a profile, true for a line spectrum; `extra.tsf_spectrum` (`profile`/`line`); MALDI frames: `extra.maldi_spot`, `position` (`[XIndexPos, YIndexPos]`), `stage_position_um`, `maldi_region`, `laser_power`, `laser_shots` |

Summation: intensities of equal TOF indices are added as integers, then TOF indices ascending → m/z; intensities are returned as `f32`. TDF spectra are sparse sums of TOF bins (`centroided` false), not smoothed or peak-picked. `rt_s` = `Frames.Time` (for a precursor, its first PASEF frame); `activation` = `CID` for MS/MS; `extra.frame`, `extra.msms_type`, `extra.accumulation_time_ms`, `extra.mz_calibration` (which model converted the m/z: `MzCalibration N (model type T), frame F temperatures applied`, or the approximation). A DDA precursor's `inverse_reduced_mobility` evaluates the TimsCalibration model at its fractional `ScanNumber` in its parent frame (timsrust truncates to an integer scan). prm-PASEF frames (`MsMsType` 10) are exposed whole (all scans summed, a note says so): no public prm-PASEF file validates a per-target split.

## Chromatograms → traces

Trace 0 `TIC` (`Frames.SummedIntensities`) and trace 1 `BPC` (`Frames.MaxIntensity`) over the MS1 frames; channels `time` (s, `Frames.Time`) and `intensity` (counts); `sample_rate_hz` 0 (irregular sampling). The corpus shows `SummedIntensities` is not the sum of the stored peaks of a frame (it differs in every real frame), so the TIC is the instrument's value, not recomputed.

## Run summary (`info` → `spectra[0]`)

`instrument` from `InstrumentVendor`, `InstrumentName`, `AcquisitionSoftware`, `AcquisitionSoftwareVersion`; `format_version` from `SchemaType` `SchemaVersionMajor.Minor`. `extra`: `acquisition` (`DDA-PASEF`, `DIA-PASEF`, `MS1 only`, `line spectra (TSF)`, `other`), `data_kind`, `frame_count`, `ms1_frames`, `pasef_frames`, `dia_frames`, `precursor_count`, `dia_window_groups`, `polarities` (the distinct `Frames.Polarity` codes: `+` positive, `-` negative), `ms_level_counts`, `acquired_at` (`AcquisitionDateTime`, with its offset), `sample_name`, `method`, `operator`, `instrument_serial`, `schema`, `compression_type`, `closed_properly`, `wal_commits_applied`, `mz_acquisition_range`, `inverse_reduced_mobility_range`, `scans_per_frame`, `mz_conversion` (which m/z model applies), `mobility_conversion`, `mz_calibration` and `mobility_calibration` (the first frame's model parameters: `calibration_id`, `model_type`, `digitizer_timebase_ns`, `digitizer_delay_ns`, `c0`, `beta`, `c2`, `c3`, `mass_offset`, `correction_range_mz`, `correction_coefficients`, `unvalidated`; `c6`, `c7`, `voltage_offset`, `voltage_slope_per_scan`), `tsf_spectra` (`line`, `profile`), `maldi_spots`, `maldi_application`.

## `check`

`not_closed_properly` (warning), `unsupported_compression` (warning), `calibration_unvalidated` and `calibration_not_applied` (warnings), `peak_count_mismatch`, `bad_frame` (blob past the end of the binary file = truncated, zstd failure, inconsistent scan table), `dangling_reference` (a PASEF selection or DIA window naming a missing frame, window group or scan range).

## Vocabulary

| identifier | meaning |
| --- | --- |
| `BrukerTimsReader`, `FORMAT_ID` | the `FormatReader`; `bruker-tdf` |
| `TimsDataset` | an open `.d`: `open`, `kind`, `frame_records`, `conversions` (the approximation), `mz_model`, `mobility_model`, `read_frame`, `database_path` |
| `TimsKind` (`Tdf`, `Tsf`) | which analysis file |
| `dataset_dir`, `kind_of` | the `.d` directory for a path; which kind it holds |
| `FrameRecord` | a `Frames` row: `id`, `time_s`, `polarity`, `scan_mode`, `msms_type`, `blob_offset` (TimsId), `max_intensity`, `summed_intensity`, `scans`, `peaks`, `accumulation_ms`, `ramp_ms`, `mz_calibration`, `tims_calibration` (row ids), `t1`, `t2` (temperatures, °C) |
| `MaldiSpot` | a `MaldiFrameInfo` row: `spot`, `region`, `x_index`, `y_index`, `motor_um`, `laser_power`, `laser_shots` |
| `MzCalibrationRow` | an `MzCalibration` row: `id`, `model_type`, `digitizer_timebase`, `digitizer_delay`, `t1`, `t2`, `dc1`, `dc2`, `c` (`C0`…); `model` (for a frame's temperatures); `mz_rows` reads the table |
| `TimsCalibrationRow` | a `TimsCalibration` row: `id`, `model_type`, `c`; `model`; `tims_rows` reads the table |
| `MzModel` | a frame's m/z model: `calibration_id`, `model_type`, `unvalidated`; `time`, `mz`, `index` (inverse), `describe` |
| `MobilityModel` | a 1/K0 model: `calibration_id`; `inverse_mobility`, `describe` |
| `Unvalidated` | which part of a model no reference file validated (`None`: none) |
| `Selection` | a PASEF selection or DIA window: `frame`, `scan_begin`, `scan_end` (exclusive), `isolation_mz`, `isolation_width`, `collision_energy` |
| `PrecursorRecord` | a `Precursors` row: `id`, `monoisotopic_mz`, `largest_peak_mz`, `charge`, `scan_number`, `intensity`, `parent_frame` |
| `Conversions` | `mz_intercept`, `mz_slope`, `mobility_intercept`, `mobility_slope`; `mz(tof)`, `mobility(scan)` |
| `TimsFrame` | decoded frame: `scan_offsets`, `tof_indices`, `intensities`; `scan_count`, `scan_range` |
| `FrameError` | why a blob did not decode |
| `read_blob`, `decode_tdf_frame`, `decode_tdf_frame_lzf`, `decode_tsf_spectrum`, `decode_tsf_profile`, `group_and_sum` | blob I/O, TDF (types 2 and 1) and TSF (line, profile) decoding, per-TOF summation |
| `SqliteDb` | read-only SQLite: `open`, `path`, `tables`, `has_table`, `read_table`, `count_rows`, `wal_frames` (committed WAL transactions applied) |
| `SqlTable` | a whole table: `name`, `columns`, `rows`; `column`, `value` |
| `SqlValue` (`Null`, `Integer`, `Real`, `Text`, `Blob`) | a stored value: `as_i64`, `as_f64`, `as_text`, `to_json` |
| `SqliteError` (`Io`, `Corrupt`, `Unsupported`) | why the database could not be read |
