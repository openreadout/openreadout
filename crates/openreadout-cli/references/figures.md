# Figures with `plot` / `openreadout_plot`

One call draws a finished figure. A `file` on the figure is the file of every series that names none. Name the data by file and selector; put only numbers you computed yourself in `values`. The JSON reply lists what was drawn (files, selectors, ranges, marked peaks); check it against the question before you describe the picture. Annotation coordinates are in the units the axes show (the reply's `x.unit`, e.g. hours on a plate time axis). Full spec: `openreadout plot --help` and the `plot` reference page.

## Plates

```json
{"title": "Growth in every well", "facet": "plate",
 "data": {"file": "plate.pda", "subtract": "blank",
          "color_by": {"at": 6, "below": 0.1, "label": "crashed", "else": "grew"}},
 "column_labels": ["0", "0", "5", "50", "500", "5k", "50k", "500k", "5M", "50M", "500M", "blank"]}
```

- Layout groups come from the file (`groups`, with `*` patterns). `color_by` also takes `group`, `row`, `column`, `growth` (each well's pattern from the growth analysis: grew, crashed, crashed then regrew, no growth) or classes `{"crashed": "E3:H11,A9:D11"}`.
- Group means with spread and a marked time window:

```json
{"data": {"file": "plate.pda", "by": "group", "groups": ["99.9 * +", "99 * +", "95 * +"], "subtract": "first"},
 "annotate": [{"x": [7.4, 8.6], "label": "came back 7.4–8.6 h"}]}
```

- Context behind the main data: two data items, `{"wells": "E1:H2", "color": "faint", "label": "no phage"}` then `{"wells": "E3:H11", "color": "accent", "label": "with phage"}`.

## Mass spectrometry and chromatography

```json
{"panels": [{"title": "TIC", "data": {"file": "run.raw", "peaks": 1}},
            {"title": "Spectrum at the apex", "data": {"file": "run.raw", "rt": "apex", "peaks": 6}}]}
```

XICs: `"mz": [195.0877, 138.0662]`. Detector traces: `"trace": N`. Several runs overlay with `"file": [...]`, or `"facet": "series", "columns": 1` stacks them.

## Patch clamp

```json
{"panels": [{"data": {"file": "cell.abf", "show": "fi", "mark": "bars"}},
            {"data": {"file": "cell.abf", "show": "sweeps", "sweep": [4, 8, 12, 16], "spikes": true},
             "facet": "series", "x": {"range": [0, 1.6]}}], "columns": 2}
```

## Flow cytometry

```json
{"data": {"file": "e01.fcs", "x": "CD4 PE-Cy7 FLR-A", "y": "CD8 PerCP-Cy55 FLR-A",
          "workspace": "ics.wsp", "populations": ["CD4+", "CD8+"]}}
```

Fluorescence axes are logicle by default (`transform` to change). One parameter over several tubes: `{"file": ["e01.fcs", "e03.fcs"], "x": "CD107a PE FLR-A"}`.

## Spectra

IR/Raman overlays: `{"data": {"file": ["a.spa", "b.spa"], "peaks": 2}, "stack": 0.9}` (wavenumber axes run high to low). NMR: `{"file": "exp/1", "process": true, "x_range": [3.3, 5.6], "peaks": 10}` with `"y": {"range": [...]}` to clip a solvent peak; peaks are picked among what the axes show.

## Fits and tables

- Dose-response: `{"file": "plate.csv", "assay": {"layout": "layout.csv"}}`; standard curve: `"show": "standard_curve"`.
- qPCR: `{"file": "run.eds"}` (amplification, by target), `"y": {"log": true}`; `"show": "melt"`.
- A batch table: `{"table": "rows.csv", "x": "name", "y": "percent_of_parent", "by": "path", "where": ["parent=/…/CD8+"]}`.
- Inline: `{"values": {"x": ["E01", "E03"], "y": [0.004, 2.6], "y_unit": "% of CD8"}}`.

`output` (`.png`, `.svg`, `.pdf`) writes the figure for a paper or slides.
