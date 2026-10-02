# analyze

`analyze` runs analyses with documented methods: chromatographic peaks, chromatograms, NMR peaks, patch-clamp features, extracellular spikes, qPCR, plate assays and flow-cytometry gating.

```text
openreadout analyze <SUBCOMMAND> [OPTIONS] <FILE>...
```

| subcommand | what it returns | guide |
| --- | --- | --- |
| `peaks` | detected and integrated peaks; bands and regions of spectra | [Quantitation](../../guides/quantitation.md) |
| `chromatogram` | TIC, BPC, XIC, SRM/MRM and stored detector traces | [Quantitation](../../guides/quantitation.md) |
| `nmr-peaks` | NMR peak list and integrals | [NMR](../../guides/nmr.md) |
| `ephys-features` | action potentials, rheobase, f–I curve, passive properties | [Electrophysiology](../../guides/ephys.md) |
| `spikes` | extracellular spike counts, rates and times | [Electrophysiology](../../guides/ephys.md) |
| `qpcr` | Cq, Tm, ΔΔCq, standard curves | [qPCR formats](../../formats/qpcr.md) |
| `assay` | plate-reader wells, curves, dose-response, kinetics, growth, QC | [Plate analysis](../../guides/plate-analysis.md) |
| `gate` | FlowJo or Gating-ML gate hierarchy and population counts | [FlowJo workspaces](../../formats/flowjo-wsp.md) |

Every subcommand takes `--json`. The guides describe the methods and how they were validated. To run any of these over many files as one table, use [`batch`](batch.md).

## peaks and chromatogram

```text
openreadout analyze peaks [OPTIONS] <FILE>...
openreadout analyze chromatogram [OPTIONS] <FILE>...
```

