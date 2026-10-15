# preview

`preview` renders a PNG, JPEG or SVG picture of the data: an image plane, a channel composite or a maximum projection; a trace; a mass spectrum; a plate heat map, or a grid of growth curves for a kinetic plate read; or a density plot of flow cytometry events. Spectra and chromatograms get their largest peaks labelled, and you can draw your own annotations on any of them.

```text
openreadout preview [OPTIONS] <FILE>
```

The kind of picture follows the file: images first, then traces, spectra and tables. The trace, spectrum and table flags pick another. The same request on the same file always gives the same bytes.

## Flags

### Output

- `-o`, `--output FILE`: `.png`, `.jpg` or `.svg`; `-` writes the image to stdout. Default: `<input stem>.preview.png` next to the input.
- `--format png|jpeg|svg`: default from the output extension, else PNG. SVG keeps text as text and embeds image data as PNG.
- `--dark`: draw in the dark theme.
- `--quality N`: JPEG quality, 1–100. Default 90.
- `--max-size N`: longest side of the picture in pixels (16–8192), rulers included. Default 1024.
- `--overwrite`: replace an existing output file.
- `--json`: print the report as JSON.

### Images

- `--image N`: image index. Default 0.
- `--select SEL`: planes, such as `c=1`, `z=4` or `c=0,1,z=2`. Repeatable. Default: channel 0, the middle z, t 0. Several channels imply `--composite`.
- `--mip z|t`: maximum-intensity projection over z or t, limited to the selected range.
- `--composite`: blend all (or the selected) channels in their colours.
- `--level N`: pyramid level. Default: the level nearest `--max-size`.
- `--region X,Y,W,H`: only this rectangle, in full-resolution pixels (or in the pixels of `--level` when one is given).
- `--contrast auto|min-max|percentile:LO,HI|raw`: `auto` stretches the 0.1–99.9 percentiles. Default `auto`.
- `--lut gray|channel-color`: default gray for one channel, channel colours for composites.
- `--axes rulers|grid|none`: `rulers` (default) frames the picture with rulers in full-resolution pixels and a scale bar, `grid` adds faint grid lines at the ruler ticks, `none` gives the bare plane.

### Traces, spectra, plates and events

- `--trace N`, `--sweep N`, `--channel N`: the trace, sweep and channels to draw (`--channel` is repeatable; default the first 8).
- `--run N`, `--spectrum N`, `--scan N`: the mass spectrum to draw, by zero-based index or instrument scan number.
- `--centroid`: draw the instrument's centroid list instead of the profile.
- `--table N`, `--column NAME`: the table, and the column to draw: a plate's value column (long layout), or the x and then the y parameter of flow cytometry events (`--column` is repeatable; one parameter gives a histogram; default FSC-A against SSC-A).
- `--marks auto|peaks|spikes|objects|none`: marks drawn from the data. `auto` (default) labels the largest peaks of spectra and chromatograms with their position; `peaks` does so on any trace; `spikes` marks the action potentials of a current-clamp sweep (upward crossings of −20 mV, as [`analyze ephys-features`](analyze.md) counts them); `objects` outlines and numbers the nuclei, cells or spots in the area shown, with the count, as [`stats --objects`](stats.md#objects) finds them (at a coarser pyramid level when the view is larger than 4096 × 4096 pixels). The JSON gives the counts and the first 20 objects under `objects`.
- `--objects JSON`: with `--marks objects`, how objects are found, with the fields of the MCP `objects` option: `{"kind":"spots"}`, `{"diameter_um":8}`.

### Annotations

- `--annotate JSON`: marks of your own, as a JSON object or array, inline or `@FILE`. See [Annotations](#annotations).

### NMR

- `--process`: draw FIDs as spectra processed by OpenReadout.
- `--process-phase MODE`, `--process-lb HZ`, `--process-size N`, `--process-baseline MODE`: processing settings. See [`analyze nmr-peaks`](analyze.md#nmr-peaks).

## Examples

```console
$ openreadout preview doctor.tif -o doctor.png
wrote doctor.png (47x49 png, 203 bytes, verified=true): image 0 level 0 c=[0] z=[1] t=[0] contrast percentile:0.1,99.9
view it: open (or Read) doctor.png; zoom with --region X,Y,W,H in full-res px read off the rulers (now showing 0,0,16,8)
```

```bash
openreadout preview cells.czi --select c=0,1,2 --mip z -o cells.png
openreadout preview run.raw --scan 1200 -o scan1200.png
openreadout preview growth.pda -o growth.svg
openreadout preview cell.abf --sweep 10 --marks spikes -o sweep10.png
```

## Zooming into large images

Image previews have rulers along the top and left edges in full-resolution pixel coordinates, whatever level was drawn. Read coordinates off the picture and pass them back as `--region X,Y,W,H` to zoom in. `preview` then reads the coarsest pyramid level that still shows the region at `--max-size`, so zooming into any part of a whole-slide image reads only a few tiles. The JSON report maps picture pixels back to source pixels (`image.plot_area`, `source_origin`, `source_per_pixel`).

## Kinetic plates

A plate table with readings at several time points draws as one small growth curve per well, laid out in plate positions with row letters and column numbers. Every card shares the same time and value axes, stated under the grid. Curves take the colour of their group in the plate layout the file records (a SoftMax Pro group, a sample name), with a key at the top. When whole rows belong to one group, the group's name labels them at the left. Blank wells are drawn faint. A heat map of the last time point would hide a well that crashed and grew back; the curves show it.

## Annotations

`--annotate` (MCP: `annotations`) draws labelled marks on top of the picture, in the picture's own coordinates: full-resolution pixels on images (the numbers on the rulers), axis units on plots, wells on plates. Each annotation has a `type` and the fields that type needs:

| `type` | fields | draws |
| --- | --- | --- |
| `point` | `x`, `y` | a ring (images) or dot (plots) |
| `box` | `x`, `y`, `width`, `height` | a rectangle |
| `polygon`, `line` | `points: [[x, y], ...]` | a closed outline, an open line |
| `arrow` | `x`, `y` (tail), `x2`, `y2` (head) | an arrow |
| `text` | `x`, `y`, `label` | the label alone |
| `vline`, `hline` | `x` or `y` | a dashed line across the plot |
| `vband`, `hband` | `x`, `x2` or `y`, `y2` | a shaded band |
| `well` | `well` (`B3`) | a ring around a plate well |

All types take an optional `label`, and `color` (`accent`, the default warm red, `ink`, `muted`, `white` or `#RRGGBB`). On a trace with several channels, `channel` picks the panel (default the first). On a kinetic plate grid a mark goes on every card unless `well` names one, and its label joins the key. Labels are placed so they do not cover each other, on a paper-coloured pill over images.

```bash
openreadout preview slide.czi --region 30000,20000,4000,3000 -o fold.png \
  --annotate '[{"type":"point","x":31000,"y":21000,"label":"Purkinje cells"},
               {"type":"box","x":33000,"y":20300,"width":600,"height":500,"label":"molecular layer"}]'
openreadout preview growth.pda -o growth.png \
  --annotate '{"type":"vband","x":7.4,"x2":8.6,"label":"came back 7.4–8.6 h"}'
```

The JSON report lists each annotation as given, with `px` (its key points in picture pixels), `label_px` (where its label went) and `outside` (true when part of it lies outside what the picture shows). Plots also report `panels`: each panel's rectangle in picture pixels and the data values at its edges, so a viewer can map pixels back to data.

## JSON

[`preview`](../json/preview.md).

Run `openreadout preview --help` for the full help of your installed version.
