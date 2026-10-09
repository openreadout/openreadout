# Provenance log — plate-reader exports

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

Plate readers write their results as text, CSV or XLSX exports that the vendor software produces for people to open in a spreadsheet. None of these exports is a binary container and none is encrypted; their structure is visible in the files themselves. The binary SoftMax Pro document (`.pda`) is out of scope: no public sample with a redistribution licence exists, and it is only detected (exit 6).

## 2026-09-22 — initial derivation (Richard Zimring with Claude as assistant)

**Prior art consulted** (permissively licensed; source and test fixtures read, no code copied; our names come from the files and are listed in `docs/formats/plate-readers.md`):
- `allotropy` 0.1.146, Benchling Inc. and contributors, MIT (`LICENSE.txt`, "this software and associated documentation files"; the test fixtures are distributed in the same repository under that licence), https://github.com/Benchling-Open-Source/allotropy at commit `6ead3ec55b527cd66f65d5487b3757fadf71bb5c` (tag `v0.1.146`). Read: `src/allotropy/parsers/agilent_gen5/*` (Gen5 header keys before "Procedure Details", "Read"/"Filter Set" procedure lines, result matrices with a trailing `label:wavelength` column, kinetic "Time" tables with a `T° <wavelength>` column, filename pattern for plate barcodes), `moldev_softmax_pro/softmax_pro_structure.py` (`##BLOCKS=` framing, `~End` terminators, the positional fields of the `Plate:` header line for absorbance/fluorescence/luminescence, PlateFormat vs TimeFormat layouts, wavelengths side by side separated by one empty column, spectrum wavelength = start + step × i), `bmg_mars/bmg_mars_structure.py` (header `key: value` pairs, `Raw Data (ex/em)` label grammar), `perkin_elmer_envision/*` (section titles "Plate information", "Background information", "Results for …", "Calculated results: …", "Basic assay information", "Labels:", "Filters:", "Instrument:"), `tecan_magellan/*` (list layout: value column + "Well positions" column; trailing metadata lines), `thermo_skanit/*` (one sheet per step, "General information"/"Instrument information" sheets), `bmg_labtech_smart_control/*` (numbered sections "4. Raw Data (ex/em)"), `revvity_kaleido/*` ("Results for …" sections, "Measurement Basic Information"). It is also run as the black-box oracle (`oracle/gen.py`, `oracle/asm_compare.py`).
- The ASM plate-reader output shape (key names such as "plate reader aggregate document", "measurement aggregate document", "device control document", cube layout `{"cube-structure": {...}, "data": {...}}`) was taken from allotropy's MIT-licensed expected-output fixtures (`tests/parsers/*/testdata/*.json`) and checked against the public Allotrope Simple Model JSON schema (below).

**Standard consulted:** Allotrope Simple Model plate-reader schema `REC/2025/03` (`http://purl.allotrope.org/json-schemas/adm/plate-reader/REC/2025/03/plate-reader.schema`), from the public Allotrope repository https://gitlab.com/allotrope-public/asm at commit `1c8c466a3fe4ad803e1a144d48971d9728ab4123` (2026-07-31). Licence (`LICENSE.md`): CC BY-NC 4.0 for non-commercial use, CC BY-ND 4.0 for commercial use, or the Allotrope member licence. Consequences we follow: the schema files are **not** copied into this repository or the binary (no derivative is distributed); `oracle/asm_validate.py` downloads the unmodified schema at a pinned commit into a gitignored cache and validates our output with `jsonschema`. Our writer emits documents that conform to the schema; it does not reproduce or modify the schema. Whether producing ASM documents in a commercial context needs the member licence is flagged for the attorney review in `docs/legal/clean-room-policy.md` (preamble).

**Corpus files used** (all in `corpus/manifest.toml`):
- allotropy test fixtures (MIT, commit `6ead3ec`): `gen5-abs450-non-numeric`, `gen5-fluor-filter-step-label`, `gen5-lum-endpoint`, `gen5-multiple-read-modes`, `gen5-kinetic-growth-curve`, `gen5-abs-spectrum`, `softmax-abs-endpoint-plates`, `softmax-fl-kinetic-plates`, `softmax-lum-endpoint-utf16`, `softmax-spectramax340-kinetic-partial`, `bmg-mars-pherastar-abs`, `bmg-mars-abs-384-qc`, `bmg-mars-fi-transcreener`, `bmg-mars-lum-1536`, `bmg-smart-control-fi`, `envision-abs-a450`, `envision-fluor-htrf`, `envision-lum-384`, `kaleido-abs-endpoint`, `magellan-pro-compact`, `magellan-elisa-384`, `skanit-luciferase`, `skanit-elisa-steps`.
- `jamesrco/ExoenzymeHydroCalc` (MIT, commit `0645f2d`): `tecan-icontrol-f200-txt` (Tecan i-control 1.9 text export from an infinite F200).
- `ACCakut/plate_reader_python` (CC0-1.0, commit `33796df`): `tecan-icontrol-kinetic-xlsx` (Tecan i-control 2.0 XLSX export from an infinite 200Pro: one endpoint read and a 35-cycle kinetic read).

**Method:** each file was dumped (text: tabs and line endings made visible; XLSX: every cell per sheet with python-calamine) and compared with allotropy's expected output for the same file. Observed and now handled:
- Encodings: UTF-8 with and without BOM, UTF-16 LE with BOM (SoftMax Pro), single-byte files that are not UTF-8 (`softmax-spectramax340-kinetic-partial`; Windows-1252 decoding is used, so `0xB0` → `°`). A Gen5 file writes `T∞ 600` where `T° 600` is meant (a Mac Roman `°` read as Windows-1252); the temperature column is found by position, not by that label.
- Line endings: CRLF, LF, and bare CR in the middle of a CRLF file (`bmg-mars-lum-1536` separates plate rows with CR only).
- Delimiters: tab (Gen5, SoftMax Pro, i-control `.txt`), comma with trailing commas and padding columns (BMG MARS, EnVision, Kaleido), and `="…"` Excel-formula quoting of barcodes (EnVision: `"=""LoP_pool"""`).
- Plate matrices: header row of column numbers (`1…12`, `01…12`, `<>` corner in i-control), row labels `A…P` and for 1536-well plates `A…Z` then `a…f` (`bmg-mars-lum-1536`, 32 × 48). Partial plates leave cells empty (BMG 384 QC, SoftMax partial) or omit rows (EnVision 384 luminescence: only rows A, E, I, M).
- Gen5: one row label followed by one line per read (`A\t…\tod:600`, `\t…\tfluor:579,616`), so a matrix row spans several lines; kinetic tables list wells as columns and time points as rows (`0:04:22`), padded with empty `0:00:00` rows after the last cycle; spectral scans have a `Wavelength` column.
- SoftMax Pro: `Plate:` header fields by position differ between absorbance and fluorescence (fluorescence has an extra top/bottom read field after the read mode); PlateFormat puts several wavelengths side by side separated by an empty column; TimeFormat lists wells as columns.
- Non-numeric cells: `OVRFLW`, `MISSED` (Gen5), `Range?` / `Path?` (SoftMax Pro), `NaN` (SkanIt), decimal commas in a dot-decimal file (`0,02` in `softmax-abs-endpoint-plates`). All are reported as non-numeric (value NaN) with the original text kept, as allotropy does (it emits an error document with the text).
- Dates: `4/11/2024` + `5:27:15 PM` (Gen5, US order), `29/02/2016` (BMG, day first, detectable because 29 > 12), `20.02.2024` (i-control, German order), ISO with offset (Kaleido). Ambiguous day/month pairs are read month first and flagged `date_order_assumed`.

**Inferred, not yet corroborated:** the Gen5 area-scan and multi-plate layouts (allotropy excludes its own multi-plate fixtures; not in our corpus), SoftMax Pro well-scan and fluorescence-spectrum blocks (allotropy rejects them), Tecan Spark exports, and the `.pda` binary (no public sample).

