# Agilent Bioanalyzer and TapeStation exports provenance

## 2026-10-15 — before the readers were written (Richard Zimring with Claude as assistant)

**Corpus files used** (all in `corpus/manifest.toml`, on the `hold` tier: they are the demo runs
that ship with Agilent's software, re-exported and published by the bioanalyzeR author in an MIT
repository, and whether Agilent's demo data may be redistributed is not settled):

- bioanalyzeR (Joseph W. Foley, MIT) `inst/extdata` at commit `39388173`:
  `agilent-bioanalyzer-dna1000` (`Demo DNA 1000 Series II.xml.gz`), `agilent-bioanalyzer-rna-nano`
  (`Demo Eukaryote Total RNA Nano Series II.xml.gz`), `agilent-bioanalyzer-hsdna`
  (`Demo High Sensitivity DNA.xml.gz`), `agilent-tapestation-d1000`
  (`D1000-Tubes-16-D1000.xml.gz` with its `_Electropherogram.csv.gz`),
  `agilent-tapestation-gdna` (`gDNA-Tubes-16-gDNA`), `agilent-tapestation-rna`
  (`Eukaryotic RNA-Tubes-16-RNA`), `agilent-tapestation-hsd1000`
  (`High Sensitivity D1000-Tubes-16-HSD1000`); each TapeStation XML with its
  `_Electropherogram.csv.gz` as a companion entry.

**Prior art consulted:** bioanalyzeR's `R/bioanalyzer.R` and `R/tapestation.R` (MIT), read as
documentation: which elements of the 2100 Expert XML export hold the samples, peaks, RIN and the
base64 signal; that the TapeStation XML holds the metadata and results and the
`_Electropherogram.csv` the signal, one column per lane; the convention that row `i` of `n` (from
0) lies at run distance `(n − i) / n`.

Native files (`.xad`, TapeStation `.D1000` and the like, Fragment Analyzer `.raw`) are not read:
there is no public description of them and no public file with a clear licence.

### Inferred from the files (scratch dumps, not committed)

Bioanalyzer XML export (root `Chipset`):

- `Chips/Chip/AssayHeader` names the assay (`Class` DNA or RNA, `Title` "DNA 1000 Series II");
  `ChipInformation` the creation date, software, computer, user and lots; `Instrument` the
  instrument; `AssayBody/DAAssaySetpoints/DAMAssayInfoMolecular` the size unit (`bp`, `nt`), the
  concentration unit (`ng/µl`, `pg/µl`) and the ladder's lengths and concentrations.
- `Files/File/Samples/Sample`: `WellNumber`, `Name`, `Category` (`Sample`, `Ladder`), `HasData`,
  `Comment`, `ResultFlagCommonLabel`; `DAResultStructures/DARIntegrator/Channel/PeaksMolecular`
  one `PeakMolecular` per peak (`MigrationTime`, `AlignedMigrationTime`, `FragmentSize`, `Height`,
  `Area`, `Concentration`, `Molarity`, `PercentTotal`, `Observations` such as `Lower Marker`,
  `StartTime`, `EndTime`); `DARRIN/Channel/RIN` for RNA; `DARConcentration/Channel/
  TotalConcentration`; `DARStandardCurve/Channel/LadderPeaks` the ladder's lengths against
  aligned migration time; `DASampleSetpoints/DAMSizing/Channel/StandardCurveFit` says
  `Point to Point`.
- `DASignals/DetectorChannels/Channel/SignalData` (`ChannelID` `RedFluorescence`): `XStart`,
  `XStep` (s), `XStartAligned`, `XStepAligned`, `NumberOfSamples`, `UnitX` `s`, `UnitY`
  `Fluorescence`, and `ProcessedSignal`, base64 of `NumberOfSamples` little-endian float32 values
  (`RawSignal` has more values, from an unstated start, and is left out).
- Sizing check: interpolating the stored standard curve point to point in aligned time
  (`XStartAligned + i · XStepAligned`) gives each sample peak's `FragmentSize` from its
  `AlignedMigrationTime`.

TapeStation export:

- XML (root `File`, UTF-8 with a byte-order mark): `FileInformation` (file name, assay, run end
  date), `ScreenTapes/ScreenTape` (tape id, run date, electrophoresis temperature and time,
  `Environment` with experimenter, instrument serial number and software versions),
  `Assay/Units`, and `Samples/Sample` (`WellNumber`, `Comment` the sample name, `Concentration`,
  `Observations` (`Ladder` marks the ladder), `RNA/RINe` and 28S/18S ratios, `DIN`, `ScreenTapeID`,
  `Peaks/Peak` with `Size`, `RunDistance` (%), `FromMW`, `ToMW`, `Height`, `Area`,
  `CalibratedQuantity`, `Molarity`, `Observations`, `Regions/Region` with `From`, `To`,
  `AverageSize`, `Concentration`, `Molarity`, `PercentOfTotal`).
- CSV `<name>_Electropherogram.csv`: a header of `Well: name` per lane in the XML's sample order,
  then one row per position along the lane (963 rows in these files), values without an axis.
  The ladder lane's maxima sit at rows `n − RunDistance · n / 100` (±1 row).
- Sizing check, by trying rules against the sample peaks' stored `Size`: aligning each lane to
  its ladder by the lower and upper markers (a linear map of run distance) and interpolating
  point to point gives D1000 sizes within 2.5 bp (mean 0.6 bp, the precision of
  `RunDistance`'s one decimal) with linear sizes and within 4.9 bp (mean 1.4 bp) with the
  logarithm of size. Assays with only a lower marker (genomic DNA, RNA) fit only when run
  distances are scaled so the marker falls on the ladder's (a shift is 8 % off on average) and
  the logarithm of size is interpolated: mean 0.5 %, at most 2 % (linear sizes: mean 3 %, at
  most 7 %). The reader follows both: with two markers a linear map and linear sizes (the
  logarithm put two High Sensitivity D1000 peaks 4 % off), with a lower marker only scaling and
  the logarithm of size.
