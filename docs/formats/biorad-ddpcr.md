# Bio-Rad droplet digital PCR (QX100, QX200, QX600)

Bio-Rad's droplet readers write a plate file per run: QuantaSoft writes `.qlp`, and QX Manager writes `.ddpcr`. Both programs export the results as CSV, and they can export each well's droplet amplitudes as a CSV of its own. OpenReadout reads QuantaSoft's `.qlp` (format `biorad-qlp`) and the CSV exports of both programs (format `biorad-ddpcr`), in crate `openreadout-benchtop`. Everything here was derived from public corpus files. The exports are checked against Python's `csv` module (`oracle/export_tables.py`), and the `.qlp` files against QuantaSoft's own export of the same plate (`oracle/qlp_oracle.py`). See `docs/provenance/biorad-ddpcr.md`.

## Plate files

- **QX Manager `.ddpcr`** is a 7-Zip archive with an encrypted header (`7z` signature `37 7A BC AF 27 1C`; listing it fails with "the archive header is encrypted", seen in all six files of Zenodo 19729930). Clean-room rule 4 forbids getting around that, so these files are not read. Export the results and amplitudes as CSV instead.
- **QuantaSoft `.qlp`** is not encrypted. It is read as format `biorad-qlp` (below).

## QuantaSoft `.qlp` (`biorad-qlp`)

Everything here was inferred from the nine plates of figshare 24278779 ... 24302350 and Zenodo 4957405 (QuantaSoft 1.7.4.0917), and checked against QuantaSoft's results export of each figshare plate.

The file looks like a little-endian TIFF with `IILQ` in place of `II*\0`: bytes 4–7 give the first IFD, each IFD is a 2-byte entry count, 12-byte entries (tag, type, count, value) and the offset of the next IFD. The types are TIFF's (2 text, 3 u16, 4 u32, 7 bytes, 11 float32). Unlike TIFF, the value field is an offset even for some entries of four bytes or fewer, so the reader reads a text entry inline only when its count is 2 to 4, and treats count 1 as empty text. The exception is the well name (tag 65019), which is always four inline bytes (`A01\0`).

| IFD | tags | meaning |
| --- | --- | --- |
| first | 305, 306, 315, 316 | software (`QuantaSoft 1.7.4.0917`), time written (`2023:06:20 12:27:54`), user, computer |
| first | 65004 | the channel names, comma-separated (`Ch1,Ch2`) |
| second | 65018 | the plate setup: `count` records of 1386 bytes at the offset. Each record starts with the offset of the next one (the reader checks this), then 2 bytes, the well (4 bytes), the sample (256), the experiment type (32, `Absolute Quantification`), the experiment (512, `ABS`), and per channel its kind (32: `Unknown`, `Negative Control`, `Not Used`, ...) and target (256). Text fields end at their first NUL |
| one per well | 65019, 65077 | the well and its supermix (`QX200 ddPCR EvaGreen Supermix`) |
| one per well | 65019, 65021, 65057, 65078 | a read well: 65021 is `count` droplet records of 28 bytes at the offset, 65057 one class byte per droplet, 65078 the droplet volume in nL (`0.85`) |

A droplet record is a u32 counter that increases from droplet to droplet, then per channel three float32: the amplitude, a width (the same value in both channels) and a third value (1.0 or 0.5) that is not read. A class byte holds QuantaSoft's cluster number in both halves: `0x00` rejected, `0x11` Ch1−Ch2−, `0x22` Ch1+Ch2−, `0x33` Ch1+Ch2+, `0x44` Ch1−Ch2+. Counted per well, these give exactly the export's accepted droplets, positives, negatives and the four Ch1±Ch2± counts, and the mean amplitude of each class equals the export's `MeanAmplitudeofPositives` and `MeanAmplitudeofNegatives`. An 8-byte flag record per droplet (tag 65054) is not read.

Where QuantaSoft could not classify a well (the export says `No Call`), the class bytes are not set: no droplet is rejected and every droplet has one class. The export then counts the droplets in another way. The reader leaves the counts of such a well empty (NaN) instead of reporting the unset classes.

Table 0 `wells` has one row per read well and used channel (a channel whose kind is not `Not Used`): `well`, `sample`, `experiment_type`, `experiment`, `channel`, `target_type`, `target`, `supermix` (text), `droplets` (stored), `accepted_droplets`, `positive_droplets`, `negative_droplets`, `droplets_ch1_pos_ch2_pos`, `droplets_ch1_pos_ch2_neg`, `droplets_ch1_neg_ch2_pos`, `droplets_ch1_neg_ch2_neg` (droplets) and `droplet_volume` (nL). Table 1 `droplets` has one row per droplet: `well`, `counter`, `ch1_amplitude`, `ch2_amplitude`, `width`, `cluster` (0 to 4). Concentrations are not stored in the file. They follow from the counts as −ln(negative ÷ accepted) ÷ droplet volume, but the reader does not compute them.

## Exports (`format_version` is the layout)

