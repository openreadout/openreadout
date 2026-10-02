# Chromatograms and peaks

`openreadout analyze chromatogram` extracts chromatograms from mass spectra and detector signals. `openreadout analyze peaks` detects and integrates their peaks, and also measures bands and regions of IR, Raman, UV-Vis and NMR spectra. This page explains what both compute and lists their output fields. The same analyses are available as the MCP tool `openreadout_analyze` (kinds `chromatogram` and `peaks`) as the Python function `openreadout.analyze(path, "chromatogram" | "peaks", ...)` and as the R function `openreadout_analyze(x, "chromatogram" | "peaks", ...)`.

```bash
openreadout analyze peaks run.D
```

This prints the peak table of the file's first detector signal, with retention time, area, height, area % and widths for each peak. The rest of this page explains how to choose the chromatogram and how the peaks are found.

## Chromatograms

```bash
openreadout analyze chromatogram run.raw --tic --bpc --mz 195.0877,138.0662 --ppm 5
openreadout analyze chromatogram run.raw --mz 120.0808 --precursor 445.12 --polarity positive   # product-ion XIC
openreadout analyze chromatogram tsq.raw --transition 279.2>179.2 --transition 293.2>113.1      # SRM/MRM
openreadout analyze chromatogram run.D --trace 1 --rt-range 2-12 -o dad.csv                      # a detector signal
openreadout analyze chromatogram run.mzML --tic -o tic.png                                       # a plot
```

Five kinds of chromatogram:

- **TIC** (`--tic`): for each scan, the total ion current the file records, or else the sum of the scan's intensities. `intensity_source` says which (`recorded` or `computed`). With `--mz-range A-B`, the sum of the intensities inside that range.
- **BPC** (`--bpc`): for each scan, the recorded base-peak intensity, or else the largest intensity.
- **XIC** (`--mz M`): for each scan, the sum of the intensities within M ± δ, where δ = M × ppm × 10⁻⁶ (`--ppm`, default 10) or a fixed `--da`. A scan with no point in the window gives 0. `--aggregate max` takes the largest intensity instead of the sum.
- **SRM** (`--transition Q1>Q3`): from the MS/MS scans whose precursor is within `--transition-tol` (default 0.5) of Q1, the intensity of the point nearest Q3 within the tolerance. When several precursors or products fall inside the tolerance, the nearest one is used and the others are listed in `notes`. Files that store transitions as chromatograms (mzML) or as MRM table columns (Waters) are read from those.
- **Stored trace** (`--trace N`, optionally `--channel C`): a chromatogram or detector signal the file stores, such as a ChemStation `.ch` or `.uv` signal, an ANDI `.cdf` file, or a stored TIC.

### Choosing scans

- `--ms-level N` (default 1; 2 with `--precursor`; any MS/MS level for SRM)
- `--polarity positive|negative`
- `--scan-filter TEXT`: a case-insensitive part of the scan filter or description, such as `"FTMS + p"`
- `--precursor MZ` with `--precursor-tol` (default 0.5)
- `--rt-range A-B`, in minutes

A file without spectra of the requested MS level falls back to its stored TIC or BPC.

### Profile and centroid data

By default each scan is read as the instrument's stored centroid list where there is one; otherwise as stored. Thermo FT scans store both. `--profile` reads profile data where both exist. Every chromatogram reports `centroid_scans` and `profile_scans`, so you can see which was used.

With profile data, the points inside the XIC window are summed. That is the integral of the profile peak over the window, not its height. A window narrower than the profile peak underestimates the ion's abundance, so use centroids or a wider window (`--ppm 20`).

### Output

The JSON output has `chromatograms[]`, each with its points and a summary. `-o FILE.csv`, `.parquet` or `.arrow` writes a table: `rt_min` plus one column per chromatogram when they share retention times, else a long table of `chromatogram`, `rt_min` and `intensity`. `-o FILE.png` or `.jpg` draws the chromatograms. `--max-points N` thins the returned points to the most intense point of each of N slices; the summary still covers every point.

The spectra are read once for all requested chromatograms. Scans of MS levels that no chromatogram needs are skipped without decoding. The work is split across `--threads` workers, and the result does not depend on their number.

## Peaks

```bash
openreadout analyze peaks run.D                                          # first detector signal, with area %
openreadout analyze peaks run.D --trace 1 --area-seconds --plot run.png
openreadout analyze peaks run.raw --mz 195.0877 --rt 5.3 --window 0.2    # the peak near 5.3 min
openreadout analyze peaks *.raw --targets compounds.csv -o results.csv   # one row per compound per file
openreadout analyze peaks run.D --integrate 5.10-5.62                    # manual integration
```

`analyze peaks` takes the same flags as `analyze chromatogram` to choose its input. By default it uses the file's first detector trace, or else its TIC.

### Method

