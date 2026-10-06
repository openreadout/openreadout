# Worked workflows

## 1. Summarize an unknown microscope file for a scientist

```bash
openreadout info run42.czi --view format --json   # {"data":{"format":"czi","confidence":"definite"}}
openreadout info run42.czi --json
```

Turn `data.images[]` into a sentence per image: "Scene B2: 486×486 px, 1 channel (EGFP, ex 488/em 509 nm), 4 z-slices, 3 timepoints, 0.10 µm/px, Plan-Apochromat 5x/0.35, acquired 2022-08-22." Mention `notes` verbatim if present.

Or let the tool write it: `openreadout info run42.czi --view explain --json` returns `summary` (one or two sentences), `paragraphs` (contents, channels, optics, timing, what is unusual), `caveats` (reader notes, missing calibration, local-time stamps, reader confidence) and `suggested_commands` (exports, selections, `check`, `info --view full --max-frames -1`), all derived from the `info` fields. Pass the caveats on; they are the things a scientist would otherwise assume.

## 2. Convert for analysis

```bash
openreadout export run42.czi -o run42.ome.tiff --json
```

Check `data.verified == true`. Then:

```python
import tifffile
arr = tifffile.imread("run42.ome.tiff")            # (T, Z, C, Y, X) or per-series
```

or `from bioio import BioImage; img = BioImage("run42.ome.tiff"); img.get_image_data("TCZYX")`.

Large file? Export one image and one timepoint: `--image 0 --select t=0`.

Chunked and multiscale instead (napari, web viewers, cloud storage):

```bash
openreadout export run42.czi --format ome-zarr -o run42.ome.zarr --json
```

```python
from bioio import BioImage
img = BioImage("run42.ome.zarr/0")       # multi-image files: one group per image (0, 1, ...); single image: "run42.ome.zarr"
img.get_image_dask_data("TCZYX")
```

## 3. Triage a folder after an instrument PC crash

```bash
openreadout check -r --jsonl --skip-unknown . > check.jsonl; echo "worst exit=$?"
```

One line per file (`path`, then the usual envelope). Exit 4 = at least one file is corrupt/truncated; read those lines' `data.findings[]` and report `missing_planes` counts. Exit 6 = readable format but a feature we do not decode (the `hint` says which); exit 3 = not an instrument file.

## 4. Feed metadata to an LLM without the vendor noise

`openreadout info FILE --view full --json` gives the normalized block plus provenance in a few KB. Add `--no-provenance` for the smallest payload. Only pull `vendor` when a specific vendor field is needed (it can be megabytes).

## 5. Use it as an MCP server

```bash
openreadout mcp --config claude      # prints the JSON for Claude Code / Claude Desktop
claude mcp add openreadout -- openreadout mcp
```

Tools: one per command, `openreadout_<command>`, and one per analysis (`analyze nmr-peaks` is `openreadout_nmr_peaks`); long flags become arguments with `-` as `_` (`--rt-range` is `rt_range`). `openreadout self doctor` lists them. Inputs and outputs are the same JSON as the CLI; each tool lists its output schema.

## 6. An NMR data set from the spectrometer PC

A Bruker data set is a directory of numbered experiments. Open one experiment:

```bash
openreadout info sucrose/13 --json      # traces[0] = fid (real/imag), traces[1] = pdata/1 (1r/1i)
openreadout trace sucrose/13 --trace 1                   # statistics, first samples, the ppm axis
openreadout export sucrose/13 --trace 1 -o c13.csv --json # columns chemical_shift_ppm,real,imag
```

Report `extra.nucleus`, `spectrometer_frequency_mhz`, `pulse_program`, `solvent`, `temperature_k`, `scans` and `acquired_at` of trace 0. In Python: `pandas.read_csv("c13.csv")`, or `openreadout.File("sucrose/13").read_trace(1, channel="real")` with `trace_axis(1)` for the ppm values. A 2D `ser` is one sweep per increment (`--sweep S`). JCAMP-DX exports (`.jdx`, `.dx`) from TopSpin, MestReNova or spectral databases work the same way.

