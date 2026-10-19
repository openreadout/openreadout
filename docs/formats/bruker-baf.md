# Bruker BAF (`analysis.baf` in a `.d` directory)

Bruker's TOF and Q-TOF instruments outside the timsTOF line (micrOTOF, micrOTOF-Q, maXis, impact, compact) and its FT-ICR instruments (apex, solariX) write each run as `analysis.baf` in a `.d` directory, with `analysis.baf_idx`, `analysis.baf_xtr` and, once processing software has opened the run, `analysis.sqlite` and `Calibrator.ami`. OpenReadout reads the spectra (centroid lists and profiles) with their retention times, MS levels, precursors and m/z. Derived from public files compared with the depositors' vendor-library conversions; no specification or prior reader exists. Provenance: `docs/provenance/bruker-baf.md`. Reader: crate `openreadout-bruker-tims`, module `baf`, format id `bruker-baf`. (timsTOF TDF/TSF directories: `docs/formats/bruker-tdf.md`.)

Detection (`locate`, `looks_like`): a directory holding `analysis.baf`, or that file itself; the file starts with the pointer record. Only `analysis.baf` and `Calibrator.ami` are read.

## Records

The file is a chain of records, each a u32 length (the whole record), a u32 tag `0xBFA0xxxx` (`TAG_BASE`) and a payload; several records hold a header and a chain of sub-records with the same framing (`walk`, `RecordRef` { `offset`, `len`, `tag` }). Little-endian unless stated.

| tag | constant | content |
| --- | --- | --- |
| `BFA00009` | `TAG_POINTERS` | first record: u32 `0xBFA00005`, then u64 offsets of the acquisition header, the summary table and the trailer index (`parse_pointers`, `Pointers` { `acquisition`, `summary`, `trailer` }) |
| `BFA04001` | `TAG_WRITER` | 32-byte writer id (`AIDA6.4.0.1`, `BCC4_3_72_0`): `format_version` |
| `BFA04003` | `TAG_COMPRESSOR` | a GUID and the compressor's name (`Variable VLI2 based compressor`) |
| `BFA03001` | `TAG_ACQUISITION` | acquisition header (below) |
| `BFA02001` | `TAG_SEGMENT` | an acquisition segment: u32 header size, u32 0, u32 segment number, then one sub-record `BFA0000D` (`TAG_SEGMENT_VARIABLES`) per scan type: u32 header size, u32 count, 4 scan-type bytes, then (u16 variable id, 8-byte value) pairs (`parse_segment`, `Segment` { `number`, `lists` }, `SegmentList` { `scan_type`, `values` }) |
| `BFA02002` | `TAG_SPECTRUM` | a spectrum (below) |
| `BFA01001` | `TAG_PROFILE` | a profile block (below) |
| `BFA01002` | `TAG_LINES` | a centroid block (below) |
| `BFA00008` | `TAG_SUMMARY` | one entry per spectrum: retention time, segment, id, block flags, five line statistics, scan-type bytes and (MS1) the calibration (`parse_summary`, `SummaryEntry` { `id`, `segment`, `rt_ms`, `flags`, `stats`, `scan_type` }) |
| `BFA0000A` | `TAG_TRAILER` | (u32 id, u64 offset) pairs after a 64-byte header for every segment, spectrum, profile, centroid and end-of-spectrum record (`parse_trailer`) |

The reader takes record offsets from the trailer index (each checked to be a record) and walks the chain one record at a time when the pointer record or the trailer index is missing (an acquisition that did not finish); `info` → `extra.index_source` says which.

## Acquisition header

`parse_acquisition` → `AcquisitionHeader` { `started_utc`, `analysis_name`, `strings`, `variables`, `reference_calibrations` } (`string`, `variable_id`, `variables_json`): a date block 40 bytes into the body (44 in the 2008 file): u8 day, u8 month, u16 year, i16 UTC offset in minutes, u16 milliseconds, u8 second, minute, hour in UTC; the analysis name; then sub-records. The string table (`TAG_STRINGS`, `parse_strings`): u32 header size, u32 offsets of strings 2 to n up to the first 0, string 1 at the header size; in order the source path, instrument, operator (left out of `info`), acquisition software, its version, method, a dotted version, a role and the sample name. Variable definitions (`TAG_VARIABLE_DEFS`, `parse_variable_defs`, `VariableDef` { `name`, `group`, `label`, `value_text`, `format`, `unit`, `kind` }, `decode`, `text_of`): u16 header size, u16 count, entries of u16 length, u16 id, u8 type (10 double; 0, 1, 3, 4, 5 64-bit integer), u8 0xFF and seven u16 offsets of strings. Calibration sub-records (`TAG_CALIBRATION`) hold the reference calibrations.

## Spectra

