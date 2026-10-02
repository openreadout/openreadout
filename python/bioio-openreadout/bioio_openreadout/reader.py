"""The bioio ``Reader`` implemented on top of :mod:`openreadout`."""

from __future__ import annotations

import logging
from datetime import timedelta
from numbers import Integral
from typing import Any, Dict, List, Optional, Tuple

import numpy as np
import openreadout
import xarray as xr
from bioio_base import constants, exceptions, io, reader, types
from fsspec.implementations.local import LocalFileSystem  # type: ignore[import-untyped]
from fsspec.spec import AbstractFileSystem  # type: ignore[import-untyped]

__all__ = ["Reader"]

log = logging.getLogger(__name__)

_PLUGIN = "bioio-openreadout"


class Reader(reader.Reader):
    """Read Zeiss CZI, Nikon ND2, Leica LIF and the other microscopy formats of the clean-room
    OpenReadout core (whole-slide SVS/NDPI/QPTIFF/VSI, Imaris, ...).

    Each image in the file (CZI scene, ND2 position, LIF series) is a bioio scene. Data is
    ``TCZYX`` (``TCZYXS`` for RGB), read lazily: one plane per dask chunk, or the file's own
    tiles grouped into chunks of about 16 MiB for large tiled planes, so only what a
    computation touches is decoded. Mosaics are returned stitched. Pyramidal files expose their
    levels as bioio resolution levels (``resolution_levels``, ``set_resolution_level``); physical
    pixel sizes follow the level.

    Parameters
    ----------
    image : Path or str
        Path to a local file.
    fs_kwargs : Dict[str, Any]
        Passed to the fsspec filesystem. Only local files are supported.
    keep_open : bool
        Keep the file open between reads (faster on files with large indexes, e.g. whole-slide
        CZIs). Default ``False``: as bioio expects of plugins, no file handle stays open; every
        read reopens the file.

    Raises
    ------
    exceptions.UnsupportedFileFormatError
        The path is not a local CZI, ND2 or LIF file that OpenReadout can open.
    """

    NAME = _PLUGIN

    _file: Optional[openreadout.File] = None
    _scene_ids: Optional[Tuple[str, ...]] = None
    _ome: Any = None
    _vendor: Any = None

    @staticmethod
    def _is_supported_image(fs: AbstractFileSystem, path: str, **kwargs: Any) -> bool:
        if not isinstance(fs, LocalFileSystem):
            raise exceptions.UnsupportedFileFormatError(
                _PLUGIN, path, "OpenReadout reads local files only."
            )
        try:
            detected = openreadout.info(path, view="format")
        except openreadout.OpenReadoutError as e:
            raise exceptions.UnsupportedFileFormatError(_PLUGIN, path, str(e)) from e
        if detected["confidence"] == "extension-only":
            raise exceptions.UnsupportedFileFormatError(
                _PLUGIN, path, "The extension matches but the file signature does not."
            )
        return True

    def __init__(
        self,
        image: types.PathLike,
        fs_kwargs: Dict[str, Any] = {},
        keep_open: bool = False,
        **kwargs: Any,
    ):
        self._fs, self._path = io.pathlike_to_fs(image, enforce_exists=True, fs_kwargs=fs_kwargs)
        self._is_supported_image(self._fs, self._path)
        try:
            self._file = openreadout.File(self._path, keep_open=keep_open)
        except openreadout.OpenReadoutError as e:
            raise exceptions.UnsupportedFileFormatError(_PLUGIN, self._path, str(e)) from e

    @property
    def file(self) -> openreadout.File:
        """The underlying :class:`openreadout.File`."""
        assert self._file is not None
        return self._file

    @property
    def scenes(self) -> Tuple[str, ...]:
        """Image names from the file, made unique; ``Image:<n>`` where the file has none."""
        if self._scene_ids is None:
            ids: List[str] = []
            for i, im in enumerate(self.file.images):
                name = (im.get("name") or "").strip() or f"Image:{i}"
                if name in ids:
                    name = f"{name} ({i})"
                ids.append(name)
            self._scene_ids = tuple(ids)
        return self._scene_ids

    def _image(self) -> openreadout.ImageInfo:
        return self.file.images[self.current_scene_index]

    def _attach_metadata(self, xarr: xr.DataArray) -> xr.DataArray:
        xarr.attrs[constants.METADATA_UNPROCESSED] = self._vendor_metadata()
        try:
            xarr.attrs[constants.METADATA_PROCESSED] = self.ome_metadata
        except Exception as err:  # ome-types missing or a document it rejects
            log.debug("OME metadata unavailable: %s", err)
        return xarr

    @property
    def resolution_levels(self) -> Tuple[int, ...]:
        """Pyramid levels of the current scene (0 = full resolution), as stored in the file."""
        return tuple(lv["level"] for lv in self.file.levels(self.current_scene_index))

    def _level(self) -> int:
        return int(self.current_resolution_level)

    def _read_delayed(self) -> xr.DataArray:
        return self._attach_metadata(
            self.file.to_xarray(self.current_scene_index, level=self._level(), delayed=True)
        )

    def _read_immediate(self) -> xr.DataArray:
        return self._attach_metadata(
            self.file.to_xarray(self.current_scene_index, level=self._level(), delayed=False)
        )

    def _read_indexed(self, given_dims: str, dim_specs: List[Any]) -> np.ndarray:
        # Index the lazy array axis by axis (dask allows one list index at a time), so only
        # the planes (and tiles) the selection touches are decoded.
        arr = self.file.to_dask(self.current_scene_index, level=self._level())
        axis = 0
        for spec in dim_specs:
            if isinstance(spec, Integral):
                arr = arr[(slice(None),) * axis + (int(spec),)]
            else:
                arr = arr[(slice(None),) * axis + (spec,)]
                axis += 1
        return np.asarray(arr.compute())  # type: ignore[no-untyped-call]

    def _vendor_metadata(self) -> Any:
        if self._vendor is None:
            self._vendor = self.file.vendor
        return self._vendor

    @property
    def ome_metadata(self) -> Any:
        """OME model (``ome_types.OME``) built from OpenReadout's normalized metadata.

        One ``Image`` per scene; ``Pixels`` are ``MetadataOnly``. RGB images report
        ``SizeC = 3 × channels`` as OME requires.
        """
        if self._ome is None:
            from ome_types import from_xml

            self._ome = from_xml(self.file.ome_xml())
        return self._ome

    @property
    def physical_pixel_sizes(self) -> types.PhysicalPixelSizes:
        """Pixel sizes in µm for Z, Y, X at the current resolution level (``None`` where the
        file records none)."""
        im = self._image()
        ps = im["physical_size"]
        lv = self.file.levels(self.current_scene_index)[self._level()]
        fx, fy = float(lv.get("downsample_x") or 1.0), float(lv.get("downsample_y") or 1.0)
        z, y, x = ps.get("z"), ps.get("y"), ps.get("x")
        if z is not None and lv.get("size_z"):
            z = z * im["size_z"] / lv["size_z"]
        return types.PhysicalPixelSizes(
            z,
            y * fy if y is not None else None,
            x * fx if x is not None else None,
        )

    @property
    def channel_names(self) -> Optional[List[str]]:
        """Channel names; ``Channel:<scene>:<c>`` where the file records none."""
        im = self._image()
        by_index = {ch["index"]: ch for ch in im["channels"]}
        names = []
        for c in range(im["size_c"]):
            ch = by_index.get(c)
            name = ch.get("name") if ch is not None else None
            names.append(name if name else f"Channel:{self.current_scene_index}:{c}")
        return names

    @property
    def time_interval(self) -> types.TimeInterval:
        """Time between T frames, or ``None`` when the file records no increment."""
        dt = self._image().get("time_increment_s")
        return timedelta(seconds=dt) if dt else None
