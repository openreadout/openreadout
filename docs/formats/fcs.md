# FCS (Flow Cytometry Standard)

FCS is the open file format that flow cytometers and their software write. OpenReadout returns each data set as a table of events with one column per parameter, plus the keywords and instrument metadata; compensation, transforms and gates are applied on request.

Derived from the public ISAC specification "Data File Standard for Flow Cytometry, Version FCS 3.1 — Normative Reference" (2009, CC BY-SA 3.0) and checked against public corpus files (FCS 2.0, 3.0, 3.1 from many instrument families). FlowIO (BSD-3-Clause) and fcsparser (MIT) were read as prior art and are used as reference readers. See `docs/provenance/fcs.md`.

FCS is an open standard, so unlike the microscopy formats most of this page restates a specification; the corpus notes record where real files depart from it. Section numbers (§) refer to the FCS 3.1 normative reference.

A file is one or more **data sets**. Each data set has a HEADER, a primary TEXT segment of keyword-value pairs, a DATA segment of events, and optionally a supplemental TEXT, an ANALYSIS segment and implementor-defined OTHER segments. `$NEXTDATA` links data sets. OpenReadout exposes each data set as one **table**: one row per event, one column per parameter.

## FCS 3.2

`FCS3.2` data sets (Spidlen et al. 2021) are read like 3.1 with these differences: `$PnDATATYPE` gives a measurement its own data type (I, F or D) when it differs from `$DATATYPE`, and widths, decoding, `dtype` and bit masks follow it per column; `$MODE` may be absent (list mode); `$BEGINSTEXT`/`$ENDSTEXT`/`$BEGINANALYSIS`/`$ENDANALYSIS` may be absent; `$BEGINDATETIME`/`$ENDDATETIME` (ISO 8601, with a zone) become `acquisition_start`/`acquisition_end` in place of `$DATE`/`$BTIM`/`$ETIM`; `$CARRIERID`, `$CARRIERTYPE`, `$LOCATIONID`, `$FLOWRATE`, `$UNSTAINEDCENTERS`, `$UNSTAINEDINFO` are copied to `carrier_id`, `carrier_type`, `location_id`, `flow_rate`, `unstained_centers`, `unstained_info`. Corpus: `zenodo19221995-facsdiscover-zam36` (BD FACSDiscover S8, 440 measurements, delimiter CR).

## HEADER (§3.1)

All offsets are ASCII decimal, right-justified in 8 bytes, relative to the first byte of the data set, and inclusive (the end offset is the last byte of the segment).

| bytes | our name | content |
| --- | --- | --- |
| 0–5 | `version` | `FCS2.0`, `FCS3.0`, `FCS3.1` or `FCS3.2` |
| 6–9 | – | spaces |
| 10–17 / 18–25 | `text` | primary TEXT begin / end |
| 26–33 / 34–41 | `data` | DATA begin / end |
| 42–49 / 50–57 | `analysis` | ANALYSIS begin / end (zeros or blanks when absent) |
| 58… | `other` | optional OTHER segment begin/end pairs, 16 bytes each, until the first segment starts |

When a segment lies (partly) beyond byte 99,999,999 its HEADER offsets are `0` and the real offsets are only in TEXT (`$BEGINDATA`/`$ENDDATA`, `$BEGINANALYSIS`/`$ENDANALYSIS`, `$BEGINSTEXT`/`$ENDSTEXT`). Blank fields (all spaces) are read as 0 and reported as `blank_header_offset` (seen in `fcsparser-fake-large`, `flowio-coulter-lmd`). `HEADER_LEN` = 58; `header_len` is 58 plus 16 per OTHER pair actually present (74 in `fcsparser-cyflow-cube-8`).

## TEXT (§3.2)

