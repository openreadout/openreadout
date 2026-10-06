# Fit a dose–response curve

Use this when a plate reader has measured a dilution series of one or more compounds and you want IC50 or EC50 values with confidence intervals, plus the plate's Z′ from its controls.

## Run it

```text
$ openreadout analyze dose-response assay-synth-dose-response.csv --layout assay-synth-dose-response-layout.csv
assay-synth-dose-response.csv — dose-response of read 1 `RLU`
layout: assay-synth-dose-response-layout.csv (negative 8, positive 8, sample 80)
outliers (grubbs): F10 (flagged, kept)
CPD-A: IC50 0.235142 (95% CI 0.199935–0.27655), Hill -0.964425, top 100988.558518, bottom 1952.059038
CPD-B: IC50 1.825145 (95% CI 1.566927–2.125916), Hill -1.558539, top 99762.40999, bottom 2172.856099
CPD-C: IC50 4.28051 (95% CI 0.460937–39.751089), Hill -0.695743, top 100173.006341, bottom 9587.948892

group                  role          n         mean         sd     cv%         conc         flag
CPD-A @ 10             sample        3  4473.666667 162.124438     3.6            -
CPD-A @ 3.33333        sample        3         9132 148.016891     1.6            -
...
negative               negative      8   101095.625 3458.018216     3.4            -
positive               positive      8     1946.625 205.558429    10.6            -
...

quality: Z′ 0.889149 (excellent), S/B 51.933796, S/N 28.672203, SSMD -28.621679, median replicate CV 2.301801%
```

The plate (a 96-well luminescence grid) and its layout are synthetic test files in the repository at [`crates/openreadout-assay/tests/fixtures/`](../../../crates/openreadout-assay/tests/fixtures/). The output on this page is real, with long tables trimmed. Any plate-reader export OpenReadout reads works the same way (see [Plate readers](../formats/plate-readers.md)).

The layout is a long table with one line per well:

```text
well,role,compound,concentration
A1,sample,CPD-A,10
A2,sample,CPD-A,3.33333
...
A11,negative,,
A12,positive,,
```

## What it tells you

- The first lines say which read was analyzed (read 1, `RLU`) and how many wells of each role the layout gave. Without a layout that names sample wells and their concentrations there is nothing to fit, and the command stops with a usage error (exit 2).
- Each compound gets a 4PL fit. `IC50` means the signal falls with dose, `EC50` that it rises. The 95 % interval is computed on the log scale. A wide one, as for CPD-C, means the data do not pin the midpoint down: at the highest dose CPD-C still gives 42159.5, far above the positive control's 1946.6, so the fit has to extrapolate its bottom.
- `Hill` is negative when the response falls with dose. `top` and `bottom` are in the read's units.
- `outliers (grubbs)` names wells that the Grubbs test flagged within their replicate group. They are kept unless you pass `--exclude-outliers`.
- The group table gives `n`, mean, SD and CV per concentration.
- `quality` comes from the positive and negative controls: Z′ = 1 − 3 (SD₊ + SD₋) / |mean₊ − mean₋|. At least 0.5 is `excellent`, 0 to 0.5 `marginal`, 0 or below `unusable`.

## Variations

### Percent of control

`--normalize controls` converts every well to percent effect between the negative (0 %) and positive (100 %) controls before fitting. The IC50 values stay the same; `top`, `bottom` and the Hill slope are now in percent effect:

```text
$ openreadout analyze dose-response assay-synth-dose-response.csv --layout assay-synth-dose-response-layout.csv --normalize controls
...
CPD-A: IC50 0.235142 (95% CI 0.199935–0.27655), Hill 0.964425, top 99.994519, bottom 0.107985
```

`--model 5pl` fits an asymmetric curve instead. If your layout calls the controls `DMSO` or `vehicle`, say which way they point with `--role DMSO=negative`.

### Plate quality only

`analyze assay-qc` reports the same quality block without fitting. Without a layout, mark the controls with flags:

```text
$ openreadout analyze assay-qc assay-synth-dose-response.csv --positive-wells A12:H12 --negative-wells A11:H11
assay-synth-dose-response.csv — qc of read 1 `RLU`
layout: --positive-wells, --negative-wells (negative 8, positive 8, unassigned 80)
...
quality: Z′ 0.889149 (excellent), S/B 51.933796, S/N 28.672203, SSMD -28.621679, median replicate CV 6.990138%
```

Z′ is the same. The median replicate CV differs from the run above because, without the layout, each of the 80 other wells is a group of one, so the median covers only the two control groups.

### Tables and a plot

```text
$ openreadout analyze dose-response assay-synth-dose-response.csv --layout assay-synth-dose-response-layout.csv --csv dr --plot dr.png
...
wrote dr.samples.csv
wrote dr.compounds.csv
wrote dr.png
```

`dr.compounds.csv` has one row per compound with `ec50`, `ec50_ci_low`, `ec50_ci_high`, `hill_slope`, `top`, `bottom`, `r_squared` and `error`. It also writes `dr.wells.csv`.

### Many plates

```text
$ openreadout batch assay plates/ --set analysis=dose-response --set layout=assay-synth-dose-response-layout.csv --fields compound,kind,ec50,hill_slope
2 data sets (2 ok, 0 failed)
path               format  compound  kind    ec50  hill_slope
─────────────────  ──────  ────────  ────  ──────  ──────────
plates/plate1.csv  plate   CPD-A     IC50  0.2351     -0.9644
plates/plate1.csv  plate   CPD-B     IC50  1.8251     -1.5585
...
```

The MCP tool is `openreadout_dose_response`, with `file` and `layout`.

## More

- [Plate-reader assays](../guides/plate-analysis.md): layouts, roles, the fit and every output field.
- [`analyze` reference](../reference/commands/analyze.md#assay): every flag.
- JSON: [`assay`](../reference/json/assay.md).
