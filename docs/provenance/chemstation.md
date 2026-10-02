# Provenance log — Agilent ChemStation (`.D` directories: `.ch`, `.uv`, `.ms`, `.reg`/`.txt`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-09-22 — initial derivation (Richard Zimring with Claude as assistant)

**Order of work.** Hex dumps of the corpus files first (`xxd`, plus small Python scripts that print every 2/4/8-byte big- and little-endian interpretation at candidate offsets); the hypotheses were written down, then checked against the permitted prior art below, then against the oracles.

**Corpus files used** (see `corpus/manifest.toml`; all from the `entab` repository test data, MIT License, commit `944a62b7ee82796ead22e7ceaa0b7c4e4e6a63d0`, `entab/tests/DATA_SOURCES.txt` says "collected by Roderick" (the author) or "from issue #32"):
- `entab-test-fid-ch` (`test_fid.ch`, version 81, GC FID, 6,434 bytes)
- `entab-test-179-fid-ch` (`test_179_fid.ch`, version 179, GC FID, 102,144 bytes)
- `entab-chemstation-mwd-d` (`chemstation_mwd.d/`, five `mwd1A..E.ch` version 30 LC multi-wavelength-detector signals plus `.REG`/`RUN.LOG`/method files)
- `entab-carotenoid-extract-d` (`carotenoid_extract.d/`, `dad1.uv` version 131 DAD spectra, `MSD1.MS` version 2 LC-MS single quad, `.REG`, `MSPARMS.txt`, `RUN.LOG`, `SAMPLE.XML`, method directory)

**Inferred from the hex dumps (before reading prior art):**
- Byte 0 is a length byte followed by ASCII digits: the file's version (`81`, `30`, `179`, `131`, `2`). Versions ≤ 102 continue with length-prefixed single-byte strings at fixed offsets: `GC DATA FILE`/`LC DATA FILE`/`MSD Spectral File` at 0x04, sample name at 0x18, operator at 0x94, date at 0xB2, instrument model at 0xD0 (`HP G1530A`, `G1365B`, `Instrumen`), separation type at 0xDA (`GC`/`LC`), method at 0xE4, units at 0x244 (`pA`, `mAU`), signal description at 0x254 (`MWD A, Sig=210,5 Ref=360,100`).
- Versions 130/131/179 repeat the version at 0x146 and hold strings as a length byte followed by UTF-16LE code units: file type 0x15B, sample 0x35A, operator 0x758, date 0x957, 0x9BC/0x9E5 (`GCI`, `DAD1`, `GC`/`LC`), method 0xA0E, instrument name 0xC11 (`Asterix ChemStation`), software 0xE11 and revision 0xEDA, units 0x104C, signal 0x1075 (`FID1A, Front Signal`).
- A big-endian u32 at 0x108 gives the header length: 3 → 1,024 bytes (0x400, versions 30/81), 13 → 6,144 bytes (0x1800, 179); the data start was located independently in each file as the end of the zero padding. Rule `(n − 1) × 512`.
- 0x11A/0x11E hold the first/last retention time in milliseconds: big-endian f32 in versions 81 and 179 (180,026.69 / 719,826.69 ms; 49.66 / 599,999.69 ms) and big-endian i32 in version 30 (−2,380 / 717,620 ms).
- A big-endian f64 scale factor sits at 0x284 in 30/81 (1/7,680 for FID pA, 1,000/2²¹ for MWD mAU) and at 0x127C in 179; the f64 at 0x27C is zero in every corpus file (offset).
- Version 179 data: little-endian f64 from 0x1800 to the end of file (96,000 bytes = 12,000 values; ≈ 59,487 raw → 7.7 pA with the scale).
- Version 30 data: records `0x10, n` followed by `n` big-endian i16 values, where `0x8000` introduces a big-endian i32 absolute value; the i16 values vary smoothly and change sign around peaks, i.e. they are first differences.
- Version 81 data: big-endian i16 words; the first word is `0x7FFF` followed by six bytes (`0000 0002 0D06`); peak regions show the pattern (+, −−, +) of a second difference, and the words sum to about zero over a peak.
- Version 131 (`.uv`): header 0x1000 bytes; per-scan records, little-endian: tag 67, record length, time (ms), wavelength low/high/step × 20, 8 unknown bytes, then first-difference i16 values with the same `0x8000` escape to a little-endian i32; record length = 22 + 2 × wavelengths.
- Version 2 (`.ms`): header length in 16-bit words at 0x10A (257 → 512 bytes); scan count at 0x116 (big-endian u32, 2,534).