- The first byte of the primary TEXT is the **delimiter** (ASCII 1–126). Seen in the corpus: `/`, `\`, `|`, form feed `0x0C` (BD FACSDiva, Cytek SpectroFlo), record separator `0x1E` (Beckman Coulter Navios).
- Pairs are `keyword DELIM value DELIM`. A delimiter inside a keyword or value is doubled. Keywords and values may not be empty, so a doubled delimiter is always an escape — except as the last two bytes of a segment, where it can only be a terminator plus an empty final value (`flowio-data1` ends `…Analysis Doc.\\`).
- Keywords are case-insensitive (`get` ignores case); FCS-defined keywords start with `$`. Values are UTF-8 in 3.1; values that are not valid UTF-8 (older Mac writers, e.g. `CellQuest Pro\xAA`) are read byte-for-byte as Latin-1 and reported as `non_utf8_value`.
- Numeric values must not be padded (§3.2.17) but often are (`$TOT/83411               /` in FACSDiva exports, `$BEGINDATA/     3446/` in Guava) or carry leading zeros (`$BEGINDATA/0000000000004161/`). We trim; `check` reports padding as `padded_numeric_value` (info).
- Trailing spaces or NULs after the final delimiter are ignored (Miltenyi supplemental TEXT ends with one space).

### Supplemental TEXT and ANALYSIS

`$BEGINSTEXT`/`$ENDSTEXT` locate the supplemental TEXT; it uses the primary delimiter and may or may not start with it (Miltenyi: starts with `/`; `flowio-m0-wm278-s1-zero-gain`: starts directly with `SORTSTATS|`, and sits after DATA). Seen oddities: the range equals the primary TEXT (`flowio-b01-kc-a-w-91-us`, not re-parsed), and a ZIP archive instead of keywords (`fcsparser-cyflow-cube-8`, signature `PK\x03\x04`; the same file's TEXT says `P$CFGTYPE/ZIP/`). ANALYSIS (§3.4) has the TEXT syntax; its location comes from `$BEGINANALYSIS`/`$ENDANALYSIS`, else the HEADER. Keyword lookups for normalized fields search the primary TEXT, then the supplemental TEXT.

## Required keywords (§3.2.18) and how we use them

| keyword | our field | use |
| --- | --- | --- |
| `$BYTEORD` | `byte_order` | `1,2,3,4` → `LittleEndian`, `4,3,2,1` → `BigEndian`; two-byte `1,2` / `2,1` appear in Beckman Coulter FCS 2.0 files; any other permutation → `Mixed` (reported, not decoded) |
| `$DATATYPE` | `data_type` | `I` `Integer`, `F` `Float`, `D` `Double`, `A` `Ascii` |
| `$MODE` | `mode` | `L` `List` (decoded); `C` `Correlated`, `U` `Uncorrelated` histograms (deprecated; described, reads exit 6) |
| `$PAR` | `parameters` | number of `$Pn*` sets read |
| `$TOT` | `event_count` | rows |
| `$PnB` | `width` | `Fixed(n)`: bits for I/F/D, characters for A; `FreeFormat` for `*` |
| `$PnN` | `short_name` | column `name` |
| `$PnR` | `range`, `range_text` | column `range`; the I bit mask |
| `$PnE` | `amplification` | `[decades, offset]` in column `extra.amplification` |
| `$BEGINDATA`, `$ENDDATA` | `data_range` | DATA location (see below) |
| `$BEGINSTEXT`, `$ENDSTEXT` | `supplemental` | supplemental TEXT location |
| `$BEGINANALYSIS`, `$ENDANALYSIS` | `analysis` | ANALYSIS location |
| `$NEXTDATA` | `next_data` | offset of the next data set **relative to the start of this one**; 0 = last |

Optional parameter keywords copied into column `extra`: `$PnS` → `label`; `$PnG` → `gain`; `$PnV` → `detector_voltage` (volts); `$PnL` → `excitation_wavelength_nm` (list; 3.1 allows several); `$PnO` → `excitation_power_mw`; `$PnF` → `filter`; `$PnT` → `detector_type`; `$PnP` → `percent_emitted`; `$PnD` → `display_scale` (verbatim); `$PnCALIBRATION` → `calibration` (verbatim). Column `extra.bits` is `$PnB`, `extra.range_keyword` is `$PnR` as written, and `extra.bit_mask` appears for I data when the mask is narrower than the field.

Data-set keywords copied into table `extra`: `$CYT`/`$CYTSN` → `instrument.model`/`serial_number`; `$SYS` → `system`; `$DATE` → `acquisition_date`; `$DATE`+`$BTIM`/`$ETIM` → `acquisition_start`/`acquisition_end`; `$FIL` → `file_name` (also the table `name`); `$SRC` → `source`; `$EXP` → `experimenter`; `$OP` → `operator`; `$INST` → `institution`; `$SMNO` → `specimen`; `$CELLS` → `cells`; `$PROJ` → `project`; `$COM` → `comment`; `$PLATEID`/`$PLATENAME`/`$WELLID` → `plate_id`/`plate_name`/`well_id`; `$ORIGINALITY`; `$LAST_MODIFIER`; `$LAST_MODIFIED` → `last_modified`; `$VOL` → `volume_nl`; `$TIMESTEP` → `timestep_s`; `$TR` → `trigger {parameter, threshold}`; `$LOST` → `events_lost`; `$ABRT` → `events_aborted`. Also `fcs_version`, `data_set_offset`, `datatype`, `byte_order`, `mode`, `event_width_bytes`, `supplemental_text`, `analysis_segment`, `spillover`, `software`, `vendor_keywords`.

### Dates and times

- `$DATE`: `dd-mmm-yyyy` (spec). Also seen: `dd-Mmm-yy` (`22-Sep-13`, FCS 2.0 CellQuest; two-digit years < 70 → 20yy, else 19yy — inferred), `yyyy-Mmm-dd` (Miltenyi MACSQuantify), mixed-case months, trailing spaces.
- `$BTIM`/`$ETIM`: `hh:mm:ss.cc` (3.1, hundredths), `hh:mm:ss:tt` (2.0/3.0, sixtieths per 3.1 Appendix B, converted to hundredths: `17:29:39:51` → `17:29:39.85`), `hh:mm:ss`. A fourth field with three digits (`09:42:05:509`, CyFlow) is not a sixtieth; it is dropped rather than guessed.
- Output is ISO-8601 without a time zone (FCS records none); `info` adds a note saying so (`book/src/guides/metadata.md`). `acquisition_end` rolls to the next day when `$ETIM` < `$BTIM`.

### Spillover (`spillover`)

`$SPILLOVER` (3.1, §3.2.20): `n,name1,…,namen,s11,s12,…,snn`, row-major, `sij` = spillover from parameter i into j; names are `$PnN`. `$SPILL` (FlowKit's compensation example) and `SPILL` (BD FACSDiva and others; not FCS keywords) have the same layout; they are used only when every name in them is a `$PnN` of the same data set (true for the DiVa files; false for the Stratedigm files, whose SPILL names were redacted). `$COMP` (FCS 3.0) is exposed as a matrix with `parameters` empty when the value lists no names; no claim is made about whether it is a spillover or a compensation matrix, and it is never applied. `info` reports the matrix; `table --compensate` and `analyze gate` apply it (`docs/formats/flowjo-wsp.md`).

## Scale values, compensation, transforms, gates

`read_table` and `export` return raw DATA values. `openreadout table FILE --compensate [--transform …] [--workspace W.wsp|--gatingml G.xml --population PATH]` and `openreadout analyze gate` first convert them to **scale values** (FCS 3.1 §3.2.19–20): `$PnE f1,f2` with `f1 > 0` gives `10^(f1·x/$PnR)·f2` (`f2 = 0` read as 1); a `$PnG` other than 0 or 1 divides; the `Time` parameter is multiplied by `$TIMESTEP` and never divided by a gain (as FlowIO does). Then compensation, transforms and gates as described in `docs/formats/flowjo-wsp.md`; `table` records every step in `processing`.

## Instrument families (`platform`) and parameter roles

`tables[].extra.platform` names the family that wrote the data set, recognised from keywords the file carries (provenance: `Source::Inferred`), and adds what that family's keywords mean:

| `family` | recognised by | corpus files | extra fields |
| --- | --- | --- | --- |
| `bd-spectral` | `CREATOR` starts `BD FACSChorus`, or `BDSPECTRAL UNMIXED` present | `zenodo19221995-facsdiscover-zam36` | `unmixing` {`stored`, `parameters` (from `BDSPECTRAL UNMIXED`), `method` (`BDSPECTRAL UNMIXING METHOD`), `applied` (`BDSPECTRAL APPLY`)}, `detector_count` (the `SPILL` detectors), `imaging_features` (`$PnFEATURE` values other than Area/Height/Width), `cytometer_configuration`, `lasers`; columns: `unmixed_fluorescence`, `imaging_feature`, `spectral_detector` |
| `bd-facsdiva` | `CREATOR` starts `BD FACSDiva` | `fcsparser-facs-diva`, `-fortessa-a01`, `-hts-lsr-ii-d06`, `flowio-100715` | `lasers` (`LASERnNAME`, `LASERnDELAY` → `delay`, `LASERnASF` → `area_scaling_factor`), `experiment` (`EXPERIMENT NAME`), `tube` (`TUBE NAME`), `cytometer_configuration`, `cst_setup_status`, `compensation_applied_in_acquisition` (`APPLY COMPENSATION`) |
| `cytek-spectral` | `CREATOR` starts `SpectroFlo`, or `$CYT` `Aurora` / `Northern Lights` / `NL-…` | `zenodo17457137-aurora-beads`, `fcsparser-cytek-nl-2000-header` | `lasers`, `detector_count`, `detectors_by_laser` (full-spectrum detectors `<laser><n>-A` with laser prefixes `UV`, `V`, `B`, `YG`, `R` named by the file's `LASERnNAME`), `unmixing` (`stored: false` when `$PnTYPE` says `Raw_Fluorescence` or `$SPILLOVER` is the identity; `true` when no detector-named parameters are present), `user_setting`, `group`, `tube` |
| `beckman-cytoflex` | `CYTEXPERTFIL` present, or `$CYT` starts `CytoFLEX` | `zenodo18439538-cytoflex` | `software` `CytExpert`, `tube` (`TBNM`), `group` (`CGNM`), `plate_number` (`PLTNO`), `compensation_channels` (`COMPCHH`) |
| `sony-spectral` | `$CYT` `SA3800`, `ID7000`, `SP6800` | `zenodo7971252-sony-sa3800` | `log_display_parameters` (`$PnD/Logarithmic,…/`) |
| `sony-sorter` | `$CYT` `SH800`, `MA900` | – | – |
| `mass-cytometry` | `$CYT` containing the word `CYTOF`, `DVSSCIENCES`, `FLUIDIGM` or `HELIOS`, or three or more metal-tag channels | `zenodo10510047-cytof-mouse` | `mass_channel_count`, `isotopes` (`142Nd`), `markers_by_isotope` (from `$PnS` `142Nd_CD19` → `CD19`), `note` |

Every platform also has `vendor`, `technology` (`conventional`, `spectral`, `mass`), `model` (`$CYT`) and `software` (`CREATOR`). Per column, `extra.channel_kind` is `time`, `scatter` (`FSC…`, `SSC…`), `fluorescence`, `spectral_detector` (Cytek detectors, with `laser`), `mass` (`<Element><mass>Di`/`Dd`, element checked against the periodic table, mass 75–209; with `metal_tag` {`element`, `mass`, `isotope`} and `marker`), `background` (`BCKG…`), `event_length` (`Event_length`), `gaussian_parameter` (`Center`, `Offset`, `Width`, `Residual`) or `other`; `extra.measure` is `area`/`height`/`width` from the `-A`/`-H`/`-W` suffix; `extra.parameter_type` is `$PnTYPE` (FCS 3.2; `Raw_Fluorescence`, `Time`, … in the Aurora file), and the other FCS 3.2 measurement keywords are copied as `detector` (`$PnDET`), `feature` (`$PnFEATURE`), `dye` (`$PnTAG`), `analyte` (`$PnANALYTE`) and `datatype` (`$PnDATATYPE`, when given). `info --view explain` describes spectral and mass-cytometry files in these terms and the experiment model carries `cytometry_technology`, `spectral_detectors` and `mass_channels` method parameters and the vendor as instrument manufacturer.

## DATA (§3.3)

### Locating DATA

1. Candidates: the TEXT pair (`$BEGINDATA`, `$ENDDATA`, unless both 0) and the HEADER pair (unless both 0). FCS 3.x tries TEXT first; FCS 2.0 tries the HEADER first.
2. The first candidate whose length (`end − begin + 1`) equals `$TOT × event width`, or is one byte longer, wins; otherwise the first candidate.
3. Differing pairs → `offset_discrepancy` (warning). In `flowio-data-start-offset-discrepancy` the HEADER says 5555–6188 (634 bytes) and TEXT 6081–6188 (108 bytes = 2 events × 54 bytes): TEXT wins. In `-stop-` the HEADER end lies past the end of the file.
4. Length one byte longer than needed → `data_end_off_by_one` (warning; Miltenyi, Coulter). Shorter → `data_length_mismatch` (error). Past end of file → `truncated` (error, exit 4).

### Decoding (list mode)

Events are stored one after another; within an event, parameters in order 1…`$PAR`, each `$PnB` wide.

- **I** — unsigned integers of `$PnB / 8` bytes (any whole number of bytes up to 8; 8, 16, 24, 32 seen), in `$BYTEORD` order, then masked: `mask = next_power_of_two(ceil($PnR)) − 1` when that is narrower than the field, else all ones (§3.3; `$PnR/1024/` → 1023; `$PnR/11209599/` on 32 bits → 2²⁴−1; `$PnR/4294967296/` on 32 bits → no mask). `$PnB` not a multiple of 8 (bit-packed) → unsupported (exit 6).
- **F** — IEEE-754 binary32 (`$PnB` must be 32); **D** — binary64 (`$PnB` 64). No mask; values may be negative or exceed `$PnR`.
- **A** — fixed width: `$PnB` characters per value, parsed as a decimal number (blank field → 0). Free format (`$PnB/*/`): values separated by space, tab, comma, CR or LF, runs of separators counting as one. Deprecated in 3.1; no corpus file uses it (unit tests only).

`read_table` returns values as f64 (lossless for every width ≤ 32 bits and for F/D), raw: not compensated and not scaled by `$PnE`, `$PnG` or `$TIMESTEP`. Storage `dtype` per column: I → `uint8`/`uint16`/`uint32`/`uint64` (smallest that holds `$PnB` bits; 24-bit → `uint32`), F → `float32`, D and A → `float64`.

### Oracle hash (corpus tests)

`xxh3` in `corpus/oracle/<id>.json` → `tables[]` = xxh3-128 over every value of the data set as little-endian f64, column-major (all events of parameter 1, then parameter 2, …). `oracle/gen.py` computes it from FlowIO (`as_array(preprocess=False)`) or, for data sets FlowIO cannot decode (24-bit integers, blank HEADER offsets), from fcsparser; the corpus test computes it from `read_table`. FlowIO 1.4 ignores FCS 3.2's `$PnDATATYPE`, so for FCS 3.2 files with mixed types the oracle re-reads the DATA segment with a NumPy record type built from each measurement's type (fcsparser, which also ignores it, is kept as a disagreeing second opinion).

## CRC (§3.5)

Eight ASCII bytes right after the last segment of a data set. CRC-16 with the CCITT polynomial, each input byte bit-reversed and initial value 0 (processed LSB-first: reflected polynomial `0x8408`; check value of `123456789` is `0x2189`). `00000000` means "not computed" (DiVa, Miltenyi 3.0, Navios). No corpus file stores a computed CRC, so a mismatch is a warning, not corruption; byte-swapped equality is accepted.

## Multiple data sets

`$NEXTDATA` is relative to the current data set's first byte (FCS 2.0 wording, followed by FlowIO; fcsparser treats it as absolute, which only works for the second data set). Guava Muse writes four data sets; Beckman Coulter `.lmd` files hold an FCS 2.0 data set followed by an FCS 3.0 copy with 32-bit values whose TEXT comes after its DATA. The walk stops at 0, at an offset past the end of the file (`truncated`), or at an offset already visited (`bad_next_data`).

## `check` finding codes

`truncated`, `data_length_mismatch`, `missing_data_offsets`, `missing_keyword`, `bad_keyword`, `unsupported_width`, `bad_offset`, `bad_next_data`, `bad_data_set` (errors); `offset_discrepancy`, `data_end_off_by_one`, `duplicate_keyword`, `duplicate_parameter_name`, `text_unterminated`, `dangling_keyword`, `empty_value`, `nonprintable_keyword`, `bad_delimiter`, `mixed_byte_order`, `histogram_mode`, `crc_mismatch`, `non_keyword_segment`, `no_events` (warnings); `padded_numeric_value`, `blank_header_offset`, `non_utf8_value`, `log_zero_offset`, `crc_ok`, `crc_not_computed` (info).

## Instrument-specific keywords (`vendor_keywords`)

Every keyword not starting with `$` (primary and supplemental TEXT) is copied verbatim into table `extra.vendor_keywords`, grouped by a mechanical prefix (`keyword_prefix`): a namespace before an inner `$` (`FJ$ACQSTATE` → `FJ`, `GTI$SAMPLEID` → `GTI`, `P$CFGTYPE` → `P`), a leading symbol (`@SAMPLEID1` → `@`, `&10PATIENT ID` → `&`, `#…` → `#`), per-parameter keywords `Pn…` → `Pn`, letters followed by a number (`LASER1NAME` → `LASERn`), else the first word (`CST SETUP STATUS` → `CST`, `ANALOG_COMP` → `ANALOG`). Values over 1024 bytes are shortened in `info`; `info --view full` has them in full. No meaning is assigned except where stated above (`CREATOR` → `software`, `SPILL` → `spillover`).

| writer (from the file) | corpus files | keywords seen |
| --- | --- | --- |
| BD FACSDiva 6.2 (`CREATOR`) | `fcsparser-facs-diva`, `-fortessa-a01`, `-fake-large`, `-hts-lsr-ii-d06`, `flowio-100715` | `CREATOR`, `TUBE NAME`, `EXPERIMENT NAME`, `GUID`, `SPILL`, `APPLY COMPENSATION`, `THRESHOLD`, `WINDOW EXTENSION`, `FSC ASF`, `AUTOBS`, `EXPORT USER NAME`, `EXPORT TIME`, `CST SETUP STATUS`, `CST BEADS LOT ID`, `CST SETUP DATE`, `CST BASELINE DATE`, `CYTOMETER CONFIG NAME`, `CYTOMETER CONFIG CREATE DATE`, `CYTNUM`, `LASERnNAME`, `LASERnDELAY`, `LASERnASF`, `PLATE NAME`, `PLATE ID`, `WELL ID`, `SAMPLE ID`, `PnDISPLAY`, `PnBS`, `PnMS` |
| Cytek SpectroFlo 2.2.0 (`CREATOR`, `$CYT/Aurora/`) | `fcsparser-cytek-nl-2000-header` | `CREATOR`, `LASERnNAME`, `LASERnDELAY`, `LASERnASF`, `FSC ASF`, `WINDOW EXTENSION`, `THRESHOLD`, `APPLY COMPENSATION`, `USERSETTINGNAME`, `TUBENAME`, `GROUPNAME`, `CHARSET`, `PnDISPLAY` |
| FlowJo Collectors' Edition 7.5 on a Cytek xP5 | `fcsparser-cytek-xp5` | `CREATOR`, `ANALOG_COMP`, `FJ$ACQSTATE`, `FJ_$P1R`, `LASERnNAME`, `SAMPLEID` |

## Observed corpus values

| file | version | delimiter | `$DATATYPE` / `$PnB` | `$BYTEORD` | data sets | notes |
| --- | --- | --- | --- | --- | --- | --- |
| `fcsparser-facs-diva` | 3.0 | 0x0C | F / 32 | 4,3,2,1 | 1 | 83411 events, SPILL 8×8 |
| `fcsparser-cytek-xp5` | 3.0 | `\` | I / 24 | 4,3,2,1 | 1 | 8 × 3 bytes × 23126 = DATA length |
| `fcsparser-cyflow-cube-8` | 3.0 | `/` | I / 8,16,32 | 1,2,3,4 | 1 | OTHER segment = STEXT = ZIP |
| `fcsparser-guava-muse` | 3.0 | `/` | F / 32 | 1,2,3,4 | 4 | relative `$NEXTDATA` |
| `flowio-coulter-lmd` | 2.0 + 3.0 | `\` | I / 16, then I / 32 | 1,2 then 1,2,3,4 | 2 | second TEXT after its DATA |
| `flowio-m0-wm278-s1-zero-gain` | 3.1 | `\|` | I / 32 | 1,2,3,4 | 1 | `$PnR/4294967296/`, STEXT after DATA |
| `fcsparser-miltenyi-fcs31` | 3.1 | `/` | F / 32 | 1,2,3,4 | 1 | STEXT 2722–127220, DATA end off by one |
| `zenodo17457137-aurora-beads` | 3.1 | 0x0C | F / 32 | 1,2,3,4 | 1 | 71 parameters: 64 full-spectrum detectors, `$PnTYPE`, 64×64 identity `$SPILLOVER` |
| `zenodo18439538-cytoflex` | 3.0 | 0x0C | F / 32 | 4,3,2,1 | 1 | zero-padded `$BEGINDATA`, 8×8 `$SPILLOVER` over H and A detectors |
| `zenodo7971252-sony-sa3800` | 3.1 | `\` | F / 32 | 1,2,3,4 | 1 | `$BEGINSTEXT` at 64 (inside the primary TEXT), `$PnD/Logarithmic,6,1/` |
| `zenodo10510047-cytof-mouse` | 3.0 | `\|` | F / 32 | 1,2,3,4 | 1 | 51 metal channels; keyword `$CYTSN_$DATE` (an empty `$CYTSN` value merged with `$DATE`) |
| `flowkit-gml-events` | 2.0 | `\` | I / 16 | 4,3,2,1 | 1 | `$P1G/3.67/`, `$P2G/8/` and `$PnE/4,0/` on four channels: scale values differ from raw |

## Vocabulary (every public identifier in `openreadout-fcs` must appear here)

| identifier | meaning |
| --- | --- |
| `FcsReader`, `FcsDataset`, `FcsFile`, `FORMAT_ID` | reader entry points: format reader, opened file (core `Dataset`), parsed data-set chain, the id `fcs` |
| `open`, `path`, `file_len`, `data_sets`, `chain_findings`, `read_at` | opening a file; its path and size; the data sets found; findings about the `$NEXTDATA` chain; bounded read helper |
| `DataSet`, `index`, `offset`, `header`, `delimiter`, `text_range`, `text`, `supplemental`, `analysis`, `data_type`, `byte_order`, `mode`, `event_count`, `parameters`, `data_range`, `data_source`, `next_data`, `findings` | one data set and its parts (absolute offsets) |
| `keyword`, `event_width`, `expected_data_len`, `is_free_format`, `last_segment_end` | data-set helpers: keyword lookup (primary then supplemental), bytes per event, `$TOT × width`, `$PnB/*/` present, where the CRC field starts |
| `AuxSegment`, `range`, `keywords`, `note` | supplemental TEXT / ANALYSIS: location, parsed keywords, why not parsed |
| `OffsetSource` { `Text`, `Header` }, `name` | which pair located DATA |
| `Header`, `HeaderError` { `TooShort`, `NoSignature`, `BadOffset` }, `HEADER_LEN`, `VERSIONS`, `version`, `data`, `other`, `header_len`, `blank_fields`, `parse_header`, `looks_like_fcs` | HEADER parsing and signature test |
| `SegmentRange`, `begin`, `end`, `new`, `is_unset`, `byte_len`, `absolute` | inclusive byte range and helpers |
| `Keyword`, `KeywordSet`, `name`, `value`, `entries`, `duplicates`, `push`, `get`, `contains`, `len`, `is_empty` | keyword-value pairs, ordered, case-insensitive lookup |
| `TextParse`, `empty_values`, `dangling_keyword`, `nonprintable_keywords`, `latin1_values`, `unterminated`, `parse_keywords` | result of parsing a keyword segment and its irregularities |
| `parse_uint`, `parse_float`, `is_padded` | numeric keyword values (trimmed) and the padding test |
| `DataType` { `Integer`, `Float`, `Double`, `Ascii` }, `from_code`, `code` | `$DATATYPE` |
| `ByteOrder` { `LittleEndian`, `BigEndian`, `Mixed` }, `from_value` | `$BYTEORD` |
| `Mode` { `List`, `Correlated`, `Uncorrelated` } | `$MODE` |
| `FieldWidth` { `Fixed`, `FreeFormat` }, `width` | `$PnB` |
| `Parameter`, `number`, `short_name`, `label`, `range_text`, `amplification`, `gain`, `voltage`, `wavelengths_nm`, `filter`, `detector_type`, `power_mw`, `percent_emitted`, `display`, `calibration`, `data_type`, `detector`, `measurement_type`, `feature`, `tag`, `analyte`, `byte_width`, `bit_mask`, `read_parameters` | one parameter's `$Pn*` keywords and derived widths/masks |
| `storage_dtype`, `decode_fixed`, `decode_free_ascii` | column dtype rule and DATA decoding |
| `SpilloverMatrix`, `values`, `parse_spillover`, `spillover` | spillover/compensation matrix and which keyword supplied it |
| `keyword_prefix` | mechanical grouping of non-`$` keywords for `vendor_keywords` |
| `scale_columns`, `file_matrix`, `parameter_names`, `is_time_parameter`, `is_fluorescence_parameter` | raw → scale values; the file's own compensation matrix; `$PnN`/`$PnS` lists; parameter roles used by `--transform` defaults |
| `Family` { `BdFacsDiva`, `BdSpectral`, `CytekSpectral`, `BeckmanCytoflex`, `SonySpectral`, `SonySorter`, `MassCytometry` }, `family`, `vendor`, `technology`, `id`, `platform` | instrument family recognition and `extra.platform` |
| `channel_kind`, `column_extras`, `metal_tag`, `metal_marker` | per-column roles, metal tags and markers |
| `FcsDate`, `year`, `month`, `day`, `iso`, `next_day`, `parse_date`, `parse_time`, `parse_timestamp`, `acquisition_span` | `$DATE`/`$BTIM`/`$ETIM`/`$LAST_MODIFIED` to ISO-8601 |
| `Crc16`, `update`, `finish`, `CrcField` { `Absent`, `NotComputed`, `Value`, `Other` }, `read_crc_field` | data-set CRC |
| `RowCondition`, `column`, `op`, `threshold`, `CompareOp` { `Greater`, `GreaterEqual`, `Less`, `LessEqual`, `Equal`, `NotEqual` }, `symbol`, `test`, `parse_conditions`, `filtered_slice` | `table --filter 'FITC-A > 1000' --count`: row conditions (`COLUMN OP NUMBER`, all must hold; NaN meets none) tested over the whole table on the values `table` returns, raw or compensated/transformed (module `filter`) |
