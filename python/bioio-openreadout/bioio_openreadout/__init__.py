"""bioio reader plugin for Zeiss CZI, Nikon ND2 and Leica LIF, backed by OpenReadout.

Installed alongside ``bioio``, it is discovered through the ``bioio.readers`` entry point::

    from bioio import BioImage
    import bioio_openreadout

    img = BioImage("run42.czi", reader=bioio_openreadout.Reader)
    img.dims, img.channel_names, img.physical_pixel_sizes
    stack = img.get_image_dask_data("ZYX", T=0, C=1).compute()
"""

from importlib.metadata import PackageNotFoundError, version

from .reader import Reader
from .reader_metadata import ReaderMetadata

try:
    __version__ = version("bioio-openreadout")
except PackageNotFoundError:  # pragma: no cover - running from a source checkout
    __version__ = "uninstalled"

__all__ = ["Reader", "ReaderMetadata", "__version__"]
