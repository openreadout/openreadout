# openreadout (Python)

Read raw lab-instrument files from Python without vendor software, Java or a C toolchain. The package wraps OpenReadout's Rust readers. It returns the same JSON as the `openreadout` command-line tool, and NumPy, dask or xarray arrays for pixels. The formats it reads are listed at <https://openreadout.github.io/openreadout/formats/index.html>.

```bash
pip install openreadout            # needs only NumPy
pip install 'openreadout[xarray]'  # adds dask and xarray for lazy, labelled arrays
```

One abi3 wheel per platform covers CPython 3.10 and newer on Linux (manylinux and musllinux, x86_64 and aarch64), macOS (x86_64 and arm64) and Windows (x64).

```python
import openreadout

meta = openreadout.info("run42.czi")        # header-only summary, a dict
meta["images"][0]["size_c"], meta["images"][0]["physical_size"]

with openreadout.File("run42.czi") as f:
    plane = f.read_plane(image=0, c=1, z=4)   # np.ndarray (Y, X), or (Y, X, 3) for RGB
    stack = f.read_image(0)                   # np.ndarray (T, C, Z, Y, X[, S])
    lazy = f.to_dask(0)                       # dask array, one chunk per plane, read on demand
    xarr = f.to_xarray(0)                     # labelled: channel names, µm and s coordinates
    f.check()["ok"]                           # integrity report
    f.export("run42.ome.tiff")                # verified OME-TIFF; the source is never modified
```

Tables, traces and spectra (flow cytometry, electrophysiology, chromatography, mass spectrometry) are read with `File.read_table`, `File.read_trace` and `File.read_spectrum`, and convert to Arrow or pandas with `File.to_arrow` and `File.to_pandas`.

Arrays use the dimension order `TCZYX`, with `S` added for interleaved RGB. `File` objects are thread-safe and picklable, so lazy arrays work with dask's threaded, multiprocessing and distributed schedulers.

Every error is an `openreadout.OpenReadoutError` and also the closest built-in exception (for example `FileNotFoundError` or `ValueError`). It carries the command-line tool's `code`, `exit_code` and a `hint`.

[`bioio-openreadout`](https://pypi.org/project/bioio-openreadout/) plugs these readers into `bioio.BioImage`, and [`napari-openreadout`](https://pypi.org/project/napari-openreadout/) into napari.

Guide and full API: <https://openreadout.github.io/openreadout/guides/python.html>. Source: <https://github.com/openreadout/openreadout>. Licensed MIT OR Apache-2.0. OpenReadout is not affiliated with any instrument vendor.
