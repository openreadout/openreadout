"""napari reader plugin for raw microscopy files, backed by OpenReadout.

Installed next to napari, it opens every image format OpenReadout reads (Zeiss CZI, Nikon ND2,
Leica LIF, whole-slide SVS/NDPI/QPTIFF/VSI, Imaris, OME-TIFF, OME-Zarr, ...) lazily: planes and
tiles are decoded only when napari displays them, and pyramidal files open as multiscale layers,
so a 40 GB slide opens in about a second::

    napari slide.svs   # or File > Open, or viewer.open(path, plugin="napari-openreadout")

Without napari, :func:`read` returns the same layer data for scripts.
"""

from importlib.metadata import PackageNotFoundError, version

from ._reader import get_reader, layer_data, read

try:
    __version__ = version("napari-openreadout")
except PackageNotFoundError:  # pragma: no cover - running from a source checkout
    __version__ = "uninstalled"

__all__ = ["__version__", "get_reader", "layer_data", "read"]