A spectrum record (`parse_spectrum`, `SpectrumRecord` { `segment`, `id`, `flags`, `rt_ms`, `scan_type`, `variables`, `precursors`, `calibration` }): u32 header size (40), segment, id (1-based, the scan number; native id `scan=<id>`), block flags (3 profile and centroids, 2 centroids), retention time in milliseconds, 0, the scan-type bytes (polarity 0 positive, 1 negative; scan mode in byte 2), then sub-records: the variable list (`TAG_VARIABLES`, `parse_variable_list`), the precursor list (`TAG_PRECURSORS`: u32, u32 n, n × f32 isolation m/z) and the calibration (`TAG_CALIBRATION`, payload after 12 bytes). MS level (`ms_level`) = 1 + the number of precursors; the precursor m/z is the last one (all of them in `extra.precursors` for MS³ and above); the isolation window is ± half of `Quadrupole_IsolationResolution_Act`, or ± 1 when that is 0; the collision energy is `Collision_Energy_Act`; the activation is the value text of `MSMS_FragmentationMode_Act` (`0:CID;1:ETD`); the precursor charge is `MSMS_PreCursorChargeState` when it is not 0 (as the vendor library reports it; values such as 13 occur). The scan window is `Mode_ScanBegin`/`Mode_ScanEnd` of the segment's list for the spectrum's scan type; TIC and base-peak intensity are the summary table's line sum and maximum.

A centroid block (`parse_lines`, `LineBlock` { `id`, `element_code`, `stats`, `position`, `intensity`, `width` }): u32 header size (52), id, id, element code, line count, u32, five f32 statistics; then u32 count and eight u32 array offsets; each array's element size (4 or 8) is its length divided by the count. Positions are fractional sample indices, converted to m/z by the calibration; the third array (element codes 1 and 3) is kept unread as widths.

A profile block (`parse_profile`): u32 header size (44), id, id, four coding bytes, u32 A, u32 B. Coding `09 04 FF 00`: A float32 samples. Coding `02 01 06 00`: A packed bytes (`decode_packed`, `PACKED_MARKER`, `MAX_SAMPLES`): u32 marker `0xEE77`, u32 sample count, u32 big-endian table length (26), the table (nine class widths 0, 4, 5, 6, 7, 9, 10, 11, 32; code lengths 1 to 9; one extra symbol of length 10), then a bit stream read most significant bit first: a class code of c zero bits and a one; class 0 repeats the current value; classes 1 to 7 read width + 1 bits u, magnitude (u >> 1) + base (1, 17, 49, 113, 241, 753, 1777), negative when u is odd; class 8 a 32-bit two's-complement delta; class 9 a 32-bit run of the current value. Coding `02 01 09 00` (`ZLIB_CODING`, the solariX XR file): A zlib-compressed bytes that inflate to float32 samples. Other tables, markers or codings exit 6. Sample i is at sample index i. The spectrum's primary view is the profile when one is stored, the centroids otherwise; `--view centroid` gives the centroids.

## Calibration

`parse_payload` → `Calibration` { `Tof`, `Ft` } (`mz`, `name`); `CalibrationIssue` { `Unknown`, `Damaged` }.