**Permissively licensed prior art consulted** (read after the hex work, to confirm; no code copied):
- `entab` by Roderick Bovee, MIT, https://github.com/bovee/entab (commit `944a62b`), `entab/src/parsers/agilent/{chemstation.rs,metadata.rs,mod.rs}` — confirms: header length rule (`2 × (n − 1)` × 256 bytes for signal files, `2 × (n − 1)` bytes for `.ms`), the version-81 escape (`0x7FFF` then i32 + u16) and second-difference accumulation that resets on escape, the version-30 record structure and `0x8000` escape, the `.uv` record layout, the `.ms` record layout (length in words, time u32 ms, 12 bytes, m/z × 20 as u16, intensity as 14-bit mantissa × 8^(2-bit exponent), 10-byte trailer ending with the TIC), and the string offsets for sample, operator, date, instrument, method. Differences noted: entab reads the 81 start time as an integer (a 20,184-minute start in its own test), and multiplies the escape's high word by 65,534 (see below).
- `Aston` by Roderick Bovee, BSD-3-Clause, https://github.com/bovee/Aston (commit `3bfd589`), `aston/tracefile/{agilent_fid.py,agilent_uv.py,agilent_ms.py,mime.py}` — confirms the f32 start time at 0x11A for 81 and 179, the i32 times for 30/130, the 179 f64 body at 0x1800, the `.uv` record walk, and the first-bytes magic table (`02 38` = 8x, `02 33` = 3x, `03 31` = 1xx, `01 32` = 2).

**Public documentation consulted** (web pages):
- rainbow documentation (LGPL-3.0 project; documentation pages), https://rainbow-api.readthedocs.io/en/latest/agilent.html and its pages `agilent/ch_fid.html`, `agilent/ch_other.html`, `agilent/uv.html`, `agilent/ms.html` — confirms the UTF-16 string offsets for 130/131/179, the 0x127C scale factor in 179/130, the `0x10, n` record layout of 30/130, the `.uv` record header and ×20 wavelengths, the `.ms` scan record (m/z × 20; intensity mantissa/exponent; TIC in the trailer) and the LC-MS vs GC-MS scan-count locations (0x118 big-endian vs 0x142 little-endian).

**Open questions and decisions.**
- Version-81 escape value: the six bytes after `0x7FFF` are read as a 48-bit big-endian signed integer (`high × 65,536 + low`). Aston and entab use `high × 65,534 + low`; no file of this first set crosses a 65,536 boundary between two escapes, so these files cannot decide it. We keep the positional reading (settled by the version-181 comparison in the next entry).
- Sample interval: `(last − first) / (n − 1)` from the header times and the decoded point count. It is exactly 400 ms in `mwd1A.ch` (version 30) and 50 ms in `test_179_fid.ch`; in `test_fid.ch` (version 81) the header range is 2,699 × 200 ms but the body holds 2,699 values, so the derived interval is 200.07 ms; `check` reports this as `time_range_mismatch` (info).
- The `.uv` scale factor is read at 0x0C0D (big-endian f64, 1,000/2²¹ in `dad1.uv`); Aston divides by 2,000 instead; rainbow's page places it at 0x127C, which is past the 0x1000 header of this file. The value at 0x0C0D reproduces rainbow's output (run as an oracle).