1. **Noise.** The signal is cut into short segments. A straight line is fitted to each to remove drift, and the RMS of the residuals is computed. The noise σ is the 25th percentile over the segments, so segments that contain peaks do not count. This follows the short-term-noise idea of ASTM E685. `--noise` sets σ yourself. The method used is reported as `method.noise_method`.
2. **Smoothing.** A quadratic Savitzky–Golay filter. The window is set automatically from the typical peak width; `--smooth N` sets it, and a value below 5 turns smoothing off. Smoothing is used to find peaks, their ends, apex times and widths. Areas and heights come from the raw signal.
3. **Detection.** Each local maximum that stands out from the running baseline is followed down both sides until it reaches a valley or the baseline. It is a peak when its height above the line between its ends is at least `--min-snr` × σ (default 3, the usual limit of detection; use 10 for the limit of quantitation) and at least `--min-height`, and when it spans at least `--min-points` samples (default 3). `--min-width` drops narrower peaks.
4. **Baselines.** Baselines are straight lines between peak ends. `--baseline` selects how they are drawn:
   - `auto` (the default): like `drop`, but a peak on a sloped background ends where it meets that background. See below.
   - `drop`: peaks that share a valley get one common baseline, split by vertical lines at the valleys. Where the signal dips below the common baseline, the group is split there.
   - `valley`: every peak gets its own line from valley to valley.
   - `tangent`: like `drop`, but a small peak on the flank of a much taller one (below `--skim-ratio`, default 0.1, of its height) is skimmed off with a straight tangent. A straight skim cuts into the small peak's foot, so it underestimates that peak.

   Peaks whose signal lies below their baseline are dropped.

### Peak fields

| field | meaning |
| --- | --- |
| `rt_min` | apex time: vertex of the parabola through the three highest smoothed points |
| `start_min`, `end_min` | integration limits |
| `height` | largest raw signal above the baseline |
| `area` | trapezoidal integral of signal minus baseline, in signal × minutes (× seconds with `--area-seconds`; `area_unit` says which) |
| `area_percent` | 100 × area / the sum of all peak areas in the chromatogram |
| `width_half_min`, `width_10pct_min`, `width_5pct_min` | widths at 50, 10 and 5 % of the apex height |
| `width_base_min` | end minus start |
| `tailing_factor` | USP tailing factor, W₀.₀₅ / 2f |
| `asymmetry_factor` | b / a at 10 % height |
| `plates` | 5.54 (t_R / W½)² |
| `resolution` | to the previous peak, 1.18 (t₂ − t₁) / (W½₁ + W½₂) |
| `snr` | height / σ |
| `points` | samples from start to end |
| `baseline_code` | how each end meets the baseline: `B` baseline, `V` valley, `T` tangent, `M` manual |
| `baseline_start`, `baseline_end` | the baseline's value at each end |
| `baseline_reason` | with `auto`: why the baseline was drawn so (below) |

`snr` uses the RMS noise. The USP and EP definition, 2H/h, uses the peak-to-peak noise, which is about 5 to 6 σ, so it gives about 0.35 to 0.4 times this value.

Each chromatogram also reports:

- `method`: every parameter actually used, such as the smoothing window, the noise and thresholds.
- `peak_count`, `total_area`, `main_peak` and `main_peak_area_percent` (chromatographic purity by area normalization).
- `picked`: the peak chosen by `--rt` and `--window`, or by `--pick largest|nearest`.
- `manual`: the result of `--integrate A-B`, a straight baseline between the signal at A and at B.

### The `auto` baseline

Chromatography data systems end a peak where its flank has become as flat as the baseline. They draw one baseline under peaks that are not separated and split them with vertical lines at the valleys. On a flat baseline, `drop` does the same. On a sloped background, such as a GC solvent tail or a gradient drift, the signal between peaks keeps falling without reaching the baseline. `drop` then follows the tail down to the next valley and draws the baseline far below the peak, which inflates its area.

`auto` changes only where a flank ends. It ends a falling flank where the slope of the signal has returned to its noise level, as long as the signal does not rise again into a neighbouring peak soon after. A flat stretch that leads into a neighbour is an overlap, so the flank continues to the valley as with `drop`. Baselines and codes are then built exactly as with `drop`. Its parameters are fixed, so the same signal always gives the same peaks.

Each peak says why its baseline was drawn so:

| `baseline_reason` | meaning | codes |
| --- | --- | --- |
| `sloped_background` | an end was placed where the peak meets a sloped background | `BB`, `BV`, `VB` |
| `drop_line` | fused with a neighbour: one baseline under both, a vertical line at the valley | `BV`, `VV`, `VB` |
| `baseline_penetration` | shares a valley with a neighbour, but the signal dips below their common baseline, so each gets its own | `BB` |
| `isolated` | alone; both ends on the baseline | `BB` |

`auto` does not skim small peaks off a larger one. Use `--baseline tangent` if your method skims.

