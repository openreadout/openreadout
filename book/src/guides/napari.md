# napari

The `napari-openreadout` plugin opens microscopy files in [napari](https://napari.org) without converting them first.

```bash
pip install napari napari-openreadout
napari slide.svs
```

You can also use **File › Open** in napari, or `viewer.open(path, plugin="napari-openreadout")` from Python.

The plugin registers for the image formats OpenReadout reads, including CZI, ND2, LIF, Olympus OIR, OIB, OIF and VSI, whole-slide SVS, NDPI and QPTIFF, Imaris, OME-TIFF, OME-Zarr, MetaMorph, Hamamatsu DCIMG and the electron-microscopy formats MRC, DM3/DM4, SER and EMD.

What you get:

- Each image in the file (a CZI scene, an ND2 position, a LIF series) is its own layer.
- Each channel is a separate layer, named and coloured as in the file. RGB images stay one layer.
- Layers are scaled by the pixel size in micrometres (and the time step in seconds), so 3D views and the scale bar are correct.
- Layers are lazy dask arrays. napari decodes a plane, or for large tiled planes the tiles in view, only when it displays it.
- Files that store a pyramid, such as whole-slide images, open as multiscale layers.

Without napari, `napari_openreadout.read(path)` returns the same layer data.

If another installed plugin also claims an extension, napari asks which reader to use, or you can pass `plugin="napari-openreadout"`.

## From Python, without the plugin

The [Python package](python.md) gives you the same lazy arrays, if you want to build the layers yourself:

```python
import napari
import openreadout

f = openreadout.File("run.czi")             # keep it open while the viewer is up
im = f.images[0]
ps = im.get("physical_size", {})
viewer = napari.Viewer()
viewer.add_image(
    f.to_dask(0),                           # (T, C, Z, Y, X)
    channel_axis=1,
    name=[ch.get("name", f"c{ch['index']}") for ch in im.get("channels", [])] or None,
    scale=(1, ps.get("z", 1), ps.get("y", 1), ps.get("x", 1)),
)
napari.run()
```

For a pyramid, pass `f.pyramid(0)` with `multiscale=True` instead.

## From an export

An OME-TIFF or OME-Zarr export opens with napari's own readers or with plugins such as `napari-ome-zarr`. Use this when you want a file to hand to someone else:

```bash
openreadout export run.czi -o run.ome.tiff
```
