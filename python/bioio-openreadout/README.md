# bioio-openreadout

A [bioio](https://github.com/bioio-devs/bioio) reader plugin that reads microscopy files with [OpenReadout](https://github.com/openreadout/openreadout)'s Rust readers. It is licensed MIT OR Apache-2.0 and depends on no GPL plugin, vendor SDK or Java. Wheels need no compiler. Each plane is one dask chunk and is decoded only when a computation needs it.

```bash
pip install bioio bioio-openreadout
```

```python
from bioio import BioImage
import bioio_openreadout

img = BioImage("run42.czi", reader=bioio_openreadout.Reader)
img.scenes  # ('P2', 'P3', 'P1') — one per CZI scene / ND2 position / LIF series
img.dims  # <Dimensions [T: 1, C: 3, Z: 5, Y: 325, X: 475]>
img.channel_names  # ['EGFP', 'TaRFP', 'Bright']
img.physical_pixel_sizes  # PhysicalPixelSizes(Z=1.0, Y=1.083, X=1.083)  (µm)
img.set_scene(1)
zyx = img.get_image_dask_data("ZYX", T=0, C=1).compute()  # decodes 5 planes, not the file
img.ome_metadata  # ome_types.OME built from the normalized metadata
```

`BioImage(path)` without `reader=` also works for `.czi`, `.nd2`, `.lif`, `.vsi`, `.svs`, `.ndpi`, `.qptiff`, `.ims`, `.oir`, `.oib`, `.oif` and `.zvi`: the plugin registers these extensions through the `bioio.readers` entry point. Other formats OpenReadout reads, such as OME-TIFF or OME-Zarr, open with `reader=bioio_openreadout.Reader`. When another plugin for the same extension is installed (for example `bioio-nd2`), bioio's own ordering decides which is tried first; pass `reader=` to choose.

## What you get

| bioio | from OpenReadout |
|---|---|
| `scenes` | image names (`Image:<n>` when the file has none; duplicates are suffixed with the index) |
| `dims` | `TCZYX`, or `TCZYXS` for RGB (interleaved samples) |
| `channel_names` | channel names (`Channel:<scene>:<c>` when the file has none) |
| `physical_pixel_sizes` | µm, `None` where the file records no size |
| `time_interval` | the recorded time increment |
| `metadata` / `ome_metadata` | `ome_types.OME` (one `Image` per scene, `MetadataOnly` pixels) |
| `xarray_dask_data.attrs["unprocessed"]` | the vendor's own metadata tree as JSON, names untouched |

Mosaics are returned stitched (no `M` dimension); pyramidal files at full resolution only. Remote (fsspec) paths are not supported: the file must be local.

## Tests

```bash
pip install -e 'python/bioio-openreadout[test]'
OPENREADOUT_CORPUS_DIR=corpus/files pytest python/bioio-openreadout/tests
```

The tests compare `img.data` against `bioio-nd2`, `czifile` and `liffile` (all BSD) on files from the public test corpus and skip when those files or readers are absent.

Guide: <https://openreadout.github.io/openreadout/guides/python.html>. Part of [OpenReadout](https://github.com/openreadout/openreadout). Licensed MIT OR Apache-2.0. OpenReadout is not affiliated with any instrument vendor.