| layout | what it is | header |
| --- | --- | --- |
| `quantasoft-results` | QuantaSoft results, one row per well and target | `Well,ExptType,Experiment,Sample,TargetType,Target,Status,Concentration,...,Positives,Negatives,...,AcceptedDroplets,...`; older versions write `TypeAssay,Assay` for `TargetType,Target` |
| `qx-manager-results` | QX Manager results, one row per well and target | `Well,Sample description 1,...,Sample description 4,Target,Conc(copies/µL),...,Accepted Droplets,Positives,Negatives,...` |
| `quantasoft-amplitude` | one well's droplets | `Ch1 Amplitude,Ch2 Amplitude,Cluster` (or `Assay1 Amplitude,Assay2 Amplitude,Cluster`) |
| `qx-manager-amplitude` | one well's droplets | three `Target Value of ...` lines and a blank line, then `Ch1Amplitude,Ch2Amplitude,<target>,<target>,` |

QuantaSoft 1.7 writes about 35 more values per results row than its header names (they look like the 68 % confidence limits and ratios without titles). Those cells are left out and counted in a note. A results export's `Concentration` is a number or `No Call`. The number goes in `concentration`, in copies/µL, and the words go beside it in `concentration_note`.

### Results columns (table 0 `results`)

| our name | export header | unit |
| --- | --- | --- |
| `well`, `experiment_type`, `experiment`, `sample`, `sample_description_2..4`, `sample_type`, `target_type`, `target`, `status`, `status_reason`, `supermix`, `dye` | `Well`, `ExptType`, `Experiment`, `Sample` / `Sample description 1`, `Sample description 2..4`, `SampleType`, `TargetType` / `TypeAssay`, `Target` / `Assay`, `Status`, `Status Reason`, `Supermix`, `DyeName(s)` | text |
| `concentration` | `Concentration`, `Conc(copies/µL)` | copies/µL |
| `mass_concentration` | `pg/µL` | pg/µL |
| `copies_per_20ul_well` | `CopiesPer20uLWell`, `Copies/20µLWell` | copies |
| `total_ci95_max`, `total_ci95_min`, `poisson_ci95_max`, `poisson_ci95_min` | `TotalConfMax`, `TotalConfMin`, `PoissonConfMax`, `PoissonConfMin` | copies/µL |
| `total_ci68_max`, `total_ci68_min`, `poisson_ci68_max`, `poisson_ci68_min` | `TotalConfidenceMax68`, ... | copies/µL |
| `positive_droplets`, `negative_droplets`, `accepted_droplets` | `Positives`, `Negatives`, `AcceptedDroplets` / `Accepted Droplets` | droplets |
| `droplets_ch1_pos_ch2_pos`, `droplets_ch1_pos_ch2_neg`, `droplets_ch1_neg_ch2_pos`, `droplets_ch1_neg_ch2_neg` | `Ch1+Ch2+`, `Ch1+Ch2-`, `Ch1-Ch2+`, `Ch1-Ch2-` | droplets |
| `linked_molecules` | `Linkage`, `Copies/uL linked molecules` | copies/µL |
| `copy_number`, `reference_copies`, `unknown_copies`, `ratio`, `fractional_abundance` (%) | `CNV`, `ReferenceCopies`, `UnknownCopies`, `Ratio`, `FractionalAbundance` | |
| `mean_amplitude_positives`, `mean_amplitude_negatives`, `mean_amplitude`, `threshold` | `MeanAmplitudeOfPositives`, `MeanAmplitudeOfNegatives`, `MeanAmplitudeTotal`, `Threshold1` / `Threshold` | |
| `experiment_comments`, `merged_wells`, `reference_used`, `tilt_corrected` | `ExperimentComments`, `MergedWells`, `ReferenceUsed`, `TiltCorrected` | text |

Other headers keep a name made from the header (`Threshold2` → `threshold2`). Column labels are the export's headers, unchanged.

### Amplitude columns (table 0 `droplets`)

`ch1_amplitude`, `ch2_amplitude` (from `Ch1 Amplitude`, `Ch1Amplitude`, `Assay1 Amplitude`), `cluster` (QuantaSoft's cluster number), and for QX Manager one `target_<name>` column per target header: `1` positive, `0` negative, `u` unclassified (read as NaN). The well comes from the file name (`quant_A01_Amplitude.csv` → `A01`, `extra.well`).

## Not read

- Concentrations, confidence intervals and droplet counts are the software's own; nothing is recomputed from the amplitudes.
- The instrument model is not in the exports.

## Vocabulary (public API of `openreadout-benchtop`, ddPCR)

| identifier | meaning |
| --- | --- |
| `DdpcrReader` | reader of Bio-Rad ddPCR exports |
| `DDPCR_FORMAT_ID` | `biorad-ddpcr` |
| `SampleTableDataset` | the dataset the benchtop readers return |
| `fuzz_ddpcr` | parse a ddPCR export from bytes (fuzz targets only) |
| `QlpReader` | reader of QuantaSoft `.qlp` plates |
| `QLP_FORMAT_ID` | `biorad-qlp` |
| `QlpDataset` | the dataset the `.qlp` reader returns |
