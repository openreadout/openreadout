# Thermo Fisher NanoDrop exports provenance

## 2026-10-15 — before the reader was written (Richard Zimring with Claude as assistant)

**Corpus files used** (all in `corpus/manifest.toml`):

- Benchling-Open-Source/allotropy test data (MIT), commit `bf24b8bd518d`, one depositor:
  `nanodrop-8000-protein`, `nanodrop-8000-protein-plate`, `nanodrop-8000-dna`,
  `nanodrop-8000-dna-2`, `nanodrop-8000-custom-mrna` (NanoDrop 8000 reports),
  `nanodrop-eight-dsdna`, `nanodrop-eight-decimal-comma`, `nanodrop-eight-tab-aligned`,
  `nanodrop-eight-rna` (NanoDrop Eight exports), `nanodrop-one-dsdna` (NanoDrop One CSV report).
  The files look edited for testing (placeholder names and serials, one run saved with decimal
  commas and once with padded header lines).
- beabock/nanodRop (MIT), commit `d5088583`: `nanodrop-one-uvvis-nanodrop-r`, a NanoDrop One UV-Vis
  spectrum export of 24 measurements.
- Two depositors in all, and one per layout: no file is held out.

**Prior art consulted:**

- allotropy (MIT, https://github.com/Benchling-Open-Source/allotropy), run as the reference reader
  (`oracle/benchtop_oracle.py`); its parsers were not needed to understand the layouts, which name
  their columns.
- nanodRop (MIT, https://github.com/beabock/nanodRop): its DESCRIPTION, which says the files are UV-Vis
  spectral exports of the NanoDrop One with repeated measurements. Its code was not needed.

### Inferred from the files

- 8000 report: tab-separated, CRLF; one header row whose titles carry trailing spaces (`Well `,
  `Date `, `conc. `); `Date` (`4/14/2022`) and `Time` (`3:48 PM`) in two columns; `Units` after the
  concentration (`ng/ul`, `mg/ml`); the application decides the columns (nucleic acid: `A260`, `A280`,
  `260/280`, `260/230`, `Conc. Factor (ng/ul)`, `340 raw`, `NA Type` `DNA-50`; protein: `A280 10mm`,
  `E 1%`, `Ext. Coeff x10e3`, `Mol. Wt. kDa`; custom method: four `nm k`/`Abs k` pairs and a formula).
  The custom file writes `NaN` and `1.449E+0`; one protein file has empty plate and sample ids.
  The mRNA file shows a blank line and a second table (`Sample  ID`, `Curve Type`, `Ref  conc`,
  standards); kept as text.
- Eight export: `Application:`, `Serial Number:`, `User Name:` lines (`Key:<tab>value`, padded with
  tabs in one file), then a header row `Sample Id` (a UUID), `Sample Name`, `Date & Time`
  (`06/16/2022 04:38:28 PM(-07:00)`), `Location`, `ng/µL`, `A260/A280`, `A260/A230`, `A260`, `A280`,
  and 261 columns titled `220.0` to `350.0` every 0.5 nm whose cells are absorbances (four decimals):
  the spectrum. The decimal-comma file writes `220,0` and `0,0159`. The RNA file has
  `Serial number:` with an empty value, columns `Date`, `UID`, `Sample`, `Username`, results and
  unknown extra columns, day-first dates without seconds and no spectra.
- One CSV: comma-separated; `Date` (`7/19/2023 3:27:29 PM`), `Sample Name`, `Nucleic Acid(ng/uL)`,
  ratios, A260, A280, `Nucleic Acid Factor`, `Baseline Correction (nm)`, `Baseline Absorbance`, then
  corrected-concentration and impurity columns (some titles start with a space); rows from another
  run leave A260 and A280 empty.
- One spectrum export: blocks separated by blank lines; a sample line (`6 1`: the sample and the
  measurement), a date line (`5/14/2024 5:04 PM`), `Wavelength (nm)<tab>10mm Absorbance`,
  `//WLCalib: Shift <tab>0.22333<tab>` and five `//WLCalib: <nm>S` lines, `//QSpecEnd: `, then
  1,321 pairs from 190.0 to 850.0 nm every 0.5 nm. Every block shares the axis.

### Decisions

- Our column names (table in `docs/formats/nanodrop.md`); concentrations in one unit per column;
  absorbances unitless (the files give none; allotropy's `mAU` is its own choice).
- The Eight's `Sample Id` is a measurement UUID, not the sample: it becomes `sample_uid`.
- Ratios of the spectrum export are read off the spectrum and say so.

### Validation (2026-10-15)

15 files, all pass against allotropy 0.1.147 and pandas: 432 sample values (names,
concentrations, absorbances, ratios) and 39 spectra.

## 2026-10-19 — tab-separated One and comma-separated Eight reports, three more depositors (Richard Zimring with Claude as assistant)

**Corpus files:** `nanodrop-one-kelps-dna` (GitHub lukaseamus/kelps-vs-urchins, GPL-3.0),
`nanodrop-one-tab-freezerbox` (GitHub kalekundert/freezerbox, MIT). **Exposure:** a UTF-16,
tab-separated One report from GitHub KaiAragaki/mop was read while choosing files; that
repository is the source of a held-out file of another format, so it is not a corpus entry and
counts as no evidence. The tab-separated layout is pinned on the freezerbox file.

**Prior art consulted:** none beyond allotropy 0.1.147 (MIT), run as a black box for ground truth.

**Inferred from the files:** the One report is also saved tab-separated, with quoted titles and
year-first dates `"2019/08/06 12:31:14"` (freezerbox). The header row is the same as the
comma-separated report's. The reader now finds the separator from the header row (`Date`,
`Sample Name`, then a nucleic-acid or A260/A280 column), and reads a date whose first part has
four digits as year, month, day. allotropy reads the One report only comma-separated, so the
oracle rewrites tab-separated fields with Python's csv module before handing them over.

**Assurance:** the exports record no software version. Each layout comes from one instrument's
software, so the profile now names that software as the writer (`NanoDrop 8000 software`,
`NanoDrop Eight software`, `NanoDrop One software`), as the plate reader does for its export
kinds.

**Also (same day):** `nanodrop-eight-comma-czabriskie` (GitHub czabriskie/test-dag-ci, GPL-3.0), a
NanoDrop Eight dsDNA export saved comma-separated (`Application:,dsDNA`, `Serial number:,…`, then
`Date,UID,Sample,Username,ng/µL,A260/A280,…`) with dates written `08-Dec-2023 13:38:55`. The Eight
reader now takes the separator from the first line, and dates of the form day, English month
abbreviation, year. allotropy reads the Eight export only tab-separated, so the oracle rewrites the
fields tab-separated with Python's csv module first.
