# Plate-reader assays

After reading a plate, most labs do one of a few standard things: subtract blanks and average replicates, fit a standard curve and back-calculate unknowns, fit dose-response curves, extract kinetic or growth rates, or check the plate's quality from its controls. `openreadout analyze` has one subcommand for each, with a documented method.

Here is a dose-response plate with three compounds, positive and negative controls, and a layout file that says which well holds what:

```bash
openreadout analyze dose-response plate.csv --layout layout.csv
```

```text
plate.csv — dose-response of read 1 `RLU`
layout: layout.csv (negative 8, positive 8, sample 80)
outliers (grubbs): F10 (flagged, kept)
CPD-A: IC50 0.235142 (95% CI 0.199935–0.27655), Hill -0.964425, top 100988.558518, bottom 1952.059038
CPD-B: IC50 1.825145 (95% CI 1.566927–2.125916), Hill -1.558539, top 99762.40999, bottom 2172.856099
CPD-C: IC50 4.28051 (95% CI 0.460937–39.751089), Hill -0.695743, top 100173.006341, bottom 9587.948892
...
```

The files are in the repository as `crates/openreadout-assay/tests/fixtures/assay-synth-dose-response*.csv`.

## Commands

```bash
openreadout analyze assay-wells   FILE [--layout L.csv] [--blank-wells H1,H2]   # blanks, replicates, CVs, outliers
openreadout analyze assay-curve   FILE [--layout L.csv] [--standard A1,A2=100] [--model linear|4pl|5pl]
openreadout analyze dose-response FILE --layout L.csv [--normalize controls] [--model 4pl|5pl]
openreadout analyze kinetics      FILE [--window N]
openreadout analyze growth        FILE [--window N]
openreadout analyze assay-qc      FILE [--positive-wells A1:H1 --negative-wells A12:H12]
```

Options shared by every subcommand:

