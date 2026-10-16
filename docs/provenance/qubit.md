# Thermo Fisher Qubit exports provenance

## 2026-10-15 — before the reader was written (Richard Zimring with Claude as assistant)

**Corpus files used** (all in `corpus/manifest.toml`): Benchling-Open-Source/allotropy test data
(MIT), commit `bf24b8bd518d`, one depositor: `qubit4-dsdna-hs`, `qubit4-out-of-range` (Qubit 4),
`qubit-flex-rna-br`, `qubit-flex-sep-header` (Qubit Flex 1.5.0). No other public Qubit export with a
licence was found (GitHub, Zenodo, figshare, Dryad), so no file is held out.

**Prior art consulted:** allotropy (MIT, https://github.com/Benchling-Open-Source/allotropy), run as
the reference reader (`oracle/benchtop_oracle.py`).

### Inferred from the files

- Qubit 4: UTF-8 CSV; header `Run ID,Assay Name,Test Name,Test Date,Qubit® tube conc.,Units,
  Original sample conc.,Units,Sample Volume (µL),Dilution Factor,Std 1 RFU,Std 2 RFU,Std 3 RFU,
  Excitation,Emission,Green RFU,Far Red RFU`; each `Units` follows its concentration (`ng/mL` for the
  tube, `ng/µL` for the sample). The run id is a date (`2023-11-04`) and the test date writes it
  `04/11/2023`: day first in that file, so the run id decides the order. A tube outside the assay's
  range has `Out of range` in both concentrations.
- Qubit Flex: optional first line `sep=,`; header `Run ID,Test Date,Assay Name,Sample Name,Original
  Sample Conc.,Original sample conc. units,Qubit Tube Conc.,Qubit tube conc. units,Sample Volume
  (uL),Dilution Factor,Extended Low Range,Core Range,Extended High Range,Excitation,Std 1 RFU,Std 2
  RFU,Std 3 RFU,Sample RFU,Last Read Standards,Reagent Lot#,` with day-first dates
  (`13/12/2022 04:41:09 PM`). Rows hold seven more cells than the header names; allotropy's Excel
  versions of the same runs title them `Plate Barcode`, `Well`, `Sample ID`, `Tags`, `Calibrated
  Tubes`, `Software Version`, `Selected Samples` (the values fit: `6`, `1.5.0`, `6`).

### Validation (2026-10-15)

4 files, all pass against allotropy 0.1.147: 82 values (sample names, tube and original
concentrations, fluorescence). allotropy reads the Qubit 4 test dates month first, against the run
id; dates are not compared.
