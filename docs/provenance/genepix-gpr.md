# GenePix Results `.gpr` provenance

## 2026-09-26 — before the reader was written (Richard Zimring with Claude as assistant)

**Corpus files used** (all in `corpus/manifest.toml`):

- Zenodo 22128078 (Roberts, Zanetti-Domingues, Martin-Fernandez; CC-BY-SA-4.0): `gpr-zenodo22128078-s1`
  … `-s4` — GenePix Pro 3.0.6 results (`Type=GenePix Results 1.4`), a GenePix 4000 scan at 532 nm,
  1,488 features of a phospho-protein array, 43 columns.
- Zenodo 21015949 (Meyerhoff, Chen, O'Brien, Anadon; CC-BY-4.0): `gpr-zenodo21015949-ab4` — GenePix
  Pro 7.3 results (`Type=GenePix Results 3`), two wavelengths (635/532 nm), 52,736 features of a
  HuProt array.
- Two depositors only: no file is held out.

**Prior art consulted:** none. The file is an Axon Text File (ATF 1.0), which OpenReadout's ATF
reader already parses for Clampfit exports (`docs/formats/abf.md`); the GenePix header records and
column titles name themselves.

### Inferred from the files (scratch dumps, not committed)

- Line 1 `ATF\t1.0`; line 2 the number of header records and of columns (22 43, 32 56); then that
  many quoted `Key=value` records, tab-separated values for per-channel records (`Wavelengths`,
  `PMTGain`, `ScanPower`, `LaserPower`; in GenePix Pro 3 `ImageName` holds the wavelengths and
  `PMTVolts` the PMT voltages, with an empty first entry for the unused channel); then one line of
  quoted column titles and one tab-separated line per feature, as many cells as columns.
- `Type=GenePix Results 1.4` / `3` identifies results files (array lists say
  `GenePix ArrayList`); `Creator=GenePix Pro 3.0.6.53` / `7.3.0.0`; `Scanner=GenePix 4000 [140014]`
  (model, serial in brackets); `DateTime=2022/05/17 17:07:54` (local time).
- Cells: numbers, quoted text (`Name`, `ID`), and `Error` where GenePix could not compute a ratio
  (every ratio column of the single-wavelength GenePix Pro 3 files).
- Positions: in the GenePix Pro 7 file `X`, `Y` (250-21,320 and 1,250-60,430) fit inside
  `ScanRegion` 2,200 × 6,248 pixels at `PixelSize` 10 if they are micrometres; the GenePix Pro 3
  files have no `ScanRegion`, so the unit is not established for them and no unit is attached.