**Oracles** (run only): rainbow-api 1.5.2 (LGPL-3.0) for versions 30, 130, 131, 179, 181 and `.ms`; Aston (BSD-3) for version 81. See `oracle/gen.py`.

## 2026-09-22 — more versions, the directory reader, oracles (Richard Zimring with Claude as assistant)

**Corpus files added** (all in `corpus/manifest.toml`; GitHub files pinned to a commit and distributed under the repository's MIT license; MetaboLights under the EMBL-EBI Terms of Use):
- `chemplexity-011f0601-fid1a-ch` (chemplexity/chromatography-gui, version 81, 45,256 values, larger signal swings and 125 escapes).
- `chromhandler-001f0101-d` (FAIRChemistry/Chromhandler, `FID1A.ch` and `TCD2B.ch`, version 181), `chromhandler-ca10-100um-d` (version 30 DAD A–E), `chromhandler-rau-r505-00-data-ms` (version 2, `GC / MS Data File`).
- `zhulong-001-1-sm-d` (ekwan/zhulong: version 130 `DAD1A.ch`, version 131 `DAD1.UV`), `autolab-001-p1-a1-a1-d` (anababnigg/auto-lab: version 130 `VWD1A.ch`, two version 2 `MSD Spectral File`s).
- `mtbls75-x-fsfa-hl-gc-o7c-1-d` (MetaboLights MTBLS75, zipped `.D` of a GC-MS run: `DATA.MS` `GC / MS DATA FILE`, `acqmeth.txt`, `runstart.txt`, `.ini` files).

**Inferred from the new files.**
- **Version 181 is not an f64 body.** The hex dump of `FID1A.ch` shows `7FFF 0000 0000 970C 7FFF …` from 0x1800: the version-81 second-difference encoding (every sample an escape in this file), 3,600 values plus one final `0x0000` word. The header is the 0x1800 UTF-16 layout of 179 (times f32 at 0x11A/0x11E: 0 and 719,800 ms; scale 1/7,680 at 0x127C; no signal string at 0x1075). 3,600 values over 719,800 ms is exactly 200 ms with `n − 1` intervals, so the final zero word is a terminator, not a sample. rainbow's documentation page groups 179 and 181 as "doubles"; the file contradicts it for 181, and we follow the file.
- **Escape multiplier.** rainbow (run as a black box) decodes `chromhandler-001f0101-d` to values equal to ours with `high × 65,536 + low` (maximum difference 0.0) and differs by up to 0.24 pA with 65,534; together with the positional reading this settles 65,536. The version-81 oracle therefore runs Aston's own `AgilentFID.total_trace` with that one constant changed (documented in `oracle/gen.py`).
- **Version-81 interval.** Both version-81 files span exactly `n` intervals (2,699 × 200 ms, 45,256 × 100 ms) between the header times, not `n − 1`; we use `(last − first) / n` for version 81 and `(last − first) / (n − 1)` for the others (exact 400 ms, 50 ms, 200 ms in the 30, 179 and 181 files). This supersedes the `time_range_mismatch` note above; `check` now reports the info only when the interval is not a whole number of milliseconds (seen: `VWD1A.ch` of `autolab-001-p1-a1-a1-d`, 144 values over 41,642 ms, whose values nevertheless equal rainbow's).
- Version 130 bodies are the version-30 delta records after a 0x1800 header (zhulong `DAD1A.ch`, auto-lab `VWD1A.ch`); times are i32 at 0x11A like version 30.
- GC-MS `.ms` files (`GC / MS Data File`, also upper-case `GC / MS DATA FILE`) use the same scan records; their header scan count is the little-endian u16 at 0x142 (rainbow documentation), and the walk counts the records.
- MS scans store their pairs in descending m/z; `read_spectrum` reverses them. Pairs never repeat an m/z within a compared scan (the oracle, which bins at 0.05 m/z, would merge repeats).

**Oracle results (2026-09-22):** 11 ChemStation corpus inputs pass `cargo test -p openreadout-corpus-tests --features corpus`: 424 trace channels bit-identical (xxh3 of the scaled f64 values, first samples, point counts, sample rates, first/last retention time) and 643 spectra bit-identical (m/z and intensity arrays; up to 200 evenly spaced scans per run of 43–4,941 scans). Adjustments to oracle output, all listed in the oracle JSON `oracle_note`: rainbow returns one extra value for version 181 (the terminator word), which the oracle drops; Aston's 65,534 constant is patched to 65,536 for version 81; Aston's fixed 0.2 s time axis is not compared.

## 2026-09-23 — the acquisition method text (`acqmeth.txt`)

**Scope:** `traces[].extra.method_file` / `spectra[].extra.method_file` (new) from the `.D` directory's `acqmeth.txt`, for the experiment model's method parameters. Signal decoding is unchanged.

**Corpus files used:** every ChemStation input; only `mtbls75-x-fsfa-hl-gc-o7c-1-d` (MetaboLights MTBLS75, GC-MSD, a `.D.zip` extracted by the corpus fetcher) holds an `acqmeth.txt` (Latin-1, CRLF, 7 KB). No LC `.D` in the corpus has one (`entab-*`, `chromhandler-*`, `zhulong-*`, `autolab-*` hold only signal files, `result.ini`, `RUN.LOG`, `SAMPLE.MAC`, `SAMPLE.XML` or `MSPARMS.txt`), so no LC pump timetable is read. The text is the method report ChemStation prints: `INSTRUMENT CONTROL PARAMETERS: GCMSD_1`, the method path (`C:\MSDCHEM\1\METHODS\PNNL_METABOLOMICS.M`), then sections with `Label   value unit` lines: `Oven` (`Oven Program On`, `60 °C for 1 min`, `then 10 °C/min to 325 °C for 10 min`, `Run Time 37.5 min`), `Front Injector` (`Injection Volume 1 µL`), `Front SS Inlet He` (`Mode Splitless`), `Column #1` (name, then `325 °C: 30 m x 250 µm x 0.25 µm`), and `MS ACQUISITION PARAMETERS` (`Solvent Delay : 6.50 min`, `Low Mass : 50.0`, `High Mass : 600.0`) and `TUNE PARAMETERS for SN: US92032548`.

**Inferred:** the oven program is one step per line (first: start temperature and hold; `then R °C/min to T °C for H min`: ramp rate, target, hold), ending at the first `Run Time`; the injection volume is the `Front Injector` section's `Injection Volume` in µL; the inlet mode the first `Mode` line after an `Inlet` heading; the column its name line and the `length x diameter x film` line; the `SN:` after `TUNE PARAMETERS for` is the mass-selective detector's serial number. Lines that do not match are ignored; the whole text stays in `info --view full` (`vendor.text_files`).

**Prior art consulted:** none (the text is a human-readable report).

## 2026-09-24 — Scan headers without peaks (`spectra`, `Dataset::visit_scan_headers`)

**Scope:** `.ms` scans from the record index built at open (RT, pair count) and the record's TIC: the u32 six bytes into the trailer after the pairs, four bytes read per scan, no pair decoded (the same field `ms_pairs` returns).

**Validation:** `crates/openreadout-corpus-tests/tests/scans.rs` compares every oracle scan's header (scan number or native id, MS level, RT, polarity, filter, precursor m/z, charge, activation, isolation target, collision energy, 1/K0, position) with the corpus oracles (rainbow-api/Aston: 4 data sets, 643 scans), and on up to 40 scans per file checks that the header equals the decoded spectrum's metadata.

**Prior art consulted:** none new.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the signal and MS file versions, encodings and the header scaling (applied). The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-26 — Vendor peak reports in `.D` directories (`Report.TXT`, `RESULTS.CSV`) and the MSD TIC export (Richard Zimring with Claude as assistant)

**Files:** `chromhandler-001f0101-d` … `chromhandler-001f0104-d` (GC, FID1 A + TCD2 B, each with ChemStation's `Report.TXT`), `chromhandler-rau-r505-01/03/05/08/12-d-*` (GC-MSD `data.ms` + `RESULTS.CSV`), and new: the remaining RAU-R505 runs 00, 02, 04, 06, 07, 09, 10, 11 and every run's `tic_front.csv` (FAIRChemistry/Chromhandler @5b5b0f4, `docs/usage/data/agilent_csv/`, MIT). No held-out record is used.

**Prior art consulted:** none. The report layouts are self-describing text; `rteres.txt` in the same directories (MSD ChemStation's printed "Area Percent Report" of the same integration) names the `RESULTS.CSV` columns in words ("peak #", "R.T. min", "first/max/last scan", "PK TY", "peak height", "corr. area", "corr. % max.", "% of total").

**What was inferred from what:**
- `Report.TXT` is UTF-16LE with a BOM and CRLF lines. After a `Signal <n>: <detector> <letter>, …` line, a table is two header lines, a separator line of dash runs each closed by `|`, then rows up to `Totals :` or a blank line. The separator gives the column extents (the `|` position belongs to the column on its left); the two header lines, cut at the same extents, give the column name and its unit in brackets (`Area [pA*s]`, `Height [25 uV]`, `Area %`). In the four reports: `Peak #`, `RetTime [min]`, `Type`, `Width [min]`, `Area`, `Height`, `Area %`. The signal name maps to a `.ch` file by removing the space (`FID1 A` → `FID1A.ch`), case-insensitively.
- Against our decoded `.ch` traces: every reported retention time is within one sample of our trace's local maximum (13 peaks); every reported height is within 0.8 % of our apex minus a linear baseline 3 peak widths each side; areas summed over ±3 widths are within 6 % (the window is ours: the report has no integration limits, so areas are not checked). The units in brackets equal the `.ch` header units (`pA`, `25 uV`).
- `RESULTS.CSV` (MSD ChemStation) is an INI-like text: `[contents]` lists sections; a section `[INT TIC: <file>]` holds `Time=`, `Header=` (quoted column names `Peak, R.T., First, Max, Last, PK  TY, Height, Area, Pct Max, Pct Total`) and one `<n>=,` row per peak. `First`/`Max`/`Last` are 1-based scan numbers: at 0-based scan `Max − 1` our scan time is within one scan of `R.T.` and our TIC within 10 % of `Height` (46 peaks, 12 runs; the vendor apex is interpolated); with `Max` taken 0-based the times are off by one scan and heights by up to 70 %. `Area` is not our sum of TIC counts (about 3.6× it, varying with the peak type), so areas are reported as written, not checked.
- `tic_front.csv` is a TIC export (a header line, `Start of data points`, then `minutes,counts`, minutes to six significant digits): in all 12 runs every point (24,144) equals our TIC, times within the printed precision and counts exactly.

**`Result.xml` (same day).** `gc2asm-v181-d` and `gc2asm-three-channels-d` (already in the corpus; GC2ASM, CeCILL-2.1) hold ChemStation's XML export: UTF-16, root `ChemStationResult`; `Chromatograms/Signal` elements (`Description`, `YUnits`) with one `IntegrationResults` per integrated peak (`RetTime`, `Area`, `AreaPercent`, `Height`, `Width`, `Symmetry`, `TimeStart`, `TimeEnd`, `LevelStart`, `LevelEnd`, `BaselineStart`, `BaselineEnd`), `Results/ResultsGroup/Peak` compound results (`CompoundID`, `SignalDesc`, `PeakType`, `ExpRetTime`, `MeasRetTime`, `Area`, `Height`, `Name`, `Amount Unit="% v/v"`) and `CalibrationInformation`. Inferred: integrating our decoded trace from `TimeStart` to `TimeEnd` above the straight line from `BaselineStart` to `BaselineEnd` reproduces `Area` (pA·s: pA × min × 60) to a median ratio of 0.99987 (24 peaks above 1 % of the largest) and 0.99999999 (42 peaks); the six that differ by more than 5 % all have a flagged peak type where one is stated (`BV E`, `VV R`, `VB E`: skimmed or reconstructed baselines) or sit among overlapping peaks. Heights measured above the same baseline agree within 10 % for all but one peak per file (a larger neighbour inside the half-width window). Each compound result's `MeasRetTime` equals one `IntegrationResults` `RetTime` of its signal (to 1e−6 min), which is how they are joined. `LevelStart`/`LevelEnd` are not used (their meaning was not established).

**Also fixed:** the `acqmeth.txt` of a `.D` directory was read with `std::fs` instead of the data set's byte source (`Fs`), so a directory opened from memory or a host source lost its method; it now goes through `Fs` like every other file.

## 2026-09-26 — `.uv` records of tag 70 (Richard Zimring with Claude as assistant)

OpenLab CDS DAD spectra parts (`docs/provenance/openlab-cds.md`, same date) are version-131 `.uv` files whose records carry tag 70 and little-endian f64 values after the usual 22-byte record header; the reader decodes them (`ScanRecord.float`) and scales them by the header's scale factor twice (checked against rainbow-api `parse_uv`, which applies it once, and against the DAD's own channels, which fix the unit). Tag-67 files are unchanged.

## 2026-09-26 — Instrument model: `GCI` is not a model; `Result.xml` modules; detector from the file name's last part (Richard Zimring with Claude as assistant)

**Files:** `gc2asm-v181-d`, `gc2asm-three-channels-d` (GC2ASM, CeCILL-2.1: `.ch` files with `Result.xml`), `chromhandler-001f0101-d` … `-0104-d`, `entab-test-179-fid-ch`, `entab-test-fid-ch`, `chemplexity-011f0601-fid1a-ch` (development inputs; no held-out record).

**Prior art consulted:** none for the parsing. chromConverter 0.9 (GPL-3.0, R) was run as a black box by the second-opinion harness (`oracle/second_fields/chrom.py`); its `detector_id` for the version-179/181 GC files is the same `GCI` we read at 0x9BC, so it does not tell which is the model.

**What was inferred from what:**
- The 0x9BC string of every version-179/181 GC signal file in the corpus, from three unrelated laboratories, is `GCI`, while LC files hold module part numbers there (`G1315B`, `G1365B`) and version-8/81 GC files `HP G1530A` (the 6890 GC's part number). The two `gc2asm` directories also hold ChemStation's own `Result.xml` export, whose `ModuleInformation/Module` names the instrument: `ModuleName` `Agilent 6890 GC` (`SerialNumber` `CN10809013`, `FirmwareRevision` `N.05.06`, `PartNumber` `6890`) and `Agilent 7890A` (`CN10834060`, `A.01.09`, `7890A`). The LC file `autolab-001-p1-a1-a1-d/VWD1A.ch` (version 179, a VWD) holds `GCI` there too. `GCI` is therefore a generic tag, not a model: the experiment's instrument model skips it (the files without `Result.xml` now report no model rather than `GCI`; `extra.instrument` keeps the stored text).
- `Result.xml` modules are read (`ResultModule`: `name`, `serial`, `firmware`, `part_number`) and given as `extra.result_modules` on every signal; the first module's name is the experiment's instrument model and its serial number the instrument serial, ahead of the signal header's 0x9BC string.
- `extra.detector` was the letters that start the file stem, which for files renamed by their depositors (`entab-test_fid.ch`, `chemplexity-011F0601-FID1A.CH`) gave `ENTAB` and `CHEMPLEXITY`. ChemStation names signal files `<detector><n><letter>` without separators (`FID1A.ch`, `mwd1A.ch`, `dad1.uv`), so the letters are now taken from the stem's last part after `-`, `_`, `.` or a space: `FID` for both; unchanged for the vendor's own names.
