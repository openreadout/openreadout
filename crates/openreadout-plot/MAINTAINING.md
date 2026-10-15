# Maintaining openreadout-plot

The figure behind `openreadout plot` and `openreadout_plot`. The user documentation is `book/src/reference/commands/plot.md`; the agent's guide is `skills/openreadout/references/figures.md`.

## Layout

| file | what it holds |
| --- | --- |
| `src/spec.rs` | The JSON spec: figure, panel, series, axis and annotation keys. It is the tool's input schema, so every doc comment is read by agents. Keep them short; enums whose values name themselves carry no docs. |
| `src/load/` | Reading series: `signal.rs` (traces, sweeps and spikes, chromatograms, mass spectra, f–I), `plate.rs` (wells, groups, blanks, colour rules), `flow.rs` (FCS events, transforms, populations), `assay.rs` (fits), `table.rs` (table files and inline values). |
| `src/model.rs` | Series in data units: geometry, quantities and units, callouts, colour class, facet, source. |
| `src/draw/` | `prep.rs` turns series into axes (time units, histogram bins, stacking, categories, ranges, peak picking, colours); `mod.rs` lays out plots, small multiples and plate grids, axes, legends, callouts and annotations; `marks.rs` draws one series. |
| `src/output.rs` | The JSON summary (`openreadout self schema plot`). |
| `src/pdf.rs` | A one-page vector PDF of a `Figure` (text as outlines, so no fonts are embedded). |
| `src/save.rs` | Saving any `Figure` (a plot, or a preview through `openreadout_export`): PNG, JPEG and TIFF at a pixel scale with the print resolution recorded, SVG and PDF sized for print, written through the verified write. |

## Rules

- Data comes from the readers and the analyses (`openreadout_batch::analyze`), never from a second parser. Numbers in the summary are the ones drawn.
- Every series says where it came from (`Source`): the file and what was read, or that it is inline.
- Errors are usage errors whose message lists the valid choices (groups, columns, parameters, traces, reads), since the generic hint cannot.
- Time axes are stored in seconds; `prep` picks the unit shown and converts. Annotations and `color_by` rules are in the unit shown.
- Peaks are picked again among the points the axes show, so zooming or clipping labels what is visible.

## Checking a change

`cargo nextest run -p openreadout-plot` covers the spec, plates, tables, inline values and output files with synthetic data. Look at real figures as well: the gallery specs used during development render every case from public corpus files in light and dark; look at each PNG at the size it is delivered (1200 px wide) before and after a change.
