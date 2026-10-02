# napari-openreadout

A [napari](https://napari.org) reader plugin that opens raw microscopy and electron-microscopy files with [OpenReadout](https://github.com/openreadout/openreadout). The formats it reads are listed at <https://openreadout.github.io/openreadout/formats/index.html>.

```bash
pip install napari napari-openreadout
napari slide.svs
```

- **Lazy**: layers are dask arrays; a plane (or, for large tiled planes, the tiles under the
  view) is decoded only when displayed.
- **Multiscale**: pyramidal files (CZI pyramids, SVS/NDPI/QPTIFF, VSI, Imaris, OME-TIFF with
  SubIFDs, OME-Zarr multiscales) open as multiscale layers.
- **Channels** become separate layers named after the file's channels, coloured by their
  recorded colours; **scale** is the physical pixel size in µm (and the time step in s).
- Every image of the file (CZI scene, ND2 position, LIF series) is its own layer.

Without napari, `napari_openreadout.read(path)` returns the same `(data, kwargs, "image")` tuples.

Guide: <https://openreadout.github.io/openreadout/guides/napari.html>. Licensed MIT OR Apache-2.0.