- `--json` prints the result in the [JSON wrapper](../getting-started/reading-json.md#the-json-wrapper). The fields are listed in [assay](../reference/json/assay.md).
- `--csv PREFIX` writes tidy tables: `PREFIX.wells.csv`, `.samples.csv`, `.compounds.csv`, `.kinetics.csv`, `.growth.csv` and `.curve.csv`.
- `--table N` picks the plate in a file with several.
- `--read R` picks the read by number or label. The default is the first measured read; for `kinetics` and `growth`, the first kinetic read.
- `--wavelength-nm NM` picks a wavelength of a spectral read.
- `--plot FILE.png` draws the fitted curve (`assay-curve` and `dose-response`).

Each subcommand is also an MCP tool with the same arguments: `openreadout_assay_wells`, `openreadout_assay_curve`, `openreadout_dose_response`, `openreadout_kinetics`, `openreadout_growth` and `openreadout_assay_qc`. The tools take `layout` and the control wells at the top level, and the rarely needed options (`table`, `read`, `wavelength_nm`, `blank_subtraction`, `outliers`, `roles`, ...) in a `plate_options` object. In Python they are `openreadout.analyze(path, "assay", analysis=…, layout=…, **options)`. To run them over many plates, see [batch](batch.md#analyses-over-a-folder).

**Input.** Any plate-reader export OpenReadout reads (see [Plate readers](../formats/plate-readers.md)), or a long CSV or TSV file. A long file has a `well` column and a value column named `value`, `signal`, `od`, `absorbance`, `fluorescence`, `luminescence`, `rfu`, `rlu` or similar. For kinetics it also has a time column (`time_s`, `time_min`, `time_h` or `time`). It may carry `read`, `wavelength_nm` and layout columns such as `role` and `sample`.

## Layouts

A layout says what each well holds. Names are matched without regard to case, and common synonyms are accepted.

- `role` (or `type`, `well_type`): one of
  - `blank` (also `buffer`, `background`, `no-cell`, `medium only`)
  - `standard` (also `std`, `calibrator`, `cal`)
  - `sample` (also `unknown`, `unk`, `test`)
  - `positive` (also `pos`, `pc`, `high control`, `max`)
  - `negative` (also `neg`, `nc`, `low control`, `min`)
  - `control`, a control whose sign is not known (also `ctrl`, `qc`, `vehicle`, `DMSO`, `untreated`)
  - `empty` (also `none`, `unused`)
- `sample` (or `sample_id`, `name`, `label`): the replicate-group name. When there is no `role`, the role is read from the name: `STD3` and `S1` are standards, `BLK` is a blank, `POS1` and `PosCtrl` are positive controls, `NEG1` and `NegCtrl` are negative controls, `DMSO` and `vehicle` are controls, and anything else is a sample.
- `compound` (or `drug`, `treatment`, `inhibitor`): the dose-response series.
- `concentration` (or `conc`, `dose`): a number. A unit after it (`1 microg/ml`) or in the header (`concentration (nM)`) is reported as `layout.concentration_unit`.
- `dilution` (or `dilution_factor`, `df`): `1:10`, `1/10`, `10x` and `10` all mean a factor of 10. A plain number below 1 is a fraction, so `0.5` means 2.
- `group` (or `replicate`): an explicit replicate group.
- Any other column is kept under `extra`.

A layout file can have either of two shapes, separated by commas, tabs or semicolons. These are the same shapes that [sample sheets](batch.md#sample-sheets-and-plate-layouts) accept.

- **Plate-map grid**: one block per field. The header line has the field name in its first cell and the column numbers `1 2 3 …`; then one line per row letter. Blank lines separate blocks.
- **Long table**: a header with a `well` column (`A1` or `A01`, or `row` and `column`) and one line per well.

```text
role,1,2,3,4
A,blank,standard,standard,sample
B,blank,standard,standard,sample

concentration,1,2,3,4
A,,100,50,
B,,100,50,
```

Layout information is merged well by well, and later sources override earlier ones:

1. The layout the export embeds, such as Gen5 `Well ID` and `Conc/Dil`, SkanIt sample matrices and layout sheets, or BMG content maps. `--no-embedded-layout` ignores it.
2. `--layout FILE`.
3. The well flags `--blank-wells`, `--positive-wells`, `--negative-wells` and `--empty-wells` (wells as `H1,H2`, `A1:H1` or `A1-A12`), and `--standard WELLS=CONC` (repeatable).

### Roles

Some names do not say which way a control points. A vehicle or untreated well is the no-effect control of an inhibition assay but the full-signal control of an activation assay. Such wells are controls of unknown sign and are left out of normalization and Z′ until you map them:

```bash
openreadout analyze dose-response plate.xlsx --layout layout.csv --role DMSO=negative
```

`--role NAME=ROLE` (repeatable) sets the role of every well whose role text or sample name is NAME. `--role CTL=positive` also covers `CTL1` and `CTL2`. It overrides every other source.

The output's `warnings` list what may be wrong with the layout: a role text that is not a role name, controls of unknown sign, and `--role` entries that matched no well. Each warning names the `--role` argument that fixes it.

## Steps

1. **Read** the selected read and wavelength. A kinetic read used in an endpoint analysis (`wells`, `curve`, `dose-response`, `qc`) needs `--reduce`:
   - `max-slope`: the largest slope over a sliding window, per minute
   - `mean-slope`: the least-squares slope over all points, per minute
   - `first`, `last`, `max`, `min`, `mean`
   - `auc`: the trapezoidal area, value × seconds
2. **Group replicates** by the explicit `group`, else the sample name, else compound and concentration, else role.
3. **Flag outliers** on the values as read, one per group at most:
   - `--outliers grubbs` (the default): the two-sided Grubbs test at alpha 0.05, in groups of at least 3 wells. `--outlier-threshold` changes alpha.
   - `--outliers mad`: a modified z-score above 3.5, in groups of at least 5 wells.
   - `--outliers none`.

   Flagged wells are kept unless `--exclude-outliers` is given.
4. **Subtract blanks.** `--blank-subtraction auto` (the default) subtracts the mean of the blank wells when there are any. `mean` and `median` require blanks; `none` skips the step. Kinetic analyses subtract blanks per time point.
5. **Summarize** each group: `n`, `n_used`, `mean`, `sd` (n − 1) and `cv_percent`.
6. Run the analysis.

## Standard curves

`analyze assay-curve` fits the standard wells and back-calculates the concentration of every other well.

| model | formula |
| --- | --- |
| `linear` | y = slope · x + intercept |
| `4pl` (default) | y = d + (a − d) / (1 + (x/c)^b) |
| `5pl` | y = d + (a − d) / (1 + (x/c)^b)^g |

In the 4PL, a is the response at zero concentration, d the response at infinite concentration, c the inflection point and b > 0 the slope factor. This is the 4PL of Gen5 and SkanIt. In the 5PL, g > 0 is the asymmetry.

**Fitting.** The fit minimizes the weighted sum of squared residuals. `--weighting` is `none` (the default), `1/y`, `1/y2`, `1/x` or `1/x2`. Logistic models are fitted from several starting points, and the best fit is kept. A concentration of 0 is allowed. The fit needs at least 2 distinct concentrations for `linear`, 4 for `4pl` and 5 for `5pl`. `--fit-on replicates` (the default) fits every standard well; `--fit-on means` fits the mean of each level.

**Fit results** (`curve.fit`):

- `parameters[]`, each with `value`, `se` and a confidence interval (`--confidence`, default 0.95). Standard errors follow the convention of SciPy `curve_fit`, R `nls` and GraphPad.
- `r_squared`, `residual_se`, `sse`, `df`, `n_points`, `n_levels`, `converged`
- `points[]`, with each point's `x`, `y`, `fitted` and `residual`

**Back-calculation.** Every well and every group mean is converted through the inverse of the curve. `flag` says how far to trust it:

- `in_range`: within the standards' concentration range
- `below_range`, `above_range`: outside it; the extrapolated concentration is still reported
- `below_curve`, `above_curve`: the signal is beyond an asymptote, so there is no concentration
- `not_computable`: the curve is flat

`final_concentration` is the back-calculated value times the dilution. Standards also get `recovery_percent`. LLOQ and ULOQ are the lowest and highest standards whose back-calculated mean is within 20 % of nominal with a CV of at most 20 %; `--lloq` and `--uloq` set them yourself. `quantifiable` says whether a well lies between them.

## Dose-response

`analyze dose-response` fits a 4PL (default) or 5PL curve per compound to the sample wells that have a concentration.

`--normalize controls` first converts every value to percent effect: 100 × (y − negative mean) / (positive mean − negative mean). The negative control is 0 %, the positive control 100 %. On `analyze assay-wells`, the same option adds `percent_effect` to every well.

Each entry of `compounds[]` has:

- `ec50` and `kind`: `IC50` when the response falls with dose, else `EC50`
- `ec50_ci_low`, `ec50_ci_high`: the confidence interval, computed on the log scale so it is always positive
- `log10_ec50`, and `absolute_ec50` when normalized (where the curve crosses 50 %)
- `hill_slope`: positive when the response rises with dose, as in GraphPad
- `top`, `bottom`, `response_at_zero`, `response_at_infinity`
- `min_concentration`, `max_concentration`, and `extrapolated` when the EC50 lies outside them
- `error`, for example when there are fewer than 4 doses or the fit did not converge

## Kinetics

`analyze kinetics` computes per well of a kinetic read. `--window` sets the points per sliding window; the default is 5 or a tenth of the time points, whichever is larger.

| field | meaning |
| --- | --- |
| `max_slope_per_min` | the largest least-squares slope over any window (Vmax), per minute |
| `max_slope_r_squared`, `window_start_s`, `window_end_s` | that window's fit and limits |
| `lag_time_s` | where the max-slope line crosses the baseline |
| `mean_slope_per_min` | the least-squares slope over all points |
| `max_value`, `time_to_max_s`, `initial_value`, `final_value` | as named |
| `auc` | trapezoidal area under the values, value × seconds |

## Growth curves

`analyze growth` computes per well after background correction. The background is the blank wells per time point when there are blanks, else the well's own minimum, as in growthcurver.

| field | meaning |
| --- | --- |
| `growth_rate_per_h` | µmax: the largest slope of ln(value) against time in hours over any window, using points above `--threshold` (default 5 % of the well's maximum) |
| `doubling_time_h` | ln 2 / µmax |
| `exp_phase_start_s`, `exp_phase_end_s` | that window |
| `lag_time_s` | where the µmax line crosses the starting level |
| `logistic_k`, `logistic_r_per_h`, `logistic_n0`, `logistic_doubling_time_h` | a least-squares fit of the logistic curve N(t) = K / (1 + ((K − N0)/N0) e^(−rt)), the model growthcurver fits |
| `max_value`, `time_to_max_s`, `auc_h` | as named; area in value × hours |

The two rates answer different questions. µmax is the steepest exponential growth seen. The logistic r belongs to the best-fitting logistic curve and is smaller when the growth is not logistic. Say which one you report.

## Assay quality

`analyze assay-qc` reports plate quality from the control wells. The other analyses add the same `quality` block whenever the layout has positive and negative controls. It uses the values before blank subtraction.

- `positive`, `negative`, `blank` and `samples`: group statistics
- `z_prime`: 1 − 3 (SD₊ + SD₋) / |mean₊ − mean₋| (Zhang, Chung and Oldenburg, 1999), with `assessment` `excellent` (at least 0.5), `marginal` (0 to 0.5) or `unusable` (0 or below)
- `z_factor`: samples against the negative control
- `signal_to_background`, `signal_to_noise` and `ssmd`
- `median_replicate_cv_percent`, over groups of two or more wells

## Errors

Mistakes in the request are usage errors (exit 2) with a message that says what to add: no standards or concentrations, no controls for `assay-qc` or `--normalize controls`, a kinetic read without `--reduce`, a spectral read without `--wavelength-nm`, an endpoint read for `kinetics` or `growth`, too few concentrations for the model, or a bad layout line. A file that is not a plate gives the same exit code as `info` would.

## Validation

The curve fits, back-calculation, dose-response, kinetics, growth and quality metrics are compared with independent implementations and with the results that plate-reader software stores in its exports. The method is on the [validation page](../project/validation.md); the reference tools and data are logged in [docs/provenance/plate-analysis.md](https://github.com/openreadout/openreadout/blob/main/docs/provenance/plate-analysis.md).