### Targeted quantitation

`--targets FILE` takes a compound list, as CSV, TSV or JSON. Each compound has a `name` and one of:

- `mz` for an XIC, with `precursor` for a product ion
- `q1` and `q3` for an SRM transition
- `trace`, optionally with `channel`, for a detector signal
- nothing, for the default chromatogram

Optional columns are `rt`, `window`, `ppm` or `da`, `polarity` and `ms_level`. Each compound gets one row with `found`, `rt_min`, `rt_shift_min`, `area`, `height`, `snr`, `width_half_min`, `tailing_factor` and the integration limits. Over many files, `-o` writes one row per compound per file. Without `--targets`, `-o` writes one row per peak.

## Spectral bands and regions

A trace whose axis is not time is a spectrum: an IR or Raman wavenumber axis, a UV-Vis wavelength, an NMR chemical shift. `analyze peaks` returns its results under `spectra[]`.

```bash
openreadout analyze peaks sample.0 --x-range 1680:1720 --x-range 1000:1060
openreadout analyze peaks sample.spa --plot bands.png
```

- **Input**: one sweep of one trace (`--trace N --sweep K`). Samples are ordered by increasing x, and non-finite samples are left out.
- **Regions** (`--x-range A:B`, repeatable, in axis units): the samples with A ≤ x ≤ B, with no interpolation at the ends. `area` is the trapezoidal integral of signal minus a baseline. The baseline is the straight line between the first and last samples (`--baseline linear`, the default) or zero (`--baseline none`); `area_no_baseline` is always given. Each region also has `max_x`, `max_height`, `max_value` and `centroid`, and lists the detected bands whose apex lies inside it.
- **Bands**: the peak detector above, run on the whole spectrum with x in place of time. Each band has `x`, `start`, `end`, `height`, `area`, `area_percent`, `centroid`, `snr` and `baseline_code`. With regions, only bands inside a region are listed.
- **Minima**: in transmittance and reflectance spectra, bands point down. They are detected on the negated signal, and their heights and areas are depths below the baseline. Region areas stay signed. Convert to absorbance for quantitative band areas.
- **Output**: `-o rows.csv` writes one row per region, else per band. `--plot` draws the spectrum with the regions or bands shaded.

On a chromatogram, `--x-range` is a manual integration in minutes, like `--integrate`.

`trace --x-range A:B` returns the samples of the same window.

## Known limits

- Negative peaks, such as refractive-index dips, are not detected. Invert the signal first.
- Peak ends are placed where the signal meets its noise. On noise-free, simulated signals the ends run out to where the signal is exactly flat; give `--noise` a realistic value for such data.
- Baselines are straight lines. There is no exponential skim, no curved baseline model and no blank subtraction.
- `auto` does not re-anchor the baseline at chosen valleys inside an unresolved hump, such as a GC hydrocarbon envelope. Data systems do that only when their method says so, and the signal alone cannot tell. There, compare `drop` and `valley` and state the choice.
- Areas are computed from the samples the file stores. Shimadzu LabSolutions areas of narrow peaks come out somewhat higher than a trapezoid over its exported samples, even with LabSolutions' own limits; its integration rule is not documented.
- Ion-mobility-filtered XICs are not available; timsTOF spectra are summed over mobility.
- The automatic smoothing window assumes peaks of similar width across the run. For runs that mix very sharp and very broad peaks, set `--smooth` and `--baseline-window`.
- Mass spectra are not integrated by `analyze peaks`; only traces are.
- To reproduce a vendor's integration exactly, use the results the file stores where the reader exposes them (for example `vendor_peaks` of Chromeleon archives in `info`), or integrate between the vendor's limits with `--integrate`.

## Validation

The automated tests compare these analyses with independent tools and with the vendors' own integration results:

- Chromatograms extracted from vendor raw files are compared scan by scan with the depositors' mzML conversions, read with pyteomics. SRM chromatograms are compared transition by transition.
- Given the same samples and limits, areas are compared with pyOpenMS `PeakIntegrator`. Given a vendor's own limits and baseline, they are compared with the vendor's areas.
- Automatic integration in every baseline mode is compared with the peak tables of Agilent ChemStation, Agilent OpenLab CDS, Shimadzu LabSolutions and Thermo Chromeleon. `auto` is the default because it is at least as close to every vendor data set as `drop`.
- Spectral regions are compared with NumPy on the samples of independent readers, with OMNIC's CSV exports and with TopSpin's integrals. Detected bands are compared with `scipy.signal.find_peaks`.

The tests are in [`crates/openreadout-corpus-tests/tests/`](https://github.com/openreadout/openreadout/tree/main/crates/openreadout-corpus-tests/tests) (`quant.rs`, `peak_agreement.rs`, `bands.rs`). How the test corpus and the reference readers work is on the [validation page](../project/validation.md).
