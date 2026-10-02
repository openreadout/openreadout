# GenePix Results `.gpr`

GenePix Pro writes a `.gpr` results file for each scanned microarray (DNA, protein or antibody arrays). OpenReadout returns the feature table with every column GenePix wrote, plus the scanner, software and scan settings. Derived from public files written by GenePix Pro 3.0.6 and 7.3, with pandas (BSD-3) as the reference reader of the table. Provenance: `docs/provenance/genepix-gpr.md`. Crate: `openreadout-biophys`.

| format id | files | reads | confidence |
| --- | --- | --- | --- |
| `genepix-gpr` | `.gpr` results of GenePix Pro (microarray scans: DNA, protein, antibody arrays) | the feature table (every column GenePix wrote), scanner, software, wavelengths, PMT gain or voltage, laser and scan power, date | medium (evidence rubric, `docs/assurance.md`) |

Not read by this reader: array lists (`.gal`, also ATF: exit 6 with a hint), settings (`.gps`), the
scan images (TIFF: the TIFF reader opens them). No normalization or background correction is
applied; values are as GenePix wrote them.

## Layout

An Axon Text File: `ATF 1.0`; the counts of header records and columns; the header records
(`"Key=value"`, per-channel records tab-separated); a line of column titles; one line per feature.
Header records used: `Type` (must start `GenePix Results`), `Creator` (software and version),
`Scanner` (model and `[serial]`), `DateTime` (local), `Wavelengths` or (GenePix Pro 3)
`ImageName`, `PMTGain` or `PMTVolts`, `LaserPower`, `ScanPower`, `PixelSize`, `Comment`; all
records are kept in the vendor tree. The ATF reader (`atf`) leaves files whose header says
`Type=GenePix` to this reader.

## What the reader returns

- **Table 0 `features`**: one row per feature, one column per title in file order (`Block`,
  `Column`, `Row`, `Name`, `ID`, `X`, `Y`, `Dia.`, `F635 Median`, …, `Flags`). A column whose cells
  are all numbers, or `Error` where GenePix could not compute a value, is numeric (`Error` → NaN);
  any other column is text (category codes, `extra.categories`). No unit is attached: the file does
  not state them. `extra.wavelengths_nm` lists the channels.
- **Experiment**: instrument vendor Molecular Devices (Axon GenePix), model and serial from
  `Scanner`, software and version from `Creator`; method parameters `wavelengths` and per
  wavelength `pmt_gain_<nm>`, `pmt_voltage_<nm>` (V), `laser_power_<nm>`, `scan_power_<nm>`,
  `pixel_size`; acquisition `started_at` (local time) and comment.
- `check`: rows with a different number of cells (left out, error finding).

## Validation

`cargo test -p openreadout-corpus-tests --features corpus` (`tests/gpr_oracle/mod.rs`,
`oracle/gpr_oracle.py`): pandas reads the table (skipping the ATF header by its declared record
count): the same row and column counts and titles, every numeric column's values at 64 rows, the
`Name` column at the same rows; the header's `Creator` and `Scanner` match the experiment model.

## Vocabulary (public API of `openreadout-biophys`, GenePix)

| identifier | meaning |
| --- | --- |
| `GprReader` | reader of GenePix Results `.gpr` files |
| `GPR_FORMAT_ID` | `genepix-gpr` |
| `GprDataset` | the dataset the reader returns |
