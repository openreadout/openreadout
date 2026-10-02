# Load planes in Python

Use this when you want pixels, table rows or trace samples as NumPy arrays in your own script or notebook. The `openreadout` package reads files with the same Rust code as the command line, so it gives the same numbers, and it needs no vendor software and no Java.

## Run it

```bash
pip install openreadout
```

Until the first release is on PyPI, install it from a checkout of the repository with `pip install .` (this needs a Rust toolchain). The package does not install the `openreadout` command; for that, see [Install](../getting-started/install.md).

```python
import openreadout

with openreadout.File("mini.nd2") as f:
    f.format                                    # 'nd2'
    len(f.images)                               # how many images (CZI scenes, ND2 positions, LIF series)
    f.images[0]["channels"][0]["name"]          # 'DAPI'
    f.dims(0), f.shape(0), f.dtype(0)           # ('TCZYX', (2, 1, 1, 8, 8), dtype('uint16'))
    plane = f.read_plane(image=0, c=0, z=0, t=1)    # NumPy array, shape (8, 8)
```

`mini.nd2` is a small Nikon file in the repository at [`crates/openreadout-cli/tests/fixtures/mini.nd2`](../../../crates/openreadout-cli/tests/fixtures/mini.nd2). The values in the comments are the ones the [Python guide](../guides/python.md) shows for it.

## What it tells you

- `openreadout.File` detects the format from the file's signature, not its extension. Use it as a context manager so the file handle is closed promptly.
- `f.images` is a list of dicts with the same keys as `openreadout info --json`: `size_x`, `size_c`, `physical_size`, `channels`, `objective` and the rest. See [Reading the JSON output](../getting-started/reading-json.md).
- Every image array has the axes `T, C, Z, Y, X`, the same order bioio uses. Interleaved RGB images have a sixth axis, `S`.
- `read_plane` decodes one plane and returns an array shaped `(Y, X)`, or `(Y, X, S)` for RGB, in the file's data type. `read_image(i)` reads the whole image into one `(T, C, Z, Y, X)` array, so keep it for small images.
- Errors are exceptions derived from `openreadout.OpenReadoutError`, with the same `code`, `exit_code` and `hint` as the command line. A damaged plane raises `CorruptFileError`.

## Variations

### Tables and traces

FCS events, plate-reader values and spike tables are tables. Electrophysiology sweeps, chromatograms and spectra are traces. Both come back as `float64` arrays:

```python
with openreadout.File("tube1.fcs") as f:
    [c["name"] for c in f.tables[0]["columns"]]     # the $PnN parameter names
    events = f.read_table(0)                        # (events, parameters)

with openreadout.File("synthetic-timeseries.nwb") as f:
    y = f.read_trace(2)                             # (channels, samples): (2, 1000)
```

Table values are raw: not compensated and not scaled. Trace values are in each channel's unit, `f.traces[i]["channels"][c]["unit"]`. `read_trace` also takes `sweep=`, `first_sample=`, `max_samples=` and `channel=`.

### Large images

For anything that does not fit in memory, use a lazy array, which decodes planes only when they are needed. This needs `pip install 'openreadout[xarray]'`:

```python
with openreadout.File("run.czi") as f:
    lazy = f.to_dask(0)                                  # one chunk per plane
    stack = f.get_image_data("ZYX", image=0, T=0, C=1)   # decodes only those planes
    window = f.read_plane(0, region=(0, 0, 512, 512))    # x, y, width, height
```

`region=` works on whole-slide images and other pyramids too; tiled formats decode only the tiles it touches. `f.levels(0)` lists the pyramid levels, and `level=` picks one.

### bioio

If your code already uses `BioImage`, install the plugin instead:

```bash
pip install bioio bioio-openreadout
```

```python
from bioio import BioImage
import bioio_openreadout

img = BioImage("run42.nd2", reader=bioio_openreadout.Reader)
img.scenes, img.dims, img.channel_names, img.physical_pixel_sizes
stack = img.get_image_dask_data("ZYX", T=0, C=1).compute()
```

For `.czi`, `.nd2`, `.lif`, `.vsi`, `.svs`, `.ndpi`, `.qptiff`, `.ims`, `.oir`, `.oib`, `.oif` and `.zvi`, `BioImage(path)` finds the plugin without `reader=`. The plugin reads local files only.

### napari

```bash
pip install napari napari-openreadout
napari run.czi
```

Each image opens as its own layers, one per channel, scaled in micrometres and read lazily. Whole-slide images open as multiscale layers.

## More

- [Python](../guides/python.md): the whole package, including Arrow and pandas, analyses, export and reading from bytes.
- [napari](../guides/napari.md): the plugin, and building layers yourself.
- [Reading the JSON output](../getting-started/reading-json.md): the metadata keys `f.images` and `f.tables` use.