- **Time of flight, kind 3** (`TofCalibration` { `kind`, `delay_ns`, `timebase_ns`, `t0_ns`, `k`, `q`, `poly`, `poly_range`, `shift`, `temperatures` }): t = delay + timebase · index; k1 = 10⁶ / √K; √(m/z) = (−k1 + √(k1² − 4Q(t0 − t))) / 2Q (or (t − t0) / k1 when Q = 0); then m/z −= p(clamp(m, lo, hi)) · exp(−d²) with d the distance outside [lo, hi] in Da. The detector temperatures follow the polynomial.
- **Kind 10** (maXis): the same quadratic, then m/z −= a mass shift; a non-zero sixth constant exits 6.
- **FT-ICR** (u32 type 4) (`FtCalibration` { `c2`, `c3`, `n_fft`, `f_high` }): f = f_high · (1 − index / N), m/z = c3 / (f + c2).
- **Drift compensation** (`DriftSettings` { `reference`, `gradient`, `enabled`, `one_sensor` }, `factor`; `ReferenceCalibration` { `t0_ns`, `k`, `q`, `reference`, `gradient` }, `parse_reference`): the header's reference calibration with the spectra's t0 gives K and Q at the reference temperatures and the gradients; at a spectrum's temperatures T, f = 1 − 10⁻⁶ (g1 (T1 − r1) + g2 (T2 − r2)) (the second term left out when `TOF_UseOnlyOneTemperature` is set, no compensation when `TOF_SwitchTempCompensation` is 0), K = K_ref · f, Q = Q_ref / f. The reader recomputes them rather than taking the stored values, a few of which are stale.
- **Recalibration** (`find_recalibration`): when the directory holds `Calibrator.ami`, the first payload in it of the first spectrum's kind, delay and time base replaces each spectrum's, with K multiplied and Q divided by f(T spectrum) / f(T recalibration) (or by the ratio of the spectra's stored K when temperatures are missing). Vendor exports of such runs use it; `info` → `extra.mz_calibration_source` says which calibration applies.

## Integrity checks (`check`)

`index` (warnings found while opening), `missing_blocks` (spectra without a centroid block), `gaps` (spectrum ids not consecutive), `summary_mismatch` (summary table and spectrum records disagree), `calibration` (spectra whose calibration is not read), `bad_block` (a block that does not decode; full check).

## Vocabulary (every public identifier of the `baf` module must appear here)

| identifier | meaning |
| --- | --- |
| `BrukerBafReader`, `BafDataset`, `FORMAT_ID`, `open`, `locate`, `looks_like` | reader, detection and opened dataset |
| `TAG_BASE`, `TAG_POINTERS`, `TAG_SUMMARY`, `TAG_TRAILER`, `TAG_ACQUISITION`, `TAG_VARIABLE_DEFS`, `TAG_STRINGS`, `TAG_SEGMENT`, `TAG_SPECTRUM`, `TAG_PROFILE`, `TAG_LINES`, `TAG_WRITER`, `TAG_COMPRESSOR`, `TAG_VARIABLES`, `TAG_PRECURSORS`, `TAG_CALIBRATION`, `TAG_SEGMENT_VARIABLES` | record tags |
| `RecordRef` { `offset`, `len`, `tag` }, `walk`, `cstr`, `Pointers` { `acquisition`, `summary`, `trailer` }, `parse_pointers`, `parse_trailer` | record chain |
| `AcquisitionHeader` { `started_utc`, `analysis_name`, `strings`, `variables`, `reference_calibrations` }, `string`, `variable_id`, `variables_json`, `parse_acquisition`, `parse_strings` | acquisition header |
| `VariableDef` { `name`, `group`, `label`, `value_text`, `format`, `unit`, `kind` }, `decode`, `text_of`, `parse_variable_defs`, `parse_variable_list`, `variable` | variables |
| `Segment` { `number`, `lists` }, `SegmentList` { `scan_type`, `values` }, `parse_segment` | segments |
| `SummaryEntry` { `id`, `segment`, `rt_ms`, `flags`, `stats`, `scan_type` }, `parse_summary` | summary table |
| `SpectrumRecord` { `segment`, `id`, `flags`, `rt_ms`, `scan_type`, `variables`, `precursors`, `calibration` }, `ms_level`, `parse_spectrum`, `SpectrumEntry` { `record`, `offset`, `lines`, `profile`, `summary` } | spectra |
| `LineBlock` { `id`, `element_code`, `stats`, `position`, `intensity`, `width` }, `parse_lines`, `parse_profile`, `decode_packed`, `PACKED_MARKER`, `ZLIB_CODING`, `MAX_SAMPLES` | centroid and profile blocks |
| `Calibration` { `Tof`, `Ft` }, `TofCalibration` { `kind`, `delay_ns`, `timebase_ns`, `t0_ns`, `k`, `q`, `poly`, `poly_range`, `shift`, `temperatures` }, `FtCalibration` { `c2`, `c3`, `n_fft`, `f_high` }, `CalibrationIssue` { `Unknown`, `Damaged` }, `parse_payload`, `mz`, `name` | calibrations |
| `DriftSettings` { `reference`, `gradient`, `enabled`, `one_sensor` }, `factor`, `ReferenceCalibration` { `t0_ns`, `k`, `q`, `reference`, `gradient` }, `parse_reference`, `find_recalibration` | drift compensation and recalibration |
| `baf`, `calibration`, `dataset`, `records`, `spectrum` | modules |

`info` → `spectra[0].extra`: `acquired_utc`, `utc_offset_minutes`, `method`, `sample_name`, `analysis_name`, `writer`, `compressor`, `segments`, `profile_spectra`, `calibration`, `mz_calibration_source`, `index_source`, `analysis_sqlite`, `profile_coding`, `centroid_element_code`, `polarities`. Per spectrum `extra`: `segment`, `scan_mode_code`, `precursors`.

## Oracle mapping (corpus harness)

`oracle/gen.py` (`bruker_baf`): the depositor's mzML (ProteoWizard with the vendor library, or CompassXport), read with pyteomics like the other vendor-export oracles; scans by scan number.

## Known gaps

- `Calibrator.ami`'s container is not decoded: its payload is found by scanning. Without the file, m/z are the acquisition calibration's.
- `analysis.baf_xtr` (where the vendor library finds a refined precursor m/z for some instruments), the integer and real list sub-records and the third centroid array are not decoded.
- Calibration kinds, profile codings and code tables other than the ones above exit 6.