## 2026-09-22 — robustness (fuzzing)

**Scope:** a loop guard only; parsing is unchanged. The `whole_plate` fuzz target (3 min; seeds: allotropy fixtures, MIT) found SoftMax Pro text exports whose plate header declares 0 rows: the PlateFormat reader advanced by the row count and so never moved (a hang). The row count is now at least 1. Fixtures: `crates/openreadout-plate/tests/fixtures/malformed/file-fuzz-softmax-zero-rows*.txt`. **Prior art consulted:** none.

A second 3-minute `whole_plate` run found a 1.4 KB SoftMax export placing a value at a row index near 2^32 (the header's first-row field plus the row offset): the plate was sized from it and its well-name list took 1.8 GB and 6 s. Values beyond 1024 rows or columns (the largest standard plate is 48 x 72) are now skipped with one `well_out_of_range` warning, and the non-standard plate size saturates. Fixture: `file-fuzz-well-at-row-4-billion.txt`.

The `whole_plate_xlsx` fuzz target (3 min; seeds `magellan-pro-compact.xlsx`, `skanit-luciferase.xlsx`, MIT) found an overflow in calamine's cell-reference parser (upstream; a panic only with debug assertions). Reading calamine's `Range::from_sparse` (MIT) to understand it showed that `worksheet_range` allocates the dense bounding box of all cells. XLSX worksheets are now streamed cell by cell (`worksheet_cells_reader`, same values), a worksheet may span at most 4 Mi cells, and a panic inside the workbook parsers becomes a corrupt-file error. Fixtures: `file-fuzz-calamine-cell-reference-overflow.xlsx` (fuzzer output) and `file-crafted-two-cells-spanning-the-sheet.xlsx` (written by hand: cells A1 and XFD1048576).

## 2026-09-23 — SkanIt `Layout definitions` sheet

**Scope:** SkanIt XLSX reports: the `Layout definitions` sheet is now read into a new table key `extra.layout_definitions` (the existing flat `extra.layout` of sample names is unchanged). It feeds the plate-analysis layouts (`openreadout analyze assay`, book/src/guides/plate-analysis.md).

**Corpus file used:** `skanit-elisa-steps` (allotropy test fixture, MIT, commit `6ead3ec`). **Prior art consulted:** none (allotropy does not read this sheet).

**Inferred from the file:** the sheet starts with `Name` → plate name (`Plate 1`) and `Plate template` lines, then a header row whose first cell is empty and whose other cells are the column numbers (`1.0 … 12.0`); each plate row takes up to three lines: the row letter and the sample names (`Std0001`, `Un0001`, `Blank1`), then (first cell empty) the group (`Group 1`), then the standard concentration with its unit (`1 microg/ml`) or the dilution of an unknown (`1:2`); a line whose first cell is blank or a single space ends the block. We store the three lines per well under the titles `Sample`, `Group` and `Conc/Dil` (the last as written; the same meaning as Gen5's `Conc/Dil` layout line). The values agree with the `Sample` matrices of the step sheets and with the dilution factors of the `Dilution Factor 1` step (`1:25` → 25).
## 2026-09-23 — calculated reads named in `table`, EnVision formulas (Richard Zimring with Claude as assistant)

**Scope:** labelling only; values are unchanged. The eval analysis tier found that a Gen5 luminescence export with a normalization step (`gen5-lum-endpoint`: `LUM:Lum` and `NormLum` lines per plate row) reached `table` users as `read` 1 and 2 with nothing in the rows or the column labels saying that read 2 was computed by Gen5 (it was flagged `calculated` only in `info` → `tables[].extra.reads`), so "the well with the highest luminescence" could be answered from the normalized percentages.

**Inferred (ours), from the exports themselves:**
- Every read now carries `origin`: `measured` or `calculated` (the existing `calculated: true` is kept). The rules that mark a read calculated are unchanged: Gen5 data labels that name no procedure read (`NormLum`, `normDAPI`); SoftMax Pro `Reduced` columns; BMG MARS and SkanIt matrices after the raw data (`Blank corrected based on …`, later protocol steps); EnVision `Calculated results:` titles.
- The `read` column's label (shown by `table`, `export` CSV label lines and the MCP tool) now lists each read with its origin when a table holds calculated reads: `1 = LUM:Lum (luminescence, measured); 2 = NormLum (CALCULATED by the vendor software, not measured)` (tables with more than 16 reads list only the calculated read numbers).
- EnVision exports end with a `Calculations:` section: per plate, ` Formula index` (`Calc 1`), ` Formula name` and ` Formula` (`envision-fluor-htrf`: `HTRF ratio value for AC HTRF Laser [Eu]`; `envision-lum-384`: `Crosstalk where Label : 0_1 US LUM 384(1) channel 1`). A calculated read titled `Calc N: …` takes the formula of index `Calc N` as `extra.reads[].formula`. `envision-abs-a450` writes `No calculations.`. No other corpus export stores a formula (Gen5 writes only the data label; SoftMax Pro group formulas are in the kept-verbatim group tables).

**Corpus files used:** the 25 plate-reader inputs (allotropy fixtures, MIT; see above). **Prior art consulted:** none new; allotropy (MIT) reports Gen5 calculated reads as calculated-data documents, which our ASM output already mirrors.

## 2026-09-24 — i-control 1.11 comma-delimited CSV and SparkControl CSV (Richard Zimring with Claude as assistant)

**Trigger:** finding M3 of the first held-out draw (2026-09-24): an i-control 1.11 comma-delimited export was detected but gave no values, and `check` exited 4. The held-out file was not opened; the layout was derived from the two new development files below.

**Corpus files added:**
- `tecan-icontrol-csv-kinetic-wellr` — `tecanON1.csv` from the wellr R package (GitHub BradyAJohnston/wellr @3fa4a8d, `inst/extdata/`; MIT, LICENSE checked 2026-09-24): i-control 1.11.1.0, infinite 200Pro, comma-delimited with a UTF-8 BOM, 120 kinetic cycles of absorbance (595 nm) and luminescence, 96 wells.
- `tecan-sparkcontrol-csv-kinetic-flopr` and `tecan-sparkcontrol-csv-endpoint-flopr` — members of `flopr-data.zip` (Zenodo 3977408, FlopR, CC-BY-4.0, checked on the record page 2026-09-24): SparkControl 2.3 comma-delimited exports of a Spark reader, a 24-hour kinetic run (OD600, OD700, GFP, mCherry) and an endpoint calibration plate (2 absorbance and 9 fluorescence gains).

**Observed (text read in an editor; no vendor document):**
- i-control 1.11 CSV: the header lines are the tab export's, but key and value sit in columns 1 and 5 (`Plate,,,,Greiner 96 …`). There are no `Label:` lines. The read settings come first, as one `Mode` group per label (`Mode,,,,Absorbance` / `Wavelength,,,,595,nm` …; `Mode,,,,Luminescence` / `Integration Time,,,,1000,ms`), after `Kinetic Measurement` / `Kinetic Cycles` / `Interval Time`. Each label's data follow under a title line `<name>:<suffix>` (`OD600:600`, `LUMI:Lum`) as a table with **one line per cycle**: `Cycle Nr.,Time [s],Temp. [°C],A1,A2,…` then `1,0,37.2,0.0459,…`. Titles are matched to the `Mode` groups in order (two of each here).
- SparkControl CSV: a `Method name:` line precedes `Application: SparkControl,,,,V2.3`. Each read is a `Mode,<mode>` / `Name,<label>` group of `key,,,,value,unit` settings (`Measurement wavelength`, `Excitation wavelength`, `Gain,,,,125,Manual`, …; `Mode,Kinetic` groups hold the kinetic settings). Kinetic data: a line with the label name, then `Cycle Nr.,1,2,…`, `Time [s],…`, `Temp. [°C],…` and one line per well (the tab export's orientation). Endpoint data: after `Start Time` and `Temperature` lines, `<>,Value,Time [ms]` and one line per well (`A1,0.0921,0`). The degree sign is a lone Windows-1252 byte after a UTF-8 byte-order mark (read with the existing fallback).

**Inferred:** a label's section is the `Mode` group, matched to a data table by its name when the table has a title equal to a `Name`, else by position; the channel label is the title's part before `:` (`OD600`), the full title is kept in the settings (`section_title`). Units as for the tab export (OD, RLU, RFU).

**Ground truth:** allotropy has no i-control or SparkControl parser, so `oracle/plate.py` gained `tecan_csv_summary`, an independent pandas reader of the CSV text written from the observations above (not from our code): it finds every table by its `Cycle Nr.` or `<>` header, reads it with pandas in whichever orientation it has, and summarizes the values per detection mode (the `Mode` groups in order) exactly as the existing plate oracle does. It is a second reading of the same text, not a vendor-independent measurement: agreement shows that the parser reads every value from the right cell into the right well and mode.

**Prior art consulted:** none.

## 2026-09-24 — Gen5 exports without their file header (N-M2 of the second generalization report)

**Problem:** a Gen5 export saved without its file header (no `Software Version` … `Reading Type` block) was not recognised as Gen5. Its embedded `Layout` was ignored, so `assay curve` exited 2 with "no standard wells".

**Search for a real file** (2026-09-24): GitHub code search (`"Well ID" "Conc/Dil"` returned 106 files; header-less exports were separated with `NOT "Software Version"`), Zenodo, Figshare, Dryad, OSF, Mendeley Data. Three depositors have header-less exports with a layout and standards: RobertsLab/sormi-assay-development, jalapic/mouse_socialhierarchy_immune and OSF 5gtn7. None has a licence, so none can be used. The licensed repositories with such files (allotropy and its forks, MIT; LennonLab/StarvationTraits, GPL-3.0) all keep the header. biocore/kl-metapool (MIT) has header-less exports, but in the per-well column form (`Well ID`, `Well`, `[Blanked-RFU]`, `[Concentration]`) with no layout grid; it is recorded as a gap below.

**Development files:** `synthetic-gen5-headerless-stdcurve-linear` and `synthetic-gen5-headerless-kinetic-meanv-4pl`, made by `oracle/make_gen5_headerless.py` from `gen5-abs-stdcurve-linear` and `gen5-abs-kinetic-meanv-4pl` (allotropy test data, MIT). Every line before `Layout` is dropped and nothing else changes. The second file also loses its procedure (the kinetic loop and the unnamed 420 nm read), which is the harder case.

**Prior art consulted:** none. The reads-from-labels rule comes from the label forms in the corpus's own Gen5 exports (`od:600`, `fluor:579,616`, `LUM:Lum`, a bare `420` title), which are already documented above.

**Ground truth:** `oracle/assay.py` cases `gen5-linear-headerless` and `gen5-meanv-4pl-headerless`. These are the same checks as for the originals (numpy/SciPy refits and Gen5's own `[Concentration]` values and curve-fit parameters, which are unchanged in the files), without the drc checks, which need R with drc. `assay curve` returns byte-identical `curve` and `wells` output for each header-less file and its original.

**Gap:** the per-well column export form without a header (biocore/kl-metapool: `Results`, then `Well ID<TAB>Well<TAB>[Blanked-RFU]<TAB>[Concentration]` rows for 384 wells, and the fit table after them, beyond the first 200 lines) is still not recognised (exit 3). Reading it needs a Gen5 column-table parser, which is follow-up work.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the export dialect, export format, read type and modes, the text container and delimiter (through `Dataset::assurance_observations`), and assumed date orders and read modes. The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-25 — SoftMax Pro binary documents: `.pda` (SoftMax Pro 5) and `.sda`/`.pda` (SoftMax Pro 6/7) (Richard Zimring with Claude as assistant)

**Why:** the binary documents were detected and refused (exit 6). They are what labs keep; no open reader exists.

**Search for public files** (2026-09-25): Zenodo file-type search (`filetype:pda`, `filetype:sda`, text searches `SoftMax`, `SpectraMax`), figshare (`SoftMax Pro`), Harvard Dataverse and 22 other Dataverse installations (file-name search), GitHub code/repository search (binaries are not indexed; repository trees of repositories holding SoftMax text exports were listed). Usable (licensed, public, not a held-out source):

| corpus id (planned) | source | licence | what |
| --- | --- | --- | --- |
| `softmax5-kinetic-phage-120524` + `.txt` export | Zenodo 4976789 (Dryad mirror, Refardt et al., E. coli phage lysis), `120524_1.pda`/`.txt` | CC0-1.0 | SoftMax Pro 5.42.1.0, SPECTRAmax M2e, kinetic absorbance 600 nm, 721 planned reads every 120 s, stopped after 239, 96 wells; TimeFormat export |
| `softmax5-kinetic-phage-120308b` + export | same record, `120308b_2.pda`/`.txt` | CC0-1.0 | same instrument, 559 of 721 reads; the export's values are plate-blank subtracted (blank group = column 12) |
| `softmax5-elisa-il10`, `softmax5-elisa-tnf` + exports | Zenodo 7607772 (CSF1R/CD40 spheroid study), `20220523_*_SupernatProt1_7dExp.pda`/`.txt` | CC-BY-4.0 | SoftMax Pro 5.4.12.1.0, SPECTRAmax M5, endpoint absorbance 595 nm, 96 wells; PlateFormat export |
| `softmax5-cuvette-spectra-*` | figshare 7455404 (Iowa State, gel nanocomposites), `Figure S1a black/red.pda`, `Figure S2.pda` | CC-BY-4.0 | SoftMax Pro 5.4.12.1.0 cuvette-set absorbance spectra (plate sections empty); no export, readme gives dimensions |
| `softmax7-lum-7skexp53` + export, `softmax7-prestoblue-*` + exports | UNC Dataverse doi:10.15139/S3/4ESRC1 (Turner et al., BET degraders / HIV latency) | CC0-1.0 | SoftMax Pro 6/7 `.sda` with UTF-16 text exports: 1 luminescence document (7 plates) and 5 fluorescence documents (ex 555 / em 585, 1–5 plates each), SpectraMax M3 |
| `softmax7-elisa-m5` | Zenodo 3980703 (EPFL, SARS-CoV-2 epitopes), `ELISA.sda` | CC-BY-4.0 | SoftMax Pro 6/7, SpectraMax M5, 10 endpoint absorbance plates (605 nm); no export |

**Prior art consulted:** none. No open reader of either binary exists that we know of; allotropy (MIT) reads only the text export.

**SoftMax Pro 5 `.pda` — observed (hex dumps and differences between the four documents with exports):**
- Head: `SoftMax Pro\0` + one byte, or two bytes, then the version as text (` 5.42.1.0`, ` 5.4.12.1.0`) and `##BLOCKS= n` padded with spaces to a CR — the export's first line. The object stream starts at offset 2000 (`0x7D0`, a big-endian u32 in the head). All numbers are big-endian.
- Objects start with a one-byte length and a class name beginning `CS` (`CSExperimentSection`, `CSPlateSection`, `CSPlateData`, `CSPlateDescriptor`, `CSSite`, `CSCalcPlateBody`, `CSMorphPlateTable`, `CSGroupSection`, `CSCuvetteSection` …). Sections carry a NUL-terminated name (`Plate#1`, `Experiment#1`) and a u32 file offset; for plate, cuvette and notes sections it is the offset of the next section.
- `CSPlateData` (fixed layout): u32 section id (`700`, `100`; wells of the template refer to their section by it), u16 read type (0 endpoint, 1 kinetic, 2 spectrum in the cuvette files), u16 read mode (1 in every file, all absorbance), u16 `12` (the export's last strip / column count), u32 reads (kinetic points; spectrum points), u32 wavelength count, u32 first wavelength (nm; 595, 600), 5 zero bytes, f64 run time (s; 86400), f64 interval (s; 120), u8 temperature control on, f64 temperature set point (37). Defaults (300, 20) sit there for endpoint reads. The export header's kinetic points / run time / interval are these fields.
- `CSPlateDescriptor`: u8 1, u32 reads, then per read 8 bytes of which the second four are the read's temperature as f32 (24.2; 37.0/37.1 — the export's `Temperature(°C)` column, including the garbage it prints for reads after a stopped run); then u32 1, u32 reads, **u32 time stamp in seconds since 1904-01-01** (local time; 2022-05-23 12:25:58, 2012-05-24 08:25:53, 2012-03-08 15:06:23 — the dates in the file names), u16 1, u16 12, **u32 reads completed** (239 and 559 where the export has values for the first 239 and 559 time points; 1 for endpoint), u16 1, u8 1, u32 section id.
- `CSSite`, one per well in row-major order (well index 1 = A1 … 96 = H12): u32 1, u32 reads, u32 well index, u32 byte count `8 × (reads + 1)`, that many bytes of f64 values — read 0 … n-1 and one trailing value (0) — then a second array of the same size (not-a-number markers and 0 in every file; meaning unknown, not read). Reads that were not measured hold the NaN pattern `7FF0006500000000` (`7FF8…` in the endpoint files' second array); the export leaves those cells empty.
- Kinetic times: the export's `Time` column is read index × interval exactly (0:00, 2:00, …); no per-read time was found, so times are nominal.
- `CSCalcPlateBody` holds the reduction (`!Lm1`, `Vmax(!KinPlot,!VmaxPoints,!ReadInterval)`) and a string `SPECTRAmax M5 ROM v2.1.35 20May09` / `SPECTRAmax M2e ROM …` just before `CSMorphPlateTable`: instrument model and firmware.
- Template (inside `CSExperimentSection`, after `Template\0`): u32 group count; each group `CSTmplGroup` + name (`Blank`, `Clear`, `HK97`, `10 G +` …), f64, unit (`units/ml`), u32, u8, column name (`Concentration`), 8 bytes, 14 × `FF`, u32 sample count; each sample `CSTmplSample` + name (`BL`, `HK01`, `M02`), f64, 16 × `FF`, u32 well count; each well `CSWell` (plate) or `CSCuvetteWell` (cuvette set) + name (`A12`), u16 row, u16 column (1-based), u32 section id, u16 1000, section name (`Plate#1`), u32 0. The export's `Group:` blocks list the same samples and wells.

**Validation:** every value of the four documents with exports agrees with the export: 192 endpoint values and 22,944 kinetic values exactly, 46,272 unmeasured cells empty in both; in `120308b_2` the export holds raw − mean(blank group wells A12…H12) at each read, and our raw values reproduce all 53,664 exported values within the export's last digit (max difference 5 × 10⁻⁷); temperatures of all completed reads within 0.005 °C. Checked by `scratchpad` scripts first, then by corpus tests against allotropy run on the exports.

**SoftMax Pro 6/7 `.sda` — observed:** a self-describing little-endian tree. `\x0bBinary File` (a .NET length-prefixed string), u32 version 1, then entries `u32 tag` (type code in bits 8–15, other bits zero) + name (7-bit length-prefixed UTF-8) + value: `0x04` string, `0x08` f64, `0x0C` i32, `0x10` byte array (u64 length; a nested `Binary File` inside is parsed the same way), `0x14` object (the u64 size counted from the tag comes before the name, then a u32 marker, then child entries), `0x18` 8-byte date/time, `0x1C` f32, `0x20` bool (1 byte), `0x28` 2 bytes, `0x2C` 4 bytes. Every file parses to its last byte with these codes. Sections are a class-name string (`SoftMaxPro.DataPersistence.SerializablePlateSectionData`, `…NoteSectionData`, `…GroupSectionData`, `…GraphSectionData`) followed by the section object: `Name`, `ID` (`Plate_<guid>`), the reader class (`SoftMaxPro.Readers.SettingsModel.SpectraMaxM3Settings`) and model (`SPECTRAmax M3`), a nested `ReaderSettings` tree (`ReadType.value__` 0 in every file; `ReadMode.value__` 0 fluorescence, 1 absorbance, 2 luminescence; `WavelengthSettings` → `WavelengthList` → `Wavelength0` with `Wavelength` or `ExcitationWavelength`/`EmissionWavelength`/`CutoffFilter`; `Plate.Microplate` with `MicroplateName` and `PlateSpecification.NumberOfWells/NumberOfColumns/NumberOfRows`; `SensitivitySettings`), an object of report strings (`DeviceInfo` `SpectraMax M3\nROM v3.0.22 16Feb11`, `TemperatureInfo`, `ReadDetails` `Start Read : 1:24 PM 9/1/2021`), then the data object: plate name, i32 (0 or 1), i32 1, i32 1, i32 columns, i32 rows, two f64, a list of wavelengths, each a list of reads, each read i32 index, f32 temperature, f64, **byte array of rows × columns f64 values (row-major)**, flags; then a byte array with one byte per well.

**Validation:** all 2,304 values of the six UNC documents (7 + 4 + 3 + 4 + 5 + 1 plates, luminescence and fluorescence) equal the exported values exactly; plate names and order equal the export's `Plate:` blocks; temperatures within 0.05 °C.

**Right or refuse (decided here):** only what the files above validate is decoded — SoftMax Pro 5 plate sections with read mode 1 (absorbance), endpoint or kinetic, one wavelength, 96 sites on a 12-column plate; SoftMax Pro 6/7 plate sections with `ReadType` 0 (endpoint), read mode 0/1/2, one wavelength, one read, rows × columns values. Any other plate section (several wavelengths, spectra, well scans, other modes, other plate sizes, cuvette sets) is listed and refused with a finding; a document with nothing decodable exits 6 with a hint to export text.

## 2026-09-26 — Gen5 experiment files (`.xpt`), Gen5 Excel exports (Richard Zimring with Claude as assistant)

**Why:** Gen5 keeps its data in `.xpt` experiment files; no open reader exists. Their Excel exports were not read either (0 values).

**Files** (Zenodo file-type search `filetype:xpt`, then records holding Gen5 exports; Harvard/DataverseNL search found more `.xpt`, DataverseNL is behind a bot challenge and was not used):

| corpus id | source | licence | what |
| --- | --- | --- | --- |
| `gen5xpt-cytation5-elisa-ifng` (+ `-xlsx`) | Zenodo 14230362 (Benboubker, colorectal cancer drug combinations) | CC-BY-4.0 | Gen5 3.15.15, Cytation5, absorbance 450/540/570, full plate; Excel export (only columns 1–4 of it kept by the depositor) |
| `gen5xpt-cytation5-elisa-region` (+ `-xlsx`) | same record | CC-BY-4.0 | same reader, region A5..H8 read |
| `gen5xpt-cytation3-lum-5plates` | same record | CC-BY-4.0 | Gen5 3.11.19, Cytation3, 5 plates, luminescence (no export) |
| `gen5xpt-synergyht-abs-calibration` (+ `-xlsx`) | Zenodo 17453979 (Empa, P. aeruginosa biosensor) | CC-BY-4.0 | Gen5 3.04.17, Synergy HT, absorbance 580/583, full plate; complete Excel export |
| `gen5xpt-synergyh1-fl-kinetic` (+ `-xlsx`) | Zenodo 12108530 (Jones, protein microcrystals) | CC-BY-4.0 | Gen5 2.08.12, Synergy H1, kinetic fluorescence 60 reads, two reads; the depositor's `Raw data` sheet holds one read's exported values with times |
| `gen5xpt-synergyh1-fl-endpoint` | same record | CC-BY-4.0 | Gen5 2.08.12, 3 plates of fluorescence endpoint reads (no usable export) |
| `gen5prt-elisa-reader` | Zenodo 14230362 | CC-BY-4.0 | Gen5 protocol file (no data) |

**Prior art consulted:** Microsoft's public MS-CFB specification (compound file; our shared reader `openreadout-core::cfb`) and Microsoft's public documentation of the MFC `CArchive` object format (Technical Note TN002, "Persistent Object Data Format": `0xFFFF` new-class tag with schema, name length and class name; `0x8000 | index` tags for classes already written; `CString` lengths as a byte, or `0xFF` + u16, or `0xFFFF` + u32). zlib per RFC 1950/1951 (`miniz_oxide`, MIT).

**Observed:**
- The `.xpt` is a compound file with `Contents` (the protocol: `Gen5ExperimentID`, `CExpTemplateDoc`, procedure objects, plate layout) and per plate `SUBSETS/<n>/HEADER` and `SUBSETS/<n>/DATA`.
- `HEADER`: an MFC archive with `CPlateInfo`, u16, the plate name (`Plate 1`), 6 bytes, `0A 00 00 80`, and a u32 Unix time. That time, converted to the reader PC's time zone (UTC+2 for all three exports), is the export's `Date`/`Time` (2022-05-05 17:53:00, 2024-06-19 15:55:57, 2024-07-11 16:00:05).
- `DATA` starts with a `CAssayDoc` class; Gen5 3.x (document versions 9 and 11) follows it with u64 size, u32 size, u32 packed size and a zlib stream holding the rest of the archive; Gen5 2.x (version 6) continues the archive as stored.
- Each read is a `CPlateDataSet`: u16 8 (3.x) or 6 (2.x), u16 3 or 2, u16 1, the read's name as a `CString` (`450`, `580`, `Lum`, `485,530`, `552,578` — the labels Gen5's exports print after each matrix row) and an OLE date (local time; the export's time or up to 8 s before it). A later read of the same class is introduced by the tag `0x8001` (3.x) or `0x8003` (2.x).
- Before its values, a record block: u32 24, u16 7, u32 **reads** (1 endpoint; 60 in the kinetic file), u32 1, u32 **rows**, u32 **columns**, u32 1 × 3. Then reads × rows × columns records of 24 bytes, read-major, wells row-major: f64 value, then flag bytes: byte 0 = 0 for a measured value; `01 00 00 03` for wells outside the read region (the 64 wells of the A5..H8 file, 66 of the kinetic file's 96); the other 12–15 bytes vary between files (uninitialised memory in part; not interpreted).
- After a kinetic block: u16 0, u16 1 or 2, u32 reads, (layout 2: one more u32), then per read u32 0, **u32 time in ms**, u64 0 — 15,000, 75,000, … = the export's `0:00:15`, `0:01:15`, ….
- `CTemperatureDataSet` (`T° 580`): record block `18 00 00 00 02 00`, u32 1, u32 1, f64 temperature (23.9 = the export's `Actual Temperature`).
- `CPlateDescr`: 5 bytes, then `CString`s: reader (`Synergy HT`, `Cytation5`, `Synergy H1`), serial number (`269171`, `22101737`; `Unknown` in one plate of the 2.x file), a code not interpreted, reader firmware (`2.25`), Gen5 version (`3.04.17`) — the export's `Reader Type`, `Reader Serial Number`, `Software Version`.
- In the 2.x endpoint file, plate 1's first record block has no `CPlateDataSet` header before it (its values are −99999 with flag byte 0x64); it is listed as not decoded.
- Gen5's Excel export puts the Results matrices one column right of the text export (row letters in column B, column numbers from C, the read label after the last column); the `Time` header cell is a time of day and `Date` a date at midnight.

**Validation:** `tests/plate_binary.rs`: every value of the three Gen5 Excel exports (384 values, 3 decimals) and all 1,800 values of the kinetic sheet (integers, with their 60 read times) equal the decoded values; reader, serial number, software version and temperature equal the export's header; the reads' time is within a minute of the export's time, which equals the plate's UTC time plus whole hours. The ground truth is read from the exports with openpyxl by `oracle/plate_exports.py`, an independent reading.

**Decided (right or refuse):** decoded are record blocks with the validated head (…, 1, rows, columns, 1, 1, 1) behind a `CPlateDataSet` header; kinetic blocks only with a time table of the observed layouts and strictly increasing times. The detection mode comes from the read name by the rules of the header-less text exports (a number: absorbance at that wavelength; `ex,em`: fluorescence; `Lum`: luminescence; anything else: `unknown_read_mode`). Records with other flag bytes are reported as NaN with `flagged (a/b)` (Gen5 prints `OVRFLW` for saturated wells; no corpus file has one in a decoded block). Blocks without a header, unreadable streams and protocol files (`.prt`) are refused or listed (`read_not_decoded`), never guessed.

**Also changed:** the Gen5 export reader accepts matrices whose row letters sit in the second column (the Excel export; the three exports above now give 192, 96 and 96 values instead of none); spreadsheet time-of-day cells read as clock times, and a date cell at midnight combines with a separate time cell.

## 2026-09-26 — BMG MARS table view and spectral scans; EnVision read modes, plate placement and untitled matrices; how read modes are established (held-out findings C-H1 and C-M1, reproduced on development files) (Richard Zimring with Claude as assistant)

**Why:** the third held-out draw found a BMG CLARIOstar spectral-scan export returning no values (C-H1) and an EnVision export whose read mode was left `unknown` (C-M1). Neither held-out file was opened: both problems were reproduced on new public files, found by GitHub code search for the export's own markers (`Fluorescence (FI) spectrum`, `Raw Data (Abs Spectrum)`, `Results for US LUM`, `Plate,Repeat,Barcode`), from repositories that no held-out or development file uses.

**Files** (all added as development inputs; ground truth from the independent text readers in `oracle/plate.py`, since allotropy 0.1.146 rejects every one of them: `pd.read_csv` tokenizing errors for the BMG files, `Plate type` missing or the `.txt` extension for the EnVision ones):

| corpus id | source | licence | what reproduces |
| --- | --- | --- | --- |
| `bmg-table-tjlane-absspectrum-semicolon` | GitHub tjlane/mmcpd-p19638 | MIT | CLARIOstar absorbance spectrum, semicolon-delimited MARS table view; before: detected, 0 values, `check` exit 4 `no_plate_data` (C-H1) |
| `bmg-table-kelp-pigments-absspectrum` | GitHub lukaseamus/kelps-vs-urchins | GPL-3.0 | the same view, comma-delimited, header pairs one per line; before: not even detected (exit 3) |
| `bmg-table-wehi-exscan-averaged`, `bmg-table-wehi-emscan-averaged` | GitHub WEHIGenomicsRnD/Manuscript_FRP_MLDE | MIT | fluorescence excitation and emission scans (C-H1's kind of read), values averaged over replicates by MARS; before: 0 values |
| `bmg-table-wehi-multichromatic` | same repository | MIT | table view of an endpoint read (two filter pairs); before: 0 values |
| `envision-text-gdr-lum384-untitled` | GitHub gdrplatform/gDRworkshops | Artistic-2.0 | EnVision 1.13, no `Labels:` section, no results title: before 0 values and no mode (C-M1) |
| `envision-text-dse-ctg-lum384-semicolon`, `envision-text-dse-lum1536-tab` | GitHub lujunyan1118/DrugScreenExplorer | GPL-3.0 | plate information after the data, `Label name;;;;…` labels, a bare matrix first; before: read by the generic dialect (mode guessed from a title) |
| `envision-text-screenwerk-lum1536` | GitHub Enserink-lab/screenwerk | GPL-3.0 | EnVision 1.14 1536-well luminescence with labels (already read; a new depositor) |

A LabKey test file (`Label name,,,,Renilla US LUM 96 (cps)`, assay information before the plates) showed the second `Labels:` form too; the repository states no licence, so it is not in the corpus; a unit test covers that form.

**Prior art consulted:** none beyond the files. allotropy (MIT) was run as a black box and fails on all of these.

**Observed (BMG):**
- MARS writes a "table view" as well as the plate view: a line `Well` + `Content` (+ column titles), one line per well (`A01`, sample name, values). Every column carries its title; columns with the same title are one read.
- `Raw Data (Abs Spectrum)`, `Raw Data (Ex Spectrum)`, `Raw Data (Em Spectrum)`: spectral scans. The next line holds `Wavelength [nm]` under `Content` and the wavelength of every column (260, 261, … or 450, 452, …). The mode line reads `Absorbance spectrum` or `Fluorescence (FI) spectrum`.
- `Average over replicates based on Raw Data (…)` and `Blank corrected based on Raw Data (…)` title columns MARS computed (the WEHI exports have only averaged columns; identical values in replicate wells confirm it).
- Header pairs can stand one per line (`User: USER`, `Path: …`, `Test ID: 4609`, `Test Name: …`) with a UTF-8 BOM; the mode line can carry a remark (`Absorbance values are displayed as OD`).
- Cells `overflow` in absorbance spectra (kept as non-numeric).

**Observed (EnVision):**
- `Auto export parameters:` state `Place plate information at … Beginning of plate` or `End of plate`; in the latter each plate's `Plate information` follows its data, and a file can start with `Calculated results: …` or a bare matrix.
- Export format `Plate` (or `Plate2`/`Plate4`) matrices can have no title and no row letters/column numbers; the gDR file writes each line with an empty first field and keeps empty lines for rows and empty fields for columns that were not read (16 lines × 24 fields for a 384-well plate of which 12 × 21 wells hold values).
- The plate information's `Label` and `Measinfo` columns and the background information's `Label`/`MeasInfo` name each read and its detector: `De=USLum Ex=N/A Em=N/A` (every luminescence file), `De=1st Ex=Btm Em=N/A` (the development absorbance file), `De=1st|2nd Ex=Top Em=Top` (the HTRF file).
- `Labels:` entries are `<label>,,,,<label id>` or (EnVision 1.12/1.13) `Label name,,,,<label>` with indented sub-sections.

**Decided:**
- BMG table view read as described in `docs/formats/plate-readers.md` (§ BMG, table view). A second header line other than `Wavelength [nm]` is refused with a warning (no development file has one). Excitation-scan wavelengths are the rows' `wavelength_nm`, named by `settings.scanned_wavelength`.
- EnVision read modes: detector code first, then label keywords, else `unknown` — the old default of fluorescence for any label without `ABS`/`LUM`/`Alpha` was a guess (labels are editable names) and is removed. Every measured read now says how its mode was established (`mode_basis`), in every dialect; derived modes are reported in `assurance.inferred` (docs/assurance.md).
- Untitled matrices are read only when their size is a standard plate.

**Validation:** all nine files agree with the independent readers of `oracle/plate.py` (values per mode as (row, col, value) hashes, scan wavelengths, calculated values separately for the WEHI scans); every earlier plate development file still passes.

## 2026-09-26 — SoftMax Pro text exports: the save time is not the read time (finding PLATE-1) (Richard Zimring with Claude as assistant)

**Corpus files:** `softmax-abs-endpoint-plates`, `softmax-fl-kinetic-plates`, `softmax-lum-endpoint-utf16`, `softmax-spectramax340-kinetic-partial` (SoftMax Pro text exports; no held-out file). **Prior art consulted:** none new; allotropy (MIT) run as the second opinion.
**What was inferred from what.** The only time in these exports is the footer's `Original Filename: …; Date Last Saved: <date> <time>`. The name says what it is: when the document was saved, which follows the read (in the two kinetic files by at least the run length, which allotropy adds to it). No block, header or note in the four files states when the plate was read. Decided: the footer time is reported as `tables[].extra.saved_at` and `experiment.acquisition.saved_at` (a new optional field: the time the file was last saved or exported), and no longer as `extra.acquired_at` / `experiment.acquisition.started_at`. The ASM export writes no `measurement time` for them. SoftMax Pro documents (`.sda`, `.pda`), which store each plate's read time, are unchanged.

## 2026-10-06 — BMG MARS table view of a kinetic read (held-out finding D-H3, reproduced on development files) (Richard Zimring with Claude as assistant)

Held-out draw D reported a CLARIOstar kinetic table-view export that returned no values (`table_axis_not_decoded`) while its assurance was `validated`. Two development files from repositories no held-out file uses show the same layout and the same result before this change:

| id | source | licence | what it is |
| --- | --- | --- | --- |
| `bmg-table-bostock-calcein-kinetic` | GitHub jonathanbostock/phd-thesis @4ad6e84 | MIT | CLARIOstar, fluorescence 483-14/530-30, 61 time points of 36 wells, CRLF, 2024 |
| `bmg-table-rpazuki-od600-kinetic` | GitHub rpazuki/lab_utils @a93c502 | GPL-3.0 (data) | MARS wizard `Onyx Fast Growth OD`, absorbance 600 nm, 109 time points of 6 wells, an empty line after every line, LF, 2025 |

**Prior art consulted:** none. No held-out file was opened.

**Observed:**
- The title line is `Well Row,Well Col,Content,` and then `Raw Data (<filters>)` once per column (with a leading space in the CLARIOstar file). The line after it has `Time` under `Content` and each column's time as MARS prints it: `0 min `, `1 min `, … in one file, and `0 h `, `0 h 15 min`, …, `0 h 60 min`, `1 h 15 min`, …, `27 h` in the other. `0 h 60 min` shows that MARS rounds each time to the minute when it prints it (59.5 min or more prints as 60), so the printed times are exact only to ±30 s.
- Every column under one title is one read, and the columns are its time points. The mode line (`Fluorescence (FI)`, `Absorbance`) has no `spectrum`.
- In the second file every line is followed by an empty line, including the title line and the time line.

**Decided:**
- A table view whose second title line is labelled `Time` (or `Time [s]`, `Time [min]`, `Time [h]`) is a kinetic read. Each column's time is its label: `<n> h`, `<n> min` and `<n> s` parts are added up; a bare number takes the unit in the label's brackets, and a bare number under a bare `Time` label is refused (its unit is not stated). The value's `time_s` is that time, and the read type is `kinetic`. If any column's label does not parse, the table is refused as before (`table_axis_not_decoded`).
- Empty lines between the title line, the axis line and the well lines are skipped.

**Validation:** `oracle/plate.py` (`bmg_csv_summary`, the project's independent text reader) now also reads `Well Row`/`Well Col` tables and the time line. Every value of both files agrees with it, with its time.

## 2026-10-06 — Tecan i-control: several reads per well, a German export, one export per sheet (Richard Zimring with Claude as assistant)

**Why:** a survey of public Tecan files found three i-control layouts that `info` read as an empty export (`check` exit 4 `no_plate_data`) while the assurance said `validated`, and one export that was not recognised at all (exit 3).

**Corpus files** (no held-out record; neither repository is used by a held-out entry):
- `tecan-icontrol-multiread-kinetic-tread`, `tecan-icontrol-multiread-endpoint-tread`, `tecan-icontrol-multiread-segments-tread`: `inst/extdata/` of github.com/gl-eb/tread @a73d60b (MIT, `LICENSE.md`), i-control 2.0.10.0 XLSX exports of an infinite 200Pro. The date, time and serial number look edited by the depositor (2022-01-01, 1234567890); the layout is i-control's.
- `tecan-icontrol-de-multiread-kinetic-sgt`: `example_data/tecan_infinite_test_data.xlsx` of github.com/DataSpott/sgt_analysis @134c3c5 (GPL-3.0; a data file, used as test input only), i-control 1.12.4.0 with German labels, 2019.

**Prior art consulted:** tread's R sources (`R/tparse.R`, `R/time_series_multiple_reads.R`, `R/single_time_multiple_reads.R`, `R/tunite.R` at the same commit; MIT), read as documentation: tread recognises multiple reads by a settings line starting `Multiple Reads`, finds a time series of them by `Cycles / Well` lines and a single read by a `Well` header, and treats each sheet of a segmented run as its own export. SGT-Analyser's code (GPL-3.0) was not opened.

**What was inferred from what (hex-free: the workbooks were listed cell by cell with openpyxl):**
- With `Multiple Reads per Well (<pattern>)` = `3 x 3` (or `2 x 2`) in a `Label:` section, a kinetic read has no `Cycle Nr.` list. Instead each well has a block: a `Cycles / Well` line, then `<well>` followed by the cycle numbers, `Time [s]`, `Temp. [°C]`, `Mean`, `StDev`, and one line per read position named `x;y` (`1;2`, `0;1`, …: 5 lines for the `3 x 3` circle, 4 for the `2 x 2` square). In every block of the four files (151 blocks) `Mean` equals the arithmetic mean of the position lines at that cycle within 0.08 %; the position values are written with four decimals and `Mean` with more, so they cannot agree exactly. `StDev` is the spread of the position lines (within 5 % of their sample standard deviation); it is not used.
- An endpoint read with several reads per well is a list: a `Well` header with `Mean`, `StDev` and one column per position, then one line per well; the `Temperature: …` line sits above it in column B.
- `time_series_segments.xlsx` holds two complete exports (`Sheet2`, `Sheet3`), each with its own header, `Date:`/`Time:` (10:00 and 10:15) and `Start Time:`; tread stitches them by those start times.
- The German export uses the same layout with translated labels: `Programm:` (Application), `Gerät:` (Device), `Seriennummer:`, `Datum:`, `Zeit:`, `Anwender` (User), `Platte` (Plate), `Platten-ID (Stapler)`, `Kinetik - Messung`, `Kinetik - Zyklen`, `Intervallzeit`, `Modus` (`Absorption`), `Wellenlänge`, `Bandbreite`, `Anzahl der Blitze`, `Ruhezeit`, `Startzeit:`, `Endzeit:`, and in the data `Zyklen / Well`, `Zeit [s]`, `Mittelwert`. The date is `04.04.2019` (day first, as i-control's English exports with dotted dates).
- **Rule:** the value of a well at a cycle is its `Mean` (`Mittelwert`) line, the value i-control reports for the well; the position lines and `StDev` are not returned (a note says so). A block without a `Mean` line is not read and is reported. Each sheet that starts with an i-control header becomes its own plate read, with that sheet's date and time. German labels are read only where they were seen in this file.

## 2026-10-06 — SoftMax Pro 6/7 documents with several wavelengths (Richard Zimring with Claude as assistant)

**Why:** a survey of public SoftMax Pro documents (figshare `:extension: sda`) found 18 of 24 `.sda` files refused (exit 6). Twelve are dual-wavelength ELISA reads (a measurement wavelength and a correction wavelength, `450 570` or `405 580`), the most common absorbance read there is; the documents' text exports are deposited next to them.

**Corpus files** (figshare 30449435, "Data sets for conditioned media and flow cytometry for menMSC comparison with BMSCs", Panek and Colbath, CC-BY-4.0; no held-out record): `softmax7-elisa2wl-ha-0513` (`5.13.22_HA_D0+D24.sda`, one plate, 450/570 nm) with its export `softmax7-elisa2wl-ha-0513-xlsx`; `softmax7-elisa2wl-ha-0630` (`6.30.22_HA_D24+D48-dilutions2.sda`, one plate whose export covers columns 1–10) with `softmax7-elisa2wl-ha-0630-xlsx`. Each export is an `.xlsx` whose first sheet holds SoftMax Pro's text export (`##BLOCKS=`, `Plate:` lines, PlateFormat matrices) and further sheets of the depositors' own calculations.

**Prior art consulted:** none.

**What was inferred from what** (the document tree listed with the existing parser, compared with the exports cell by cell):
- `WavelengthSettings/WavelengthList` holds `Wavelength0` and `Wavelength1` (450 and 570; 405 and 580). The export's `Plate:` line says `2` wavelengths, `450 570`.
- The plate's data object is: plate name, i32 0, i32 1, **i32 2** (the number of wavelengths; 1 in every single-wavelength document of the corpus), i32 columns, i32 rows, two f64, then an object with one entry per read (`0`; endpoint), whose second item is an object with **one entry per wavelength** (`0`, `1`, in `WavelengthList` order), each `i32 index`, `f32 temperature`, `f64`, a byte array of rows × columns f64 values, then flags. The earlier entry (2026-09-25) read these two levels the other way round (a list of wavelengths, each a list of reads); with one wavelength and one read both readings give the same values, so nothing decoded before changes.
- In both documents the first wavelength's array holds the export's first matrix (450 nm) and the second wavelength's array the second (570 nm), value for value (352 values; the 0630 export covers columns 1–10, and the document stores not-a-number in columns 11–12). The second byte array of each wavelength entry is all zeros (not read, as before).
- The record's PGE2 documents (405/580 nm, such as `7.8.22_PGE2_ConditionedMedia_undiluted.sda` with four plates) decode the same way, but their exports hold blank-subtracted values (0.0198 exported where the document stores 0.0983), so they are not ground truth and were not added.
- Not decoded: `21500211`'s documents ("Cell viability", figshare, CC-BY-4.0) use another settings class (`Excitation`, `Emission`, `NrOfWavelengthPairs` instead of a `WavelengthList`) and have no export; they stay refused.

**Rule:** a plate section is decoded when its `WavelengthList` has n ≥ 1 entries, its data object says n wavelengths, and its one read holds n wavelength entries indexed 0 … n−1, each with a rows × columns value array; each wavelength becomes a read (channel) of the table. Kinetic and spectrum sections, several reads, and settings without a `WavelengthList` stay refused.

## 2026-10-06 — Gen5 1.x experiment files (Richard Zimring with Claude as assistant)

**Why:** the survey above found seven Gen5 `.xpt` files of figshare 33174608 ("Human Preclinical Blood-Brain Barrier Model Reveals Surface Chemistry-Dependent Permeability…", CC-BY-4.0, 2026; no held-out record) refused with exit 6 ("no read (CPlateDataSet) found"). They were written by Gen5 1.11 (`CPlateDescr`: Synergy H1, Gen5 `1.11.4`), older than any `.xpt` in the corpus (2.08 to 3.15).

**Corpus files:** `gen5xpt-gen5v1-synergyh1-dextran`, `gen5xpt-gen5v1-synergyh1-cdneg`, `gen5xpt-gen5v1-synergyh1-cdpos` (`2025-07+15 Dextran 4-37degres.xpt`, `2025-07+15 D-CD-  4-37degres.xpt`, `2025-07+15 D-CD+  4-37degres.xpt`), and the depositors' workbook `gen5xpt-gen5v1-fluo-results-xls` (`Fluo Results for 4 and 37 degres brut 2025-07-15.xls`), whose sheets `Dex`, `CD-` and `CD+` hold Gen5 result matrices (row letters in column A, column numbers 1–12, the read label `490,525` or `399,505` after the last column) copied by the depositors.

**Prior art consulted:** none. The streams were listed with olefile (BSD-2-Clause) and the archives read as bytes.

**What was inferred from what:**
- `SUBSETS/<n>/DATA` starts with the `CAssayDoc` class as in Gen5 2.x and 3.x, document version 4, stored as is (not zlib-compressed).
- The data set header after `CPlateDataSet` is u16 **5**, u16 **0**, u16 1, the read's name as a `CString` (`490,525`, `399,505`), then two f64 that are 1.0 in all seven files of the record; there is no OLE date where Gen5 2.x and 3.x write one. (Further on, `0A 00 00 80` and a u32 Unix time hold what looks like the read time, 7 to 28 s after the plate time in `HEADER`; it is not read.)
- The record block is the one Gen5 2.x and 3.x write: `18 00 00 00 07 00`, u32 reads (1), u32 1, u32 rows (8), u32 columns (12), 3 × u32 1, then 96 records of 24 bytes (f64 value, flag byte 0; one file of the record not added here, `Dextran 4 et 37°.xpt`, has flag `64` with −99999, as Gen5 2.x writes for wells it did not measure). In the three files every value in the depositors' sheets equals the decoded value of the same well, 159 values (the `Dex` sheet's second matrix is the Dextran file; the depositors kept columns 1–9 and a few further wells).
- The other four `.xpt` files of the record decode the same way; their values are not in the depositors' sheet, so they were not added.
- `CPlateDescr` starts with u16 6 and four zero bytes (Gen5 2.x and 3.x files: u16 7 and three zero bytes), then the same strings: reader `Synergy H1`, serial number `258503`, a code `8040200`, firmware `1.03.0`, Gen5 version `1.11.4`. Read with the 2.x offsets, the strings shift by one (an empty model, the reader name as serial number, the firmware as Gen5 version), so the schema now picks the offset and another schema is not read.
- **Rule:** a data set header with u16 5, u16 0, u16 1 and a name is a Gen5 1.x read without a date; its record block is decoded by the existing grammar. Only the class declaration's header is taken for this version (later reads that refer to the class by a tag carry a date in 2.x and 3.x, and no Gen5 1.x file with several reads was seen), and a record block that no header announces is still reported (`read_not_decoded`).

## 2026-10-07 — Gen5 Excel kinetic tables, and workbooks with several Gen5 exports (Richard Zimring with Claude as assistant)

**Why:** the signals bug hunt (2026-10) found that `zenodo4449746-gen5-synergy-htx` yields no values: `info` calls it `unvalidated` and `check` exits 4 (`no_plate_data`). The reader also read only the first worksheet of a workbook and said nothing about the others, and this workbook holds seven Gen5 exports, one per sheet.

**Corpus files:** `zenodo4449746-gen5-synergy-htx` (Zenodo 4449746, "Phages weaponize their bacteria with biosynthetic gene clusters", Dragos, Andersen et al., CC-BY-4.0; no held-out record). The development workbooks with several sheets were run before and after the change: `bmg-smart-control-fi`, `skanit-elisa-steps`, `tecan-icontrol-kinetic-xlsx`, `tecan-icontrol-multiread-kinetic-tread`, `tecan-icontrol-multiread-endpoint-tread`, `tecan-icontrol-multiread-segments-tread`, `tecan-icontrol-de-multiread-kinetic-sgt`, `zenodo7922369-spark-lipid-mixing`, `zenodo21627132-clariostar-cou3-050`, `zenodo21627132-clariostar-cou3-200`.

**Prior art consulted:** none beyond the 2026-09-22 entry (allotropy's Gen5 text parser, MIT, for the `Time` / `T° <label>` table shape of text exports). allotropy 0.1.147 does not read Gen5 Excel exports (its Gen5 parser takes `.txt` only).

**What was inferred from what** (every sheet listed cell by cell with openpyxl, MIT):
- Each of the seven sheets (`experiment 1` … `experiment 4_transfer3`) starts with Gen5's own header (`Software Version` 3.03.14 in A2, `Experiment File Path:` naming a different `.xpt` per sheet, `Plate Number`, `Date`, `Time`, `Reader Type:` Synergy HTX), then `Procedure Details` with `Start Kinetic` (24 h, every 10 min, 145 reads) around three unnamed `Read` steps: `Absorbance Endpoint` at 600 nm, `Fluorescence Endpoint` 485/20 → 528/20, and `Fluorescence Endpoint` 590/20 → 635/32.
- Each read is a kinetic table titled in column A (`Read 1:600`, `Read 2:485/20,528/20`, `Read 3:590/20,635/32`). The table starts in column B, one column to the right of a text export, as Gen5's Excel matrices do: `Time` in B, `T° <title>` in C, then the wells `A1` … `H12`. Each row's time is an Excel time value; from 24 h on it is stored as a number of days above 1 (`1.005…` for 24:07:33), and an unfinished run ends with `0:00:00` rows without values.
- The depositors added their own cells to the sheets: rows between the title and the table (`replicate 1`, `comp. A`, `(GFP)`, `AVERAGE`), columns after `H12` (`comp. F`, averages, `#DIV/0!`), and cells in the blank rows that separate the tables. So a table ends at the next title in column A or the next `Time` header, not only at a blank row, and the title of a table may be overwritten by a depositor's line. Gen5's `T° <title>` header repeats the title, so it names the read when the line above does not.
- Two reads share a mode (fluorescence) without names. The filter set of the label (`485/20,528/20`) matches one `Read` step's `Excitation`/`Emission`, which picks the step.
- **Rule:** in a Gen5 export, a `Time` or `Wavelength` header in column A (text) or column B (Excel, column A empty) starts a table; its rows run until a blank row, a cell left of the time column, or a time cell that is not a time; an Excel time value is a fraction of a day (`× 86,400` s). The read is named by the title line above, or else by the `T° <title>` header. Every worksheet whose first lines are a Gen5 export header is read, each plate becoming one table named after its sheet.
- **Worksheets not read:** the other dialects read the sheets they recognise. A non-blank worksheet that no dialect read is reported by `check` (`worksheet_not_read`) and in `info`'s notes. When that sheet on its own is recognised as an export, its data are left out with `undecoded` and no scope (the file is `partially_validated`, as for a refused read); a sheet that holds no export (the depositors' own calculations, as in `gen5xpt-cytation5-elisa-ifng-xlsx`'s `Calcul`) is only reported.

## 2026-10-08 — BMG SMART Control kinetic workbooks: one plate matrix per cycle (Richard Zimring with Claude as assistant)

**Why:** `check` on the two CLARIOstar workbooks of Zenodo 21627132 reported each kinetic read as one time point with 2,400 values for 24 wells (`duplicate_value`), and `info` called the read `endpoint`. The run has 100 cycles (97 in the second file), and the reader gave every cycle's matrix the same time.

**Corpus files:** `zenodo21627132-clariostar-cou3-050`, `zenodo21627132-clariostar-cou3-200` (development inputs since 2026-10-06). `bmg-smart-control-fi` (an endpoint workbook) was run before and after the change.

**Prior art consulted:** none. The workbooks were listed cell by cell with openpyxl (MIT).

**What was inferred from what:**
- The `Microplate Cycle 1 (0 min  -Inj` sheet holds one block per cycle. Each block starts with a line `Cycle <n> (<time>)` in column A (`Cycle 1 (0 min  - Inj. only)`, `Cycle 2 (0 min 38 s)`, …, `Cycle 100 (62 min 42 s)`), followed by the numbered matrices of that cycle: `1. Raw Data (402-8/474-9)`, `2. Layout`, `3. Volume 1`, `4. Temperature`, `5. Blank corrected based on Raw Data (…)`, `6. Average over replicates based on Blank corrected (…)`. The cycle line is the only place the sheet gives a matrix its time.
- The time is written as MARS writes kinetic table columns (`<n> h`, `<n> min`, `<n> s` parts), and the first cycle adds a remark after ` - ` (`Inj. only`: the cycle in which the injector ran, so its wells read `Inj.` instead of a value). The times step by 38 s, the `Cycle time [s]` of the `Protocol Information` sheet, whose `No. of cycles` (100 and 97) equals the number of cycle lines.
- The workbook's own `Table All Cycles` sheet lists the same run as one line per cycle and section, with the cycle's time in column B (`0 min `, `0 min 38 s`, …). Read with openpyxl, its raw-data lines equal the matrices of the cycle sheet value for value (2,400 values in the first file and 2,328 in the second, none different), and the time of the k-th line equals the time of the k-th cycle line in both files.
- **Rule:** in a SMART Control plate sheet, a line whose first cell is `Cycle <n> (<time>)` gives its time to the matrices after it, up to the next cycle line. The time is the part before ` - `, read like a kinetic table column. With more than one cycle the read is `kinetic`. A cycle line whose time does not read is reported (`cycle_time_not_read`) and its matrices keep no time.