Both take the flags in [Several inputs](index.md#several-inputs).

### Which signal (both)

- `--tic`: total-ion chromatogram. The default for mass-spectrometry files.
- `--bpc`: base-peak chromatogram.
- `--mz MZ`: extracted-ion chromatogram at this m/z. Repeatable or comma-separated.
- `--ppm PPM`: XIC half-width in ppm. Default 10.
- `--da DA`: XIC half-width in m/z units instead.
- `--transition Q1>Q3`: SRM/MRM transition. Repeatable.
- `--transition-tol DA`: Q1 and Q3 tolerance. Default 0.5.
- `--trace N`: a stored chromatogram or detector signal (UV/DAD, FID, TCD), by `traces[]` index. Repeatable. The default for files with traces and no spectra.
- `--channel C`: channel of `--trace`, such as one wavelength of a DAD.
- `--sweep N`: sweep of `--trace`. Default 0.
- `--run N`: spectra run index. Default 0.
- `--ms-level N`: MS level of the scans. Default 1.
- `--polarity positive|negative`: only scans of this polarity.
- `--scan-filter TEXT`: only scans whose filter contains this text.
- `--precursor MZ`: only MS/MS scans of this precursor (a product-ion XIC with `--mz`).
- `--precursor-tol DA`: precursor tolerance. Default 0.5.
- `--rt-range A-B`: retention-time range in minutes.
- `--mz-range A-B`: TIC or BPC over this m/z range only.
- `--profile`: use profile data where a scan stores both profile and centroids.
- `--aggregate sum|max`: how points inside an XIC window are combined. Default `sum`.

### chromatogram only

- `--max-points N`: at most this many points per chromatogram. Default: all.
- `-o`, `--output FILE`: write `.csv`, `.parquet`, `.arrow` or a `.png`/`.jpg` plot. One input only.
- `--width N`: plot width in pixels. Default 1200.
- `--overwrite`: replace an existing output file.

### peaks only

- `--smooth N`: Savitzky–Golay window in points. Default: half the typical peak width.
- `--min-snr S/N`: detection threshold as height over noise. Default 3.
- `--min-height H`: detection threshold in signal units.
- `--min-width MIN`: smallest width at half height, in minutes.
- `--min-points N`: fewest samples from peak start to end. Default 3.
- `--baseline auto|drop|valley|tangent|linear|none`: baseline under detected peaks (default `auto`), or under `--x-range`/`--integrate` windows (default `linear`).
- `--skim-ratio R`: with `--baseline tangent`, skim a peak when its height is below this fraction of its neighbour's. Default 0.1.
- `--noise SIGMA`: noise level in signal units, instead of the estimate.
- `--baseline-window MIN`: window of the running baseline that decides where peaks end.
- `--area-seconds`: report areas in signal × seconds instead of signal × minutes.
- `--rt MIN`: report the peak at this expected retention time.
- `--window MIN`: half-width of the `--rt` window, and of `--targets` compounds without their own. Default 0.5 min.
- `--pick largest|nearest`: which peak in the window to report. Default `largest`.
- `--integrate A-B`: integrate between two retention times with a straight baseline. Repeatable.
- `--x-range A:B`: integrate a window of the trace's own axis (cm⁻¹, nm, ppm; minutes on a chromatogram). Repeatable.
- `--targets FILE`: compound list (CSV, TSV or JSON) with `name` and `mz`, `q1`+`q3` or `trace`, and optionally `rt`, `window` and tolerances. One row per compound per file.
- `-o`, `--output FILE`: write the rows of every input to one `.csv`, `.tsv` or `.jsonl` file.
- `--plot FILE|DIR`: plot the chromatograms with the integrated peaks shaded.
- `--width N`: plot width in pixels. Default 1200.
- `--overwrite`: replace existing output files.

```bash
openreadout analyze chromatogram run.raw --mz 195.0877 --ppm 5 -o xic.csv
openreadout analyze peaks run.D --rt 3.1 --window 0.2 --plot peaks.png
openreadout analyze peaks spectrum.0 --x-range 980:1060 --x-range 1350:1525
```

## nmr-peaks

```text
openreadout analyze nmr-peaks [OPTIONS] <FILE>
```

The input is a Bruker experiment directory, a Varian `.fid` directory, a JEOL `.jdf` or a JCAMP-DX NMR spectrum.

- `--from auto|fid|processed`: use the vendor's processed spectrum, or process the FID here. `auto` prefers the stored spectrum.
- `--trace N`: trace index. Default: chosen by `--from`.
- `--sweep N`: row of a `ser` or arrayed FID to process. Default 0.
- `--phase MODE`: `default`, `stored`, `auto`, `magnitude`, `none`, or `P0,P1` in degrees.
- `--lb HZ`: exponential line broadening. Default: stored, else 0.3 Hz for 1H and 1 Hz otherwise.
- `--gb HZ`: Gaussian line width instead.
- `--size N`: transform size in complex points (a power of two).
- `--baseline MODE`: `default`, `none` or `poly:N`.
- `--no-group-delay`: keep the digital-filter group delay.
- `--min-snr X`: minimum peak height in noise SDs. Default 10.
- `--min-prominence X`: minimum prominence in noise SDs. Default 5.
- `--min-height-fraction F`: minimum height as a fraction of the tallest point. Default 0.
- `--negative`: also report negative peaks (DEPT, APT).
- `--range A:B`: only pick peaks between two shifts, in ppm.
- `--max-peaks N`: keep at most this many peaks. Default 1000.
- `--integrate A:B`: integrate a region in ppm. Repeatable.
- `--integral-reference I=V`: normalize integrals so that region I equals V. Default `0=1`.

The `--process-*` flags of `trace`, `export` and `preview` take the same values as `--phase`, `--lb`, `--size` and `--baseline`.

```bash
openreadout analyze nmr-peaks sample/1 --integrate 7.5:7.0 --integrate 3.8:3.6
```

## ephys-features

```text
openreadout analyze ephys-features [OPTIONS] <FILE>
```

- `--trace N`: trace index. Default 0.
- `--channel N`: channel index. Default: the first voltage channel, else the first current channel.
- `--sweeps LIST`: sweeps to analyse, such as `0,3,5-9`. Default: all.
- `--peak-threshold MV`: voltage a spike must cross upward. Default −20 mV.
- `--dvdt-threshold V_PER_S`: dV/dt that defines spike onset. Default 10 V/s.
- `--max-spikes N`: rows in the per-spike table. Default 25 with `--json`, every spike with `--csv`.
- `--csv sweeps|spikes|fi`: print one tidy table as CSV instead.

```bash
openreadout analyze ephys-features cell.abf --csv fi > fi.csv
```

## spikes

```text
openreadout analyze spikes [OPTIONS] <FILE>
```

- `--trace N`: trace index; pick the broadband stream. Default 0.
- `--channels LIST`: channels, such as `0-3,7`. Default: all.
- `--sweeps LIST`: sweeps or segments. Default: all.
- `--band LOW:HIGH`: band-pass in Hz (zero-phase Butterworth). Default `300:6000`.
- `--order N`: Butterworth order. Default 5.
- `--threshold K`: threshold in noise units (noise = median(|x|)/0.6745). Default 5.
- `--sign neg|pos|both`: spike polarity. Default `neg`.
- `--exclude-ms MS`: a spike must be the extreme within this many ms on either side. Default 0.1.
- `--max-seconds S`: analyse only the first S seconds of each sweep.
- `--max-times N`: spike times listed per channel. Default 1000.

## qpcr

```text
openreadout analyze qpcr [OPTIONS] <FILE>
```

The input is an RDML file (also a LightCycler 96 `.lc96p`), an Applied Biosystems `.eds`, a Rotor-Gene `.rex` or a LightCycler 480 `.ixo`.

- `--well WELL`, `--target TARGET`, `--sample SAMPLE`, `--run RUN`: only these records.
- `--cq`: also compute a threshold Cq for every curve and compare it with the vendor's.
- `--threshold T`: with `--cq`, the threshold in baseline-corrected units.
- `--baseline START-END`: with `--cq`, the baseline window in cycles.
- `--ddcq`: relative quantification (2^−ΔΔCq) per sample and target.
- `--reference TARGET`: ΔΔCq reference target. Repeatable. Default: the file's.
- `--control SAMPLE`: ΔΔCq calibrator sample. Default: the file's.
- `--standard-curve`: fit a standard curve per target (slope, R², efficiency).
- `--max-records N`: return at most this many records.
- `--undetermined-as CQ`: count wells without a Cq at this value in means and ΔΔCq. Default: leave them out and count them.

```bash
openreadout analyze qpcr plate.eds --ddcq --reference GAPDH --control untreated
```

## assay

```text
openreadout analyze assay <wells|curve|dose-response|kinetics|growth|qc> [OPTIONS] <FILE>
```

- `wells`: per-well values with roles, blank subtraction, replicate statistics and outlier flags.
- `curve`: fit a standard curve and back-calculate every well's concentration.
- `dose-response`: fit a 4PL or 5PL per compound: IC50/EC50 with confidence interval, Hill slope, top, bottom.
- `kinetics`: per-well max slope, lag time, time to max, mean slope and AUC.
- `growth`: per-well growth rate, doubling time, lag time and a logistic fit.
- `qc`: Z′, signal/background, signal/noise, SSMD and CVs from the control wells.

The input is a plate-reader export or a long CSV with `well` and `value` columns.

### Flags of every assay subcommand

- `--layout CSV`: plate layout: a plate-map grid or a long table with a `well` column.
- `--no-embedded-layout`: ignore the layout the export embeds.
- `--blank WELLS`, `--positive WELLS`, `--negative WELLS`, `--empty WELLS`: mark wells (`H1,H2` or `H1:H12`).
- `--role NAME=ROLE`: role of the wells a layout names NAME, such as `--role DMSO=negative`. Repeatable.
- `--table N`: plate index. Default 0.
- `--read READ`: read to analyse, by 1-based number or label. Default: the first measured read.
- `--wavelength NM`: the wavelength of a spectral read.
- `--blank-subtraction auto|mean|median|none`: default `auto`.
- `--outliers grubbs|mad|none`: outlier test within replicate groups. Default `grubbs`.
- `--outlier-threshold X`: alpha (Grubbs) or modified z-score (MAD).
- `--exclude-outliers`: leave flagged outliers out of means, fits and controls.
- `--csv PREFIX`: write tidy CSV tables to `PREFIX.wells.csv`, `PREFIX.samples.csv` and so on.
- `--overwrite`: replace existing output files.

### Flags of some assay subcommands

- `--reduce first|last|max|min|mean|max-slope|mean-slope|auc`: `wells`, `curve`, `dose-response`, `qc`: how a kinetic read becomes one value per well.
- `--window N`: points per window, for `--reduce max-slope` or for `kinetics` and `growth`.
- `--normalize none|controls`: `wells`, `dose-response`, `qc`: percent effect between the negative (0 %) and positive (100 %) controls.
- `--model linear|4pl|5pl`: `curve`, `dose-response`. Default `4pl`.
- `--weighting none|1/y|1/y2|1/x|1/x2`: `curve`, `dose-response`. Default `none`.
- `--confidence C`: `curve`, `dose-response`: confidence level. Default 0.95.
- `--preview PNG`: `curve`, `dose-response`: draw the points and the fit.
- `--standard WELLS=CONC`: `curve`: standard wells and their concentration. Repeatable.
- `--fit-on replicates|means`: `curve`. Default `replicates`.
- `--lloq X`, `--uloq X`: `curve`: limits of quantification. Default: from the standards' recovery.
- `--wells WELLS`: `kinetics`, `growth`: only these wells.
- `--threshold OD`: `growth`: values at or below this are left out of the log-scale fit.

```console
$ openreadout analyze assay curve assay-synth-elisa-5pl.csv --layout assay-synth-elisa-5pl-layout.csv --model 5pl
assay-synth-elisa-5pl.csv — curve of read 1 `OD450`
layout: assay-synth-elisa-5pl-layout.csv (blank 2, sample 80, standard 14)
blank: mean of 2 wells = 0.0647

standard curve: 5pl [y = d + (a - d) / (1 + (x/c)^b)^g], fit on replicates, weighting none
…
  R² 0.99992, n 14, levels 7, range 15.625–1000, LLOQ 15.625, ULOQ 1000
```

## gate

```text
openreadout analyze gate [OPTIONS] [FILE]...
```

Without an FCS file, the gating file is described (samples, compensation, transforms, the gate tree) without counts.

- `--workspace WSP`: FlowJo 10 workspace (`.wsp`).
- `--gatingml XML`: Gating-ML 2.0 document.
- `--sample NAME`: workspace sample. Default: the sample whose name or file matches FILE.
- `--population PATH`: report only this population and its descendants. Repeatable.
- `--median PARAMETER`: report each population's median of this parameter (`Comp-NAME` for compensated values). Repeatable or comma-separated.
- `--table N`: FCS data set index. Default 0.

Several FCS files, a directory or a glob give one table with a row per file and population. `gate` takes the flags in [Several inputs](index.md#several-inputs) and the [batch table flags](batch.md#batch-table-flags).

```bash
openreadout analyze gate sample.fcs --workspace analysis.wsp
openreadout analyze gate fcs/ --workspace analysis.wsp --median Comp-FITC-A -o populations.csv
```

## JSON

[`peaks`](../json/peaks.md), [`chromatogram`](../json/chromatogram.md), [`nmr-peaks`](../json/nmr-peaks.md), [`ephys-features`](../json/ephys-features.md), [`spikes`](../json/spikes.md), [`qpcr`](../json/qpcr.md), [`assay`](../json/assay.md), [`gate`](../json/gate.md).

Run `openreadout analyze <subcommand> --help` for the full help of your installed version.
