# Upgrading from 0.1

OpenReadout 0.2 renames several commands, MCP tools and flags so that the command line and the MCP server use the same names, and so that each command does one thing. The JSON output is `schema_version` 2. There are no aliases for the old names: a script or prompt written for 0.1 needs the changes below. The full rationale is in the [surface note](https://github.com/openreadout/openreadout/blob/main/docs/surface-2026-10.md).

## Commands

| 0.1 | 0.2 |
| --- | --- |
| `check --planes` | `planes` |
| `check A --against B` | `compare A B` |
| `check --report` | `report` |
| `spectra` (scan list) | `scans` |
| `spectra --scan N`, `--index I`, `--nth N` (one spectrum) | `spectrum --scan N`, `--spectrum I`, `--nth N` |
| `analyze assay wells`, `curve`, `dose-response`, `kinetics`, `growth`, `qc` | `analyze assay-wells`, `assay-curve`, `dose-response`, `kinetics`, `growth`, `assay-qc` |
| `export --attachment NAME` | `extract FILE NAME` |
| `batch summarize TABLE` | `summarize TABLE` |
| `search INDEX_DIR --health` | `health INDEX_DIR` |
| `search INDEX_DIR QUERY --export OUT` | `export-dataset INDEX_DIR QUERY OUT` |
| `index --health` | removed, use `health` |

`info`, `check`, `preview`, `stats`, `trace`, `table`, the other `analyze` subcommands, `export`, `batch`, `link`, `index`, `search`, `watch`, `self` and `mcp` keep their names.

## MCP tools

An MCP tool is now `openreadout_` plus the command name, with `-` as `_`, and its arguments are the command's long flags with `-` as `_`. The [MCP tools page](../reference/mcp.md) lists all of them.

| 0.1 | 0.2 |
| --- | --- |
| `openreadout_analyze` with `kind` | one tool per analysis: `openreadout_peaks`, `openreadout_chromatogram`, `openreadout_nmr_peaks`, `openreadout_ephys_features`, `openreadout_spikes`, `openreadout_qpcr`, `openreadout_gate` |
| `openreadout_analyze` with `kind: assay` and `analysis` | `openreadout_assay_wells`, `openreadout_assay_curve`, `openreadout_dose_response`, `openreadout_kinetics`, `openreadout_growth`, `openreadout_assay_qc` |
| `openreadout_check` with `against` | `openreadout_compare` |
| `openreadout_check` with `report` | `openreadout_report` |
| `openreadout_spectra` | `openreadout_scans` and `openreadout_spectrum` |
| `openreadout_export` with `attachment` | `openreadout_extract` |
| `openreadout_batch` with `measure: summarize` | `openreadout_summarize` |
| `openreadout_formats` | the `openreadout://formats` resource |
| (none) | `openreadout_health` |

The analysis options that 0.1 took in `options` are now arguments of each tool.

## Flags and arguments

| command | 0.1 | 0.2 (command line / MCP) |
| --- | --- | --- |
| `info` | vendor tree shown, `--no-vendor` to leave it out | left out, `--vendor` / `vendor` to include it |
| `info` | `--all-frames` | `--max-frames N` / `max_frames` |
| `stats` | planes listed, `--no-planes` to leave them out | left out, `--per plane` / `per: plane` to list them |
| `trace` | `--first` | `--first-sample` / `first_sample` |
| `table` | MCP `transform_parameters` | `--parameter` / `parameters` |
| `preview` | `--plain`, `--grid` | `--axes rulers\|grid\|none` / `axes` |
| `scans` | `--rt`, `--tol`, `--ppm`, `--filter` | `--rt-range`, `--precursor-tol`, `--precursor-ppm`, `--scan-filter` |
| `scans` | MCP `precursor_mz` | `--precursor` / `precursor` |
| `export` | `--to FORMAT` | `--format` / `format`. MCP also accepts `csv` now. |
| `batch` | MCP `inputs`, measure `spectra` | `paths`, measure `scans` |
| `watch` | MCP `stall_after_s` | `--stall-after` / `stall_after` (seconds) |
| `analyze nmr-peaks` | `--range A:B` | `--range-ppm` / `range_ppm` |
| `analyze ephys-features` | `--peak-threshold` | `--peak-threshold-mv` / `peak_threshold_mv` |
| `analyze spikes` | `--band` | `--band-hz` / `band_hz` |
| `analyze qpcr` | `--cq`, `--baseline START-END`, `--reference`, `--control`, `--undetermined-as` | `--compute-cq`, `--baseline-start` and `--baseline-end`, `--reference-target`, `--control-sample`, `--undetermined-cq` |
| plate assays | `--blank`, `--blank-subtraction` (MCP `blank`), `--positive`, `--negative`, `--empty`, `--wavelength` | `--blank-wells`, `--blank-subtraction` / `blank_subtraction`, `--positive-wells`, `--negative-wells`, `--empty-wells`, `--wavelength-nm` |
| `analyze growth` | `--threshold` | `--growth-threshold` |
| `analyze assay-curve`, `dose-response` | `--preview PNG` (MCP `plot`) | `--plot PNG` / `plot` |
| `info`, `stats`, `trace`, `table`, `batch` | `--samples`, `--layout` | `--sample-sheet` (the plate assays keep `--layout`) |

## JSON output

The envelope's `schema_version` is `"2"`. Besides the renames above, two defaults changed what a plain call returns: `info --view full` leaves out the vendor tree unless you add `--vendor`, and `stats` leaves out the per-plane list unless you ask for `--per plane`. LIF channels are named after their dye (`DAPI`, `ALEXA 488`) rather than their display colour. The schemas are on the [JSON reference](../reference/json.html) and in `openreadout self schema`.

## Plugins and integrations

The Claude Code plugin now installs from its own repository, openreadout/agent-plugins. Add that marketplace and install the plugin from it:

```text
/plugin marketplace add openreadout/agent-plugins
/plugin install openreadout@openreadout
```

Run `openreadout self skill --install all` again to replace an installed skill with the 0.2 version. The Python and R packages follow the command names. Python has `File.scans()` and `openreadout.summarize()`, and R has `openreadout_scans()`, `openreadout_spectrum()` and `openreadout_summarize()`.
