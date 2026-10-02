"""Plugin metadata bioio reads through the ``bioio.readers`` entry point."""

from __future__ import annotations

from typing import List

import bioio_base.reader
import bioio_base.reader_metadata

__all__ = ["ReaderMetadata"]


class ReaderMetadata(bioio_base.reader_metadata.ReaderMetadata):
    """Extensions this plugin reads and the reader class that reads them."""

    @staticmethod
    def get_supported_extensions() -> List[str]:
        """File extensions routed to this plugin: the proprietary microscopy formats (other
        extensions, such as ``.tiff`` or ``.zarr``, are read with ``reader=`` as well, but left
        to their dedicated plugins by default)."""
        return [
            ".czi",
            ".nd2",
            ".lif",
            ".vsi",
            ".svs",
            ".ndpi",
            ".qptiff",
            ".ims",
            ".oir",
            ".oib",
            ".oif",
            ".zvi",
        ]

    @staticmethod
    def get_reader() -> bioio_base.reader.Reader:
        """The :class:`bioio_openreadout.Reader` class."""
        from .reader import Reader

        return Reader  # type: ignore[return-value]  # bioio's annotation says instance
