# Integrate chromatogram peaks

Use this when you have an HPLC, GC or LC-MS run and want its peak table: retention times, areas, heights, widths, tailing and S/N, or the area of one named compound in every run.

## Run it

```text
$ openreadout analyze peaks cheminfo-agilent-hplc.cdf
cheminfo-agilent-hplc.cdf (andi-chrom)

DAD1 A, Sig=254,4 Ref=360,100: 17 peaks, area in mAU·min, noise σ 0.002 (segment_rms), smoothing 17 pts, baseline auto
 #  RT min     start     end     area  area %    height      S/N  W½ min  tailing  type
──  ──────  ────────  ──────  ───────  ──────  ────────  ───────  ──────  ───────  ────
 1   1.501  2.000e-4   2.107   3.5713    2.54    2.8741   1409.9       -        -  BV
 2   2.107     2.107   2.327   0.5086    0.36    2.3345   1145.2       -        -  VV
 ...
 5   3.270     3.107   3.900   9.7211    6.92  100.9718  49531.4  0.0885     1.34  VB
 6   5.541     3.994   7.854   6.9947    4.98    5.1854   2543.7  1.2721     1.02  BB
 ...
14  17.169    16.467  18.287  39.1151   27.83   80.3328  39407.0  0.4436     1.22  BV
15  19.629    18.287  22.580  67.0857   47.73  117.4083  57594.3  0.4949     1.20  VB
16  23.098    22.767  25.594   0.0774    0.06    0.0512     25.1  1.3371     4.48  BB
17  26.694    25.800  28.927   0.1958    0.14    0.1276     62.6  1.5553     1.74  BB
```

`cheminfo-agilent-hplc.cdf` is an Agilent HPLC run in ANDI (netCDF) format, committed at [`fuzz/corpus/whole_andi/`](../../../fuzz/corpus/whole_andi/cheminfo-agilent-hplc.cdf). The output on this page is real, with long tables trimmed. A ChemStation `.D` directory works the same way.

## What it tells you

- The heading line names the signal that was integrated (here the DAD channel at 254 nm), the area unit, the noise σ and the smoothing window that were used. Without options, `peaks` takes the file's first detector trace, or else its TIC.
- `start` and `end` are the integration limits, in minutes. `area` is the integral of signal minus baseline between them; `area %` is the share of the total area of all peaks, so it doubles as purity by area normalization.
- `S/N` is height over the RMS noise σ. The default threshold is 3 (`--min-snr`); use 10 for a limit of quantitation. The USP peak-to-peak definition gives about 0.35 to 0.4 times this value.
- `type` says how each end meets the baseline: `B` baseline, `V` valley (peaks 14 and 15 share one baseline, split at the valley), `T` tangent, `M` manual.
- A `-` in `W½ min` or `tailing` means the value could not be measured. In this run every such peak is fused with a neighbor (`baseline_reason` `drop_line` in the JSON), so the signal never falls to that height between them.

The method, every field and the `auto` baseline are described in [Chromatograms and peaks](../guides/quantitation.md).

## Variations

### One compound in many runs

A compound list gives one row per compound per file. Each row has a `name` and, for this detector trace, an expected `rt` and `window` in minutes:

```text
$ cat compounds.csv
name,rt,window
main,3.27,0.2
late,19.6,0.3
$ openreadout analyze peaks cheminfo-agilent-hplc.cdf --targets compounds.csv -o rows.csv
...
compound  chromatogram                   expected  RT min     area    height      S/N  found
────────  ─────────────────────────────  ────────  ──────  ───────  ────────  ───────  ─────
main      DAD1 A, Sig=254,4 Ref=360,100     3.270   3.270   9.7211  100.9718  49531.4  yes
late      DAD1 A, Sig=254,4 Ref=360,100    19.600  19.629  67.0857  117.4083  57594.3  yes
wrote 2 rows to rows.csv
```

`rows.csv` also has `rt_shift_min`, `width_half_min`, `tailing_factor` and the limits. Give a directory or several files instead of one to get every run in the same file. For mass spectra, put `mz` (an XIC) or `q1` and `q3` (an SRM transition) in the list instead.

`analyze peaks` reports areas, not concentrations. To quantify against standards, run the standards through the same list and fit the areas against their known amounts yourself.

### An ion from an mzML file

```bash
openreadout analyze peaks run.mzML --mz 195.0877 --ppm 5 --rt 5.3 --window 0.2
```

`--mz` integrates an extracted-ion chromatogram, and `--rt` with `--window` picks the peak nearest the expected time. `--tic`, `--bpc` and `--transition Q1>Q3` choose other signals.

### A summary per run

`batch` turns a folder into one table. Here `runs/` holds the ANDI file and `CA10_100uM.D`, a ChemStation directory with one DAD signal, `dad1A.ch` (the first part of the bundle in [`fuzz/corpus/whole_chemstation/`](../../../fuzz/corpus/whole_chemstation/)):

```text
$ openreadout batch peaks runs/ --set rows=chromatogram --fields peak_count,total_area,main_peak_area_percent
2 data sets (2 ok, 0 failed)
path                            format       peak_count  total_area  main_peak_area_percent
──────────────────────────────  ───────────  ──────────  ──────────  ──────────────────────
runs/CA10_100uM.D               chemstation          53     19.6066                 62.6849
runs/cheminfo-agilent-hplc.cdf  andi-chrom           17    140.5482                 47.7315
```

### From an assistant

The MCP tool is `openreadout_peaks`. Its arguments are the flag names in snake case (`rt`, `window`, `min_snr`, `baseline`), and `compounds` takes the compound list as a JSON array.

## More

- [Chromatograms and peaks](../guides/quantitation.md): noise, smoothing, baselines and validation.
- [`analyze` reference](../reference/commands/analyze.md#peaks-and-chromatogram): every flag.
- JSON: [`peaks`](../reference/json/peaks.md), [`chromatogram`](../reference/json/chromatogram.md).
