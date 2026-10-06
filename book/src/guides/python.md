# Python

Three Python packages are built on OpenReadout. All three read files with the same Rust code as the command line, so they give the same numbers.

- `openreadout` reads files directly: metadata as dicts, pixels as NumPy, dask or xarray arrays, tables and traces as NumPy or Arrow.
- `bioio-openreadout` is a reader plugin for [bioio](https://github.com/bioio-devs/bioio). Use it if your code already uses `BioImage`.
- `napari-openreadout` is a reader plugin for napari. See [napari](napari.md).

All three are MIT OR Apache-2.0 licensed and have no copyleft dependencies.

## Install

```bash
pip install openreadout                   # needs only NumPy
pip install 'openreadout[xarray]'         # adds dask and xarray, for lazy and labelled arrays
pip install 'openreadout[arrow]'          # adds pyarrow, for File.to_arrow
pip install bioio bioio-openreadout       # the bioio plugin
```

Wheels are built for CPython 3.10 and newer on Linux (x86_64 and aarch64, glibc and musl), macOS (x86_64 and arm64) and Windows (x64). On other platforms, pip builds from source, which needs a Rust toolchain.

The Python package does not install the `openreadout` command. For that, see [Install](../getting-started/install.md).

## The `openreadout` package

Open a file with `openreadout.File` and use it as a context manager. The metadata uses the same keys as `openreadout info --json` (see [Reading the JSON output](../getting-started/reading-json.md)):

```python
import openreadout

with openreadout.File("mini.nd2") as f:
    f.format                                    # 'nd2'
    f.images[0]["channels"][0]["name"]          # 'DAPI'
    f.dims(0), f.shape(0), f.dtype(0)           # ('TCZYX', (2, 1, 1, 8, 8), dtype('uint16'))
    plane = f.read_plane(image=0, c=0, z=0, t=1)    # NumPy array, shape (8, 8)
    f.experiment["measurements"][0]["what"]     # a sentence describing the measurement
    f.check()["ok"]                             # True when the file is intact
```

`openreadout.info(path)` returns the header metadata without keeping the file open. Like [`openreadout info --view`](../reference/commands/info.md), it takes a view: `view="full"` adds the vendor metadata tree and the provenance of each field, `view="structure"` lists the container's elements, `view="explain"` describes the file in sentences (`ask=` answers a question), and `view="format"` returns only the format.

The functions and methods are named after the CLI commands: `info`, `File.check()`, `File.stats()`, `File.spectra()`, `analyze`, `export`, `batch` and `link`. The `read_*` methods return NumPy arrays.

`mini.nd2` is a small file in the repository at [`crates/openreadout-cli/tests/fixtures/mini.nd2`](https://github.com/openreadout/openreadout/blob/main/crates/openreadout-cli/tests/fixtures/mini.nd2).

### Images

Every image array has the axes `T, C, Z, Y, X`, the same order bioio uses. Interleaved RGB images have a sixth axis, `S`, for the three samples. Each image in a file (a CZI scene, an ND2 position, a LIF series) is separate: pass `image=`. Images in one file can differ in shape and data type.

`read_image(i)` reads a whole image into memory. For anything large, use a lazy array, which decodes planes only when they are needed:

```python
with openreadout.File("run.czi") as f:
    lazy = f.to_dask(0)                                  # one chunk per plane
    mip = lazy[0, 0].max(axis=0).compute()               # max projection over z of t=0, c=0
    stack = f.get_image_data("ZYX", image=0, T=0, C=1)   # bioio-style selection, decodes only those planes
    xr = f.to_xarray(0)                                  # labelled: channel names, z/y/x in µm, t in s
```

Whole-slide images and other pyramids have resolution levels. `region=(x, y, width, height)` reads part of a plane, in the pixels of the chosen level. Tiled formats decode only the tiles the region touches:

```python
with openreadout.File("slide.svs") as f:
    f.levels(0)                                          # size and downsampling of each level
    thumb = f.read_image(0, level=2)
    window = f.read_plane(0, region=(22744, 16201, 512, 512))
    levels = f.pyramid(0)                                # one dask array per level, for napari
```

A `File` can be shared between threads, and reads of one file run in parallel, so dask's threaded scheduler decodes several chunks at once. A `File` pickles by its path, so lazy arrays also work with the multiprocessing and distributed schedulers.

### Tables

Flow-cytometry (FCS) events, plate-reader values, and spike or event tables are tables:

```python
with openreadout.File("tube1.fcs") as f:
    [c["name"] for c in f.tables[0]["columns"]]     # the $PnN parameter names
    events = f.read_table(0)                        # float64 array (events, parameters)
    df = f.to_pandas()                              # needs pyarrow
```

Values are raw. They are not compensated or scaled; the spillover matrix is in `f.tables[0]["extra"]["spillover"]`.

### Traces

Electrophysiology sweeps, chromatograms, NMR FIDs and spectra, and other 1-D signals are traces. Here is a small NWB file from the repository's test fixtures:

```python
with openreadout.File("synthetic-timeseries.nwb") as f:
    t = f.traces[2]
    t["name"], t["sample_rate_hz"]                  # ('voltage', 2000.0)
    [(c["name"], c["unit"]) for c in t["channels"]] # [('voltage[0]', 'volts'), ('voltage[1]', 'volts')]
    y = f.read_trace(2)                             # float64 array (channels, samples): (2, 1000)
```

`read_trace(trace, sweep=0, first_sample=0, max_samples=None, channel=None)` returns values in each channel's unit. `trace_axis(trace)` returns the x axis when the trace has one: retention time for a chromatogram, ppm for an NMR spectrum, wavenumber for an IR spectrum. It returns `None` for signals sampled at a fixed rate.

### Mass spectra

```python
with openreadout.File("sample.raw") as f:
    sp = f.read_spectrum(scan=1200)             # or index=1199; centroid=True for the stored centroids
    sp["mz"], sp["intensity"]                   # NumPy arrays
    sp["ms_level"], sp["rt_s"], sp["precursor_mz"]
    f.spectra(ms_level=2)                       # scan headers, without the peaks
    f.export("sample.mzML")                     # verified by reading it back
```

`rt_s` is `None` when the file gives no retention time for a scan.

### Analyses

`openreadout.analyze(path, kind, **options)` runs the analyses of [`openreadout analyze`](../reference/commands/analyze.md): `kind` is `"peaks"`, `"chromatogram"`, `"nmr-peaks"`, `"ephys-features"`, `"spikes"`, `"qpcr"`, `"assay"` or `"gate"`. It returns the same dict as the command's `--json` data. The keyword arguments are the options of the MCP tool `openreadout_analyze` for that kind (see [MCP tools](../reference/mcp.md)); write the option `from` as `from_=`. A dose-response fit on a test plate from the repository:

```python
r = openreadout.analyze("assay-synth-dose-response.csv", "assay", analysis="dose-response",
                        layout="assay-synth-dose-response-layout.csv")
for c in r["compounds"]:
    print(c["compound"], c["kind"], round(c["ec50"], 3))
# CPD-A IC50 0.235
# CPD-B IC50 1.825
# CPD-C IC50 4.281
```

More examples:

```python
openreadout.analyze("run.raw", "chromatogram", mz=[195.0877], ppm=5)  # rt_min and intensity as NumPy arrays
openreadout.analyze("run.D", "peaks", traces=[0], min_snr=10)
openreadout.analyze("nmr/1", "nmr-peaks", from_="fid", integrate=[(3.2, 3.9)])
openreadout.analyze("cell.abf", "ephys-features")
openreadout.analyze("plate.eds", "qpcr", ddcq=True)
with openreadout.File("run.raw") as f:
    f.analyze("peaks", mz=[195.0877], rt=0.6, window=0.3)
```

`File.analyze(kind, ...)` runs the analysis on an open file, including one opened from bytes; `qpcr`, `assay` and `gate` need a file opened from a path.

For the methods, see [Chromatograms and peaks](quantitation.md), [Plate-reader assays](plate-analysis.md), [NMR processing](nmr.md) and [Electrophysiology](ephys.md).

### Arrow and pandas

`File.to_arrow()` returns a `pyarrow.Table` with the same columns and metadata as `openreadout export --format parquet`, without writing a file. Without arguments it returns the spectra of a mass-spectrometry file, else table 0, else trace 0:

```python
with openreadout.File("cell.abf") as f:
    sweeps = f.to_arrow()                       # columns: sweep, time_s, one per channel
    one = f.to_arrow(sweep=3, rows=(0, 9999))   # one sweep, the first 10,000 samples
with openreadout.File("run.raw") as f:
    points = f.to_arrow(spectra=True)           # one row per m/z point
    per_scan = f.to_arrow(spectra=True, per_scan=True)
```

### Export

```python
with openreadout.File("run.czi") as f:
    f.export("out.ome.tiff", image=0, select=["c=0", "z=1-3"])
    f.export("out.ome.zarr")
openreadout.export("plate.eds", "plate.rdml")       # qPCR to RDML
```

`export(output, to=None, **options)` writes OME-TIFF, OME-Zarr, mzML or RDML; `to` is told from the output's extension unless given. Exports take the same options as [`openreadout export`](../reference/commands/export.md). Each export is written to a temporary file, read back and checked, then given its final name. An existing file is replaced only with `overwrite=True`.

### Bytes and file objects

`open`, `File`, `info` and `analyze` also take the file's bytes or a binary file object with `read` and `seek`, such as an open file, `io.BytesIO` or an fsspec file. This doesn't write anything to disk:

```python
import fsspec, openreadout

meta = openreadout.info(uploaded_bytes, name="run42.czi")   # the name's extension helps detection
with fsspec.open("s3://bucket/plate1.nd2", "rb") as fh, openreadout.open(fh) as f:
    f.images[0]["size_x"]                                    # reads only the blocks it needs
```

This works for formats stored in one file. Data sets spread over several files or a directory, such as Bruker `.d` or an NMR experiment, need a path on disk.

### Many files

`openreadout.batch(measure, inputs, ...)` runs one measurement over many files and returns a table, optionally joined to a sample sheet and summarized by group. `openreadout.link(paths)` groups files that measured the same sample. The options are the same as for [`openreadout batch`](../reference/commands/batch.md):

```python
res = openreadout.batch("table", ["runs/"], sample_sheets=["samples.csv"],
                        where=["parameter=FITC-A"], by=["condition"], values=["median"])
res.table          # pandas DataFrame: one row per file and parameter
res.summary        # n, mean, sd, sem, median, min, max, cv_percent per condition
```

See [Many files: batch tables and sample sheets](batch.md).

### Errors

Every exception derives from `openreadout.OpenReadoutError` and carries the same `code`, `exit_code` and `hint` as the command line's JSON errors. Each one is also the closest built-in exception, so generic handlers work:

| exception | also a | `code` | `exit_code` |
| --- | --- | --- | --- |
| `InstrumentFileNotFoundError` | `FileNotFoundError` | `io` | 5 |
| `InstrumentIOError` | `OSError` | `io` | 5 |
| `UnknownFormatError` | `ValueError` | `unknown_format` | 3 |
| `UsageError` | `ValueError` | `usage` | 2 |
| `CorruptFileError` | `RuntimeError` | `corrupt_file` | 4 |
| `UnsupportedFeatureError` | `NotImplementedError` | `unsupported_feature` | 6 |

```python
try:
    openreadout.File("notes.txt")
except openreadout.UnknownFormatError as e:
    print(e.code, e.exit_code, e.hint)
```

The package ships type hints. `FileInfo`, `ImageInfo`, `CheckReport` and the other result types are `TypedDict`s; at run time they are plain dicts.

## The bioio plugin

```python
from bioio import BioImage
import bioio_openreadout

img = BioImage("run42.nd2", reader=bioio_openreadout.Reader)
img.scenes, img.dims, img.channel_names, img.physical_pixel_sizes
img.set_scene(1)
stack = img.get_image_dask_data("ZYX", T=0, C=1).compute()
```

The plugin registers for `.czi`, `.nd2`, `.lif`, `.vsi`, `.svs`, `.ndpi`, `.qptiff`, `.ims`, `.oir`, `.oib`, `.oif` and `.zvi`, so `BioImage(path)` finds it without `reader=`. Any other image format OpenReadout reads opens with `reader=bioio_openreadout.Reader`. If another plugin for the same extension is installed (for example `bioio-nd2`), bioio may pick that one; pass `reader=` to choose.

Pyramids appear as bioio resolution levels (`img.resolution_levels`, `img.set_resolution_level(2)`). The plugin reads local files only, not fsspec URLs.

## Without the package

If you cannot install wheels, call the command line and parse its JSON:

```python
import json, subprocess

def info(path):
    p = subprocess.run(["openreadout", "info", path, "--json"], capture_output=True, text=True)
    env = json.loads(p.stdout)
    if not env["ok"]:
        raise RuntimeError(f"{env['error']['code']}: {env['error']['message']} ({env['error'].get('hint')})")
    return env["data"]
```

## Working on the package

To work on the bindings, run `maturin develop` from the repository root inside a virtual environment, then `python -m pytest`. Do not pass `--release`: the project selects a build profile in which a Rust bug raises a Python exception instead of ending the interpreter. [`docs/release-process.md`](https://github.com/openreadout/openreadout/blob/main/docs/release-process.md) describes how the wheels are built and published.
