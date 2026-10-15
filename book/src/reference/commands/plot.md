# plot

`plot` draws a finished figure from lab files: traces and sweeps, chromatograms and mass spectra, plate wells in their plate positions or as group means, FCS dot plots and histograms, f–I curves, dose-response and standard-curve fits, and the columns of a table file. You describe the figure in JSON. The same JSON is the arguments of the MCP tool `openreadout_plot`, which returns the figure as a picture; `openreadout_export` with `plot` set to that JSON saves it (see [saving figures](../mcp.md#saving-figures)).

```text
openreadout plot [OPTIONS] <SPEC>
```

`SPEC` is a `.json` file or the JSON text itself. The figure is drawn in the OpenReadout look (IBM Plex, paper and ink, a warm accent, direct labels), light or dark, and written to `-o` as PNG, JPEG or TIFF (at twice the pixel size unless `--scale` says otherwise), SVG or PDF. Every write goes to a temporary file that is read back and compared before it is renamed into place.

## Flags

- `-o`, `--output FILE`: `.png`, `.jpg`, `.tiff`, `.svg` or `.pdf`. Required.
- `--scale N`: pixels per figure pixel in PNG, JPEG and TIFF files, 0.25–8. Default 2. Text and lines are drawn again at that size.
- `--dpi N`: print resolution. PNG, JPEG and TIFF files record it, so they open at pixels ÷ dpi inches; SVG and PDF pages get the size the raster would print at. A 1200-pixel figure at the default scale and `--dpi 300` prints 8 inches wide.
- `--overwrite`: replace an existing output file.
- `--theme light|dark`: overrides the spec's `theme`.
- `--json`: print what was drawn as JSON (the same summary the MCP tool returns) and the file written under `written`.

## The spec

A figure is one panel, with the panel's keys at the top level, or several in `panels` (side by side, `columns` per row). Each panel has `data`: one series spec or a list of them, drawn in order. A `file` on the figure or on a panel is the file of every series there that names none, so a figure of one file names it once: `{"file": "cell.abf", "panels": [{"data": {"show": "fi"}}, {"data": {"show": "sweeps"}}]}`. A `file` alone draws that file's default view. A key in the wrong place, such as `show` at the top level or an axis object inside `data`, is an error that says where it goes. A long `caption` wraps.

```json
{"title": "Plate 1: growth in every well",
 "data": {"file": "120308b_2.pda", "subtract": "blank",
          "color_by": {"at": 6, "below": 0.1, "label": "crashed", "else": "grew"}},
 "facet": "plate",
 "column_labels": ["0", "0", "5", "50", "500", "5k", "50k", "500k", "5M", "50M", "500M", "blank"]}
```

### Figure

| Key | What it does |
| --- | --- |
| `title`, `caption` | Text over and under the figure. |
| `panels` | Several panels, each with its own `title` and the panel keys below. |
| `columns` | Panels per row; with one faceted panel, small multiples per row. |
| `share` | `x`, `y` or `both`: panels use the same range. |
| `width`, `height` | Pixels. Default 1200 wide (960 for a lone dot plot) and a height that suits the content; at most 3000. |
| `theme` | `light` (default) or `dark`. |
| `strict` | Refuse values that this file's assurance does not validate. |

### Panel

| Key | What it does |
| --- | --- |
| `data` | One series spec or a list. |
| `facet` | `series`: one small plot per series (sweeps, files, groups) on shared axes. `plate`: plate wells in their positions, with the layout groups as row headings. |
| `x`, `y` | Axis: `label`, `log`, `range` `[low, high]`, `reverse`, and for time axes `unit` (`s`, `min`, `h`). |
| `annotate` | A list of bands, lines, labelled points, arrows and notes (below). |
| `legend` | `direct` (labels at the line ends), `key` or `none`. Default: direct labels when they fit. |
| `column_labels`, `row_labels` | Headings of a plate facet. Row labels are one per row, or one per block of rows that share a layout group. |
| `stack` | Offset each series upward by this fraction of the tallest (stacked spectra). |

### Series

A series names its data by `file` and selector, by a `table` file and its columns, or by a few inline `values`. Several files in `file` give one series each. `show` says what to draw; the default comes from the file.

| `show` | Data | Selectors |
| --- | --- | --- |
| `trace` | A sampled signal or 1-D spectrum (IR, Raman, UV-Vis, NMR, ÄKTA, detector traces) | `trace`, `sweep` (one or a list), `channel`, `x_range`, `process` (NMR FIDs), `peaks` |
| `sweeps` | Patch-clamp sweeps, named by their current step | `sweep` (default all), `spikes` marks action potentials |
| `fi` | Spikes in the current step (or `y: "rate_hz"`) against injected current | `trace`, `channel`, `sweep` |
| `chromatogram` | TIC (default for mass spectrometry), BPC, XICs, stored or detector traces | `chromatogram`, `mz`, `ppm`, `trace`, `x_range` (minutes), `peaks` |
| `spectrum` | One mass spectrum | `rt` (minutes, or `"apex"` for the TIC's highest point) or `scan`, `peaks` |
| `wells` | Plate reads: a curve per well (kinetic), or values per well | `wells` (`A1:H11`), `groups` (layout groups, `*` matches any text), `by` (`well` or `group`), `spread`, `subtract` (`blank`, `first`), `read`, `color_by` |
| `events` | FCS events, x against y, coloured by gate population | `x`, `y`, `populations`, `workspace` or `gatingml`, `compensate`, `transform` |
| `histogram` | One FCS parameter or table column | `x`, `populations`, `transform` |
| `dose_response`, `standard_curve` | Points and fitted curves from the [plate-reader assays](analyze.md#plate-reader-assays) | `assay`: their arguments without `file` (`layout`, `standards`, `model`, `normalize`, …) |
| `amplification`, `melt` | qPCR curves, coloured by target | `wells`, `target` |

Every series also takes `mark` (`line`, `points`, `line_points`, `bars`, `area`, `sticks`, `density`), `label` and `color` (`ink`, `accent`, `muted`, `faint`, `blue`, `ochre`, `green`, `purple`, `teal`, `brown` or `#RRGGBB`). A `label` on a data item that gives several series (wells, files) names them once, as one colour class.

- `table`: a `.csv`, `.tsv`, `.jsonl`, `.json` or `.parquet` file, such as `batch -o` writes. `x` and `y` are column names, `error` a column of error-bar half-widths, `by` the column that splits rows into series and `where` filters rows (`["parent=CD8+"]`). Text in `x` gives bars over categories.
- `values`: `{"x": [...], "y": [...], "error": [...], "labels": [...], "x_unit": "...", "y_unit": "..."}` for a few numbers you computed. x may be category names. The result counts these series under `inline_series`, so a reader can tell drawn data from computed data.

Plate groups come from the layout the file stores (SoftMax Pro groups, Gen5 well IDs, a long CSV's `sample` column). `by: "group"` draws the mean of each group with its spread (`sd`, `sem`, `range` or `none`). With `groups` patterns that contain `*`, each pattern pools its matching groups into one series: `"groups": ["99.9 * +", "99 * +"]`. A `color_by` rule flags wells by their value at one point on the x axis, in the unit the axis shows: `{"at": 6, "below": 0.1, "label": "crashed", "else": "grew"}`. `color_by: "growth"` colours each well by its pattern in the growth analysis (`analyze growth`): grew, crashed, crashed then regrew, or no growth. Blank wells are drawn faint.

`peaks: N` labels the N highest peaks that the axes show, so a narrowed `x_range` or a clipped `y` range (a solvent peak cut off) labels the peaks you can see.

### Annotations

`x` and `y` are in the units the axes show (hours on a plate time axis that shows hours). A number on one axis draws a line, a pair `[a, b]` a band, numbers on both a labelled point; `to` turns the point into an arrow from the label to `[x, y]`.

```json
"annotate": [
  {"x": [7.4, 8.6], "label": "came back 7.4–8.6 h"},
  {"y": 0.1, "label": "threshold"},
  {"x": 4, "y": 0.18, "label": "crash", "to": [5.5, 0.02]}
]
```

Each annotation also takes `color`, `marker: false` (text without a dot) and `facet` (only in the small plot with this title).

## Output

```console
$ openreadout plot hk97.json -o hk97.png
wrote hk97.png (1200 × 742 px, 351722 bytes, verified)
  44 series; x time (h); y absorbance 600 nm, blank-subtracted (OD)
  note: subtracted the mean of 8 blank well(s) at each reading
```

With `--json`, each panel lists its axes (label, unit, range, scale) and its series: the file and what was read from it, points drawn, x and y ranges and colour. Many wells of one colour are summarized as one entry with a `count`. Peaks, apexes and spikes appear under `marked` with their coordinates. `notes` says what was done to the data and `warnings` what could not be drawn as asked. Field list: `openreadout self schema plot`.

## Examples

```bash
# the TIC with its apex, and the spectrum at the apex with the six largest peaks
openreadout plot '{"panels": [{"data": {"file": "run.raw", "peaks": 1}},
                              {"data": {"file": "run.raw", "rt": "apex", "peaks": 6}}]}' -o run.pdf

# an f–I bar chart and four sweeps with their spikes
openreadout plot '{"panels": [{"data": {"file": "cell.abf", "show": "fi", "mark": "bars"}},
                              {"data": {"file": "cell.abf", "show": "sweeps", "sweep": [4, 8, 12, 16], "spikes": true}, "facet": "series"}]}' -o cell.png

# CD4 against CD8 with the workspace's populations
openreadout plot '{"data": {"file": "e01.fcs", "x": "CD4 PE-Cy7 FLR-A", "y": "CD8 PerCP-Cy55 FLR-A",
                            "workspace": "ics.wsp", "populations": ["CD4+", "CD8+"]}}' -o e01.svg

# per-group numbers from a batch table
openreadout plot '{"data": {"table": "gates.csv", "x": "name", "y": "percent_of_parent", "by": "path"}}' -o gates.png
```
