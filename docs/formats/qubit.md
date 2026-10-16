# Thermo Fisher Qubit exports

Qubit fluorometers measure DNA, RNA and protein concentration with dye assays. The Qubit 4 and Qubit Flex export their results as CSV. OpenReadout reads both as a table of samples with units. Derived from public export files with allotropy (MIT) as the reference reader. Provenance: `docs/provenance/qubit.md`. Crate: `openreadout-benchtop`.

| format id | files | reads | confidence |
| --- | --- | --- | --- |
| `qubit` | Qubit 4 and Qubit Flex exports (`.csv`) | one row per tube: run, assay, sample, time, tube and original sample concentrations with their units, sample volume, dilution, the standards' and the sample's fluorescence | see `cargo xtask assurance-audit` |

Not read: Excel saves of the exports. Concentrations are the Qubit's own, as stored; the standard curve is not refitted.

## Layouts

| layout | header | notes |
| --- | --- | --- |
| `qubit-4` | `Run ID,Assay Name,Test Name,Test Date,Qubit® tube conc.,Units,Original sample conc.,Units,Sample Volume (µL),Dilution Factor,Std 1 RFU,Std 2 RFU,Std 3 RFU,Excitation,Emission,Green RFU,Far Red RFU` | each `Units` column holds the unit of the concentration before it. The run id is the run's date (`2023-11-04`), which settles the day and month order of `Test Date` (`04/11/2023`) |
| `qubit-flex` | `Run ID,Test Date,Assay Name,Sample Name,Original Sample Conc.,Original sample conc. units,Qubit Tube Conc.,Qubit tube conc. units,...` | may start with a `sep=,` line. Qubit Flex 1.5 writes seven trailing columns without titles; the reader names them as the Flex's documented columns (`Plate Barcode`, `Well`, `Sample ID`, `Tags`, `Calibrated Tubes`, `Software Version`, `Selected Samples`) and says so in a note |

A concentration cell may hold text instead of a number (`Out of range`); it is kept in the `<name>_note` column next to the concentration.

## What the reader returns

- **Table 0 `samples`** (`extra.kind` `samples`), one row per tube read:

  | name | meaning | unit |
  | --- | --- | --- |
  | `run_id`, `assay`, `sample` | the run, the assay (`1x dsDNA High Sensitivity`, `RNA BR`), the sample (`Test Name`, `Sample Name`) | |
  | `measured_at` | the test date and time, ISO 8601 | |
  | `tube_concentration`, `sample_concentration` | the concentration in the assay tube and in the original sample | as the units columns state (`ng/mL`, `ng/µL`, `µg/mL`) |
  | `tube_concentration_note`, `sample_concentration_note` | text in place of a concentration (`Out of range`) | |
  | `sample_volume`, `dilution_factor` | sample volume and dilution | `µL` |
  | `std1_rfu`, `std2_rfu`, `std3_rfu`, `green_rfu`, `far_red_rfu`, `sample_rfu` | fluorescence of the standards and the sample | `RFU` |
  | `excitation`, `emission` | the light used | |
  | `extended_low_range`, `core_range`, `extended_high_range`, `standards_read_at`, `reagent_lot`, `plate_barcode`, `well`, `sample_id`, `tags`, `calibrated_tubes`, `software_version`, `selected_samples` | Qubit Flex columns | |

- **Experiment**: instrument Thermo Fisher Scientific Qubit 4 or Qubit Flex, software version (Flex); method name the assay, technique fluorescence detection; acquisition from the earliest to the latest test time; the dilution factor when every tube shares it.
- `preview` draws each sample's concentration as a bar, with the instrument's text where it wrote no number.
- `batch table` gives every row of every file.

## Validation

`cargo test -p openreadout-corpus-tests --features corpus` (`tests/corpus/benchtop_oracle/mod.rs`, `oracle/benchtop_oracle.py`): allotropy's ASM, per tube in file order: the sample name, the tube and original sample concentrations (unit-converted) and the fluorescence. allotropy reads `Test Date` month first, so where the run id says otherwise the two disagree on the date; dates are not compared.

## Vocabulary (public API of `openreadout-benchtop`, Qubit)

| identifier | meaning |
| --- | --- |
| `QubitReader` | reader of Qubit exports |
| `QUBIT_FORMAT_ID` | `qubit` |
| `SampleTableDataset` | the dataset the NanoDrop and Qubit readers return |
| `fuzz_qubit` | parse a Qubit export from bytes (fuzz targets only) |
