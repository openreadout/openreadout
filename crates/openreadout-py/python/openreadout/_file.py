"""The :class:`File` object: metadata, planes, whole images, lazy arrays and export."""

from __future__ import annotations

import io
import json
import os
from types import TracebackType
from typing import (
    IO,
    TYPE_CHECKING,
    Any,
    Dict,
    List,
    Optional,
    Sequence,
    Tuple,
    Type,
    Union,
    cast,
)

import numpy as np

from . import _native
from ._errors import UsageError
from ._types import CheckReport, Experiment, ExportReport, FileInfo, ImageInfo, LsEntry, Source

if TYPE_CHECKING:  # optional dependencies, imported lazily at call time
    import dask.array
    import pyarrow
    import xarray

__all__ = ["DIMS", "File", "PathLike", "Region", "Selection", "SourceLike"]

PathLike = Union[str, "os.PathLike[str]"]
"""Anything :func:`os.fspath` accepts."""

SourceLike = Union[str, "os.PathLike[str]", bytes, bytearray, memoryview, IO[bytes]]
"""What :class:`File` opens: a path, the file's bytes, or a binary file-like object with
``read`` and ``seek`` (an open file, :class:`io.BytesIO`, an fsspec/S3 file object, ...)."""

#: Name given to a buffer when the caller gives none (no extension: detection by content only).
DEFAULT_BUFFER_NAME = "memory"


def _fileobj_name(obj: Any) -> str:
    name = getattr(obj, "name", None)
    if isinstance(name, (str, os.PathLike)):
        base = os.path.basename(os.fspath(name))
        if base:
            return str(base)
    return "stream"


def _open_native(
    source: SourceLike, name: Optional[str]
) -> Tuple[_native.NativeFile, Optional[bytes]]:
    """The native handle for ``source``, and the bytes to pickle when it is a buffer."""
    if isinstance(source, (str, os.PathLike)):
        return _native.open(os.fspath(source)), None
    if isinstance(source, (bytes, bytearray, memoryview)):
        data = source if isinstance(source, bytes) else bytes(source)
        return _native.open_bytes(data, name or DEFAULT_BUFFER_NAME), data
    if isinstance(source, io.TextIOBase):
        raise TypeError("open the file in binary mode ('rb'): openreadout reads bytes, not text")
    if callable(getattr(source, "read", None)) and callable(getattr(source, "seek", None)):
        return _native.open_fileobj(source, name or _fileobj_name(source), None), None
    raise TypeError(
        "openreadout.File() takes a path, bytes, or a binary file object with read() and "
        f"seek(), not {type(source).__name__}"
    )


Selection = Union[int, slice, Sequence[int], range]
"""Index along one dimension in :meth:`File.get_image_dask_data`."""

Region = Tuple[int, int, int, int]
"""A rectangle of a plane, ``(x, y, width, height)`` in the pixel coordinates of the resolution
level it is read at (``x``/``y`` of the top-left pixel)."""

DIMS = "TCZYX"
"""Dimension order of every array this package returns. ``S`` (samples, e.g. RGB) is appended
for images with ``samples_per_pixel > 1``. This is the order bioio/aicsimageio use."""


def _check_nonnegative(**indices: int) -> None:
    for name, value in indices.items():
        if not isinstance(value, (int, np.integer)) or isinstance(value, bool):
            raise UsageError(f"{name} must be an int, got {type(value).__name__}")
        if value < 0:
            raise UsageError(f"{name}={value} is negative; indices are zero-based and non-negative")


def _plane_bytes_to_array(
    width: int, height: int, spp: int, dtype: str, data: np.ndarray
) -> np.ndarray:
    """Shape the plane buffer (a 1-D uint8 array of little-endian samples, handed over from the
    native reader without a copy) as ``(Y, X)`` or ``(Y, X, S)`` of ``dtype``: a view, so no
    copy on little-endian hosts; big-endian hosts get a native-order copy."""
    le = np.dtype(dtype).newbyteorder("<")
    shape: Tuple[int, ...] = (height, width, spp) if spp > 1 else (height, width)
    arr = data.view(le).reshape(shape)
    if not le.isnative:  # pragma: no cover - big-endian hosts only
        arr = arr.astype(np.dtype(dtype))
    return arr


def _check_region(region: Optional[Sequence[int]]) -> Optional[Region]:
    if region is None:
        return None
    try:
        x, y, w, h = (int(v) for v in region)
    except (TypeError, ValueError) as e:
        raise UsageError(f"region must be (x, y, width, height), got {region!r}") from e
    if min(x, y) < 0 or min(w, h) < 1:
        raise UsageError(f"region {region!r}: x and y must be >= 0, width and height >= 1")
    return (x, y, w, h)


class File:
    """An opened instrument file (Zeiss CZI, Nikon ND2, Leica LIF, FCS, or an electrophysiology
    recording: Axon ABF, Neuralynx, Blackrock, SpikeGLX, Intan, Plexon, NWB).

    Metadata comes from headers only and is cheap; pixels are decoded one plane at a time when
    asked for. Use it as a context manager to release the file handle deterministically::

        with openreadout.File("run42.czi") as f:
            print(f.info["images"][0]["size_x"])
            plane = f.read_plane(image=0, c=1, z=4)

    A ``File`` can be shared between threads (plane reads on one file serialize; the GIL is
    released while decoding) and pickled (the copy reopens the same path), so the lazy arrays
    from :meth:`to_dask` work with dask's threaded, process and distributed schedulers.

    A ``File`` can also be opened from the file's bytes or from a binary file-like object
    (``seek`` and ``read``: an open file, :class:`io.BytesIO`, an fsspec or S3 file object).
    Nothing is written to disk; a file object is read on demand through a block cache.
    Single-file formats read this way; a data set spread over several files or a directory
    (a Bruker ``.d``, a multi-file OME-TIFF) needs a path.

    Args:
        path: Path to the file, its bytes, or a binary file-like object. The format is detected
            from its signature, not its extension.
        name: For bytes and file objects, the file name to report (its extension helps detect
            formats without a signature). Defaults to the file object's ``name``, else
            ``"memory"``/``"stream"``.
        keep_open: With ``False``, no file handle stays open between calls: the metadata is read
            now and every read reopens the file (for hosts that must not hold descriptors, such
            as bioio's plugin contract). Slower on formats with large indexes.

    Raises:
        InstrumentFileNotFoundError: The path does not exist (also a ``FileNotFoundError``).
        UnknownFormatError: Not a supported format (also a ``ValueError``).
        CorruptFileError: The header is unreadable (also a ``RuntimeError``).
    """

    def __init__(
        self, path: SourceLike, *, name: Optional[str] = None, keep_open: bool = True
    ) -> None:
        self._n, self._buffer = _open_native(path, name)
        self._from_fileobj = self._buffer is None and not isinstance(path, (str, os.PathLike))
        self._info: Optional[FileInfo] = None
        self._keep_open = keep_open
        if not keep_open:
            self._info = json.loads(self._n.info_json())
            self._n.set_transient(True)

    # ----- lifecycle -------------------------------------------------------------------------

    def close(self) -> None:
        """Release the underlying file handle. Further reads raise ``ValueError``. Idempotent."""
        self._n.close()

    @property
    def closed(self) -> bool:
        """True after :meth:`close`."""
        return self._n.closed

    def __enter__(self) -> File:
        return self

    def __exit__(
        self,
        exc_type: Optional[Type[BaseException]],
        exc: Optional[BaseException],
        tb: Optional[TracebackType],
    ) -> None:
        self.close()

    def __getstate__(self) -> Dict[str, Any]:
        if self._from_fileobj:
            raise TypeError(
                "a File opened from a file object cannot be pickled; open it from a path or bytes"
            )
        if self._buffer is not None:
            return {"bytes": self._buffer, "name": self.path, "closed": self.closed}
        return {"path": self.path, "closed": self.closed, "keep_open": self._keep_open}

    def __setstate__(self, state: Dict[str, Any]) -> None:
        if "bytes" in state:
            self._n = _native.open_bytes(state["bytes"], state["name"])
            self._buffer = state["bytes"]
        else:
            self._n = _native.open(state["path"])
            self._buffer = None
        self._from_fileobj = False
        self._info = None
        self._keep_open = bool(state.get("keep_open", True))
        if not self._keep_open:
            self._info = json.loads(self._n.info_json())
            self._n.set_transient(True)
        if state.get("closed"):
            self._n.close()

    def __repr__(self) -> str:
        if self.closed:
            return f"<openreadout.File {self.path!r} format={self.format!r} (closed)>"
        return f"<openreadout.File {self.path!r} format={self.format!r} images={len(self.images)}>"

    # ----- metadata --------------------------------------------------------------------------

    @property
    def path(self) -> str:
        """The path this file was opened from (for bytes and file objects, the name given)."""
        return self._n.path

    @property
    def format(self) -> str:
        """Format id, such as ``"czi"``, ``"nd2"`` or ``"fcs"``; ``openreadout.formats()`` lists
        them all."""
        return self._n.format

    @property
    def info(self) -> FileInfo:
        """The normalized summary (``FileInfo``), identical to ``openreadout info --json``.

        Read from headers once and cached.
        """
        if self._info is None:
            self._info = json.loads(self._n.info_json())
        return self._info

    @property
    def plate(self) -> Optional[Dict[str, Any]]:
        """The plate layout of a high-content screening plate (Harmony, ImageXpress,
        CellVoyager, OME-Zarr plate): ``id``, ``plate_type``, ``rows``, ``columns``, ``wells``
        (each with the image indices of its fields), ``planes_missing`` and ``complete``.
        ``None`` for other files. Same as ``info["plate"]``."""
        return cast(Optional[Dict[str, Any]], self.info.get("plate"))

    def stats(
        self,
        image: Optional[int] = None,
        select: Sequence[str] = (),
        level: int = 0,
        region: Optional[Tuple[int, int, int, int]] = None,
        bins: int = 0,
        log_bins: bool = False,
        per_plane: bool = True,
        mip: Optional[str] = None,
        per: Optional[str] = None,
        wells: Sequence[str] = (),
    ) -> Dict[str, Any]:
        """Pixel statistics, the same JSON as ``openreadout stats --json`` (its ``data``):
        ``planes`` (per plane; omitted with ``per_plane=False``), ``channels`` (per image ×
        channel, with ``name`` and ``image_name``) and ``images`` (per image, with ``name``);
        each holds ``stats`` = count, min, max, mean, std, percentiles (p1, p5, p50, p95, p99),
        zero and saturated fractions. ``select`` takes selection strings (``"c=0"``,
        ``"z=2-5"``), ``region`` an ``(x, y, width, height)`` rectangle at pyramid ``level``,
        ``bins`` > 0 adds histograms (``log_bins`` for geometric bins), ``mip="z"`` (or
        ``"t"``) computes the statistics of maximum-intensity projections.

        ``per="well"`` (``stats --per well``) gives per-well statistics of a plate instead: tidy
        ``rows`` keyed by well (``well``, ``c``, ``channel``, ``mean``, ``std``, ``min``,
        ``max``, ``median``, ``p1``, ``p99``, ``planes``, ``planes_missing``, ...), one per
        (well, channel), or per (well, field, channel) with ``per="field"``. ``select`` and
        ``wells`` (well names such as ``"C05"``) narrow them. Planes never acquired are left
        out; missing plane files are skipped and counted."""
        if per is not None:
            if per not in ("well", "field"):
                raise UsageError(f"per must be 'well' or 'field', not {per!r}")
            rows: Dict[str, Any] = json.loads(
                self._n.well_stats_json(list(select), list(wells), per == "field")
            )
            return rows
        if wells:
            raise UsageError("wells= needs per='well' or per='field'")
        _check_nonnegative(level=level, bins=bins)
        if image is not None:
            _check_nonnegative(image=image)
        if mip is not None and mip not in ("z", "t"):
            raise UsageError(f"mip must be 'z' or 't', not {mip!r}")
        out: Dict[str, Any] = json.loads(
            self._n.stats_json(
                image,
                list(select),
                level,
                None if region is None else (region[0], region[1], region[2], region[3]),
                bins,
                log_bins,
                per_plane,
                mip,
            )
        )
        return out

    @property
    def experiment(self) -> Optional[Experiment]:
        """The experiment the file records: sample (id and the field it came from), instrument,
        method (name, technique term, parameters with UCUM units), acquisition (start, operator,
        duration) and measurements in scientific words, each value with its provenance.
        Shorthand for ``info.get("experiment")``; ``None`` when nothing is known."""
        return self.info.get("experiment")

    @property
    def images(self) -> List[ImageInfo]:
        """One entry per image (scene, series or position); shorthand for ``info["images"]``."""
        return self.info["images"]

    @property
    def tables(self) -> List[Dict[str, Any]]:
        """Tabular datasets (FCS data sets): ``row_count`` and ``columns[]`` with ``name`` ($PnN)
        and ``label`` ($PnS); shorthand for ``info["tables"]`` (empty for image formats)."""
        return self.info.get("tables", [])

    def read_table(
        self, table: int = 0, first_row: int = 0, max_rows: Optional[int] = None
    ) -> np.ndarray:
        """Rows of a table as a float64 array shaped ``(rows, columns)``.

        Values are raw: not compensated and not scaled by ``$PnE``/``$PnG``; FCS integer data is
        masked per ``$PnR``. ``max_rows=None`` reads to the end of the table.
        """
        _check_nonnegative(table=table, first_row=first_row)
        if max_rows is not None:
            _check_nonnegative(max_rows=max_rows)
        rows, cols, data = self._n.read_table(table, first_row, max_rows)
        # column-major from the reader: one copy into a C-contiguous (rows, columns) array
        return np.ascontiguousarray(data.reshape((cols, rows)).T, dtype=np.float64)

    @property
    def traces(self) -> List[Dict[str, Any]]:
        """Sampled-signal blocks and spectra (electrophysiology sweeps, NMR FIDs and spectra,
        IR/Raman spectra; one sweep per spectrum of a group or map): ``sweep_count``,
        ``sample_count`` (samples per sweep), ``sample_rate_hz`` and ``channels[]`` with ``name``,
        ``unit``, ``scale`` and ``offset``. Shorthand for ``info["traces"]`` (empty for image and
        table formats)."""
        return self.info.get("traces", [])

    def read_trace(
        self,
        trace: int = 0,
        sweep: int = 0,
        first_sample: int = 0,
        max_samples: Optional[int] = None,
        channel: Optional[Union[int, str]] = None,
    ) -> np.ndarray:
        """Samples of one sweep as a float64 array shaped ``(channels, samples)``, or
        ``(samples,)`` when ``channel`` (an index or a name such as ``"real"``) is given.

        Values are scaled to each channel's physical unit (``traces[t]["channels"][c]["unit"]``,
        e.g. pA or mV; Bruker NMR values include 2^NC / 2^NC_proc, JCAMP-DX values their
        factors). ``max_samples=None`` reads to the end of the sweep; the time of sample ``i``
        relative to the sweep start is ``(first_sample + i) / sample_rate_hz``, and spectra have
        their abscissa in :meth:`trace_axis`.
        """
        _check_nonnegative(trace=trace, sweep=sweep, first_sample=first_sample)
        if max_samples is not None:
            _check_nonnegative(max_samples=max_samples)
        chans, n, data = self._n.read_trace(trace, sweep, first_sample, max_samples)
        arr: np.ndarray = data.reshape((chans, n))  # channel-major already: no copy
        if channel is None:
            return arr
        row: np.ndarray = arr[self._channel_index(trace, channel)]
        return row

    def trace_axis(
        self, trace: int = 0, first_sample: int = 0, max_samples: Optional[int] = None
    ) -> Optional[np.ndarray]:
        """The abscissa of a trace (``extra["axis"]``: time in s for an FID, ppm for a processed
        spectrum, Hz, 1/cm, nm ... for JCAMP-DX and FT-IR spectra), or ``None`` when the trace has
        none. An unevenly spaced abscissa (``axis["irregular"]``: a Renishaw Raman-shift list, a
        peak table's ``x``) is read from its channel (``axis["channel"]``) of sweep 0."""
        _check_nonnegative(trace=trace, first_sample=first_sample)
        t = self._trace_info(trace)
        axis = t.get("extra", {}).get("axis") or {}
        if axis.get("irregular") and axis.get("channel") is not None:
            return self.read_trace(
                trace, 0, first_sample, max_samples, channel=int(axis["channel"])
            )
        if "first" not in axis or "step" not in axis:
            return None
        end = (
            t["sample_count"]
            if max_samples is None
            else min(t["sample_count"], first_sample + max_samples)
        )
        idx = np.arange(first_sample, max(end, first_sample), dtype=np.float64)
        return float(axis["first"]) + float(axis["step"]) * idx

    def _trace_info(self, trace: int) -> Dict[str, Any]:
        for t in self.traces:
            if t["index"] == trace:
                return t
        raise UsageError(f"trace {trace} not found (file has {len(self.traces)} traces)")

    def _channel_index(self, trace: int, channel: Union[int, str]) -> int:
        chans = self._trace_info(trace)["channels"]
        if isinstance(channel, (int, np.integer)) and not isinstance(channel, bool):
            if 0 <= int(channel) < len(chans):
                return int(channel)
        else:
            for c in chans:
                if str(c["name"]).lower() == str(channel).lower():
                    return int(c["index"])
        names = ", ".join(f"{c['index']} ({c['name']})" for c in chans)
        raise UsageError(f"channel {channel!r} not found; trace {trace} has {names}")

    @property
    def vendor(self) -> Any:
        """The vendor's own metadata tree converted to JSON, with names untouched."""
        return json.loads(self._n.vendor_json())

    @property
    def provenance(self) -> Dict[str, Source]:
        """Where each normalized field's meaning came from, keyed by JSON path into :attr:`info`."""
        return cast(Dict[str, Source], json.loads(self._n.provenance_json()))

    def entries(self) -> List[LsEntry]:
        """Structural listing of the container (the CLI's ``info --view structure``)."""
        return cast(List[LsEntry], json.loads(self._n.entries_json()))

    def check(self) -> CheckReport:
        """Validate the file's integrity. ``report["ok"]`` is False if it is corrupt or truncated.

        Unlike opening, this walks every block, so it can take a while on large files.
        """
        return cast(CheckReport, json.loads(self._n.check_json()))

    def ome_xml(self) -> str:
        """OME-XML (schema 2016-06) describing every image, with ``MetadataOnly`` pixels.

        Parse it with ``ome_types.from_xml`` if you want objects.
        """
        return self._n.ome_xml()

    def _image(self, image: int) -> ImageInfo:
        _check_nonnegative(image=image)
        images = self.images
        if image >= len(images):
            raise UsageError(
                f"image {image} is out of range; {self.path} has {len(images)} image(s)"
            )
        return images[image]

    def dims(self, image: int = 0) -> str:
        """Dimension order of arrays for ``image``: ``"TCZYX"`` or ``"TCZYXS"`` (RGB)."""
        return DIMS + ("S" if self._image(image)["samples_per_pixel"] > 1 else "")

    def levels(self, image: int = 0) -> List[Dict[str, Any]]:
        """Resolution levels of ``image``, full resolution first: ``level``, ``size_x``,
        ``size_y``, ``downsample_x``, ``downsample_y`` (level-0 size over this level's) and,
        for tiled images, ``tile_width``/``tile_height`` (the stored tile a region read aligns
        to), plus ``size_z`` where a level has fewer z planes (Imaris). One entry for images
        without a pyramid. The same list as ``info`` → ``images[i]["resolution_levels"]``."""
        im = self._image(image)
        levels = im.get("resolution_levels") or []
        if levels:
            return [dict(lv) for lv in levels]
        return [
            {
                "level": 0,
                "size_x": im["size_x"],
                "size_y": im["size_y"],
                "downsample_x": 1.0,
                "downsample_y": 1.0,
            }
        ]

    def _level(self, image: int, level: int) -> Dict[str, Any]:
        _check_nonnegative(level=level)
        levels = self.levels(image)
        for lv in levels:
            if lv["level"] == level:
                return lv
        raise UsageError(
            f"pyramid level {level} out of range for image {image} "
            f"({len(levels)} level{'s' if len(levels) > 1 else ''}: 0..{len(levels)})"
        )

    def shape(self, image: int = 0, level: int = 0) -> Tuple[int, ...]:
        """Shape of :meth:`read_image` for ``image`` at pyramid ``level``, in :meth:`dims`
        order."""
        im = self._image(image)
        lv = self._level(image, level)
        shape: Tuple[int, ...] = (
            im["size_t"],
            im["size_c"],
            lv.get("size_z") or im["size_z"],
            lv["size_y"],
            lv["size_x"],
        )
        if im["samples_per_pixel"] > 1:
            shape += (im["samples_per_pixel"],)
        return shape

    def dtype(self, image: int = 0) -> np.dtype:
        """NumPy dtype of ``image``'s samples."""
        return _numpy_dtype(self._image(image)["pixel_type"])

    # ----- pixels ----------------------------------------------------------------------------

    def read_plane(
        self,
        image: int = 0,
        c: int = 0,
        z: int = 0,
        t: int = 0,
        *,
        level: int = 0,
        region: Optional[Sequence[int]] = None,
    ) -> np.ndarray:
        """Decode one plane, or only the rectangle ``region = (x, y, width, height)`` of it, at
        pyramid ``level`` (0 = full resolution; :meth:`levels` lists the sizes).

        ``region`` is in the pixel coordinates of ``level``. Tiled formats (CZI subblocks, tiled
        and whole-slide TIFF, VSI, Imaris, OME-Zarr) decode only the tiles the region touches,
        so a window of a 40 GB slide costs a few tiles; this also reads regions of planes too
        large to assemble whole. Other formats read the plane and crop it. Mosaics are
        returned stitched.

        Returns:
            A writable, C-contiguous array shaped ``(Y, X)`` or ``(Y, X, S)`` for RGB, in the
            file's dtype (native byte order). Its buffer is the decoded plane itself, handed
            over by the reader without a copy, and is shared with nothing else.

        Raises:
            UsageError: An index is negative or out of range, or the region extends past the
                level (also a ``ValueError``).
            CorruptFileError: The plane's data is damaged.
            UnsupportedFeatureError: The plane uses a codec this build cannot decode, or is
                too large to read whole (read a region or a coarser level).
        """
        _check_nonnegative(image=image, c=c, z=z, t=t, level=level)
        w, h, spp, dtype, data = self._n.read_plane(image, c, z, t, level, _check_region(region))
        return _plane_bytes_to_array(w, h, spp, dtype, data)

    def read_image(self, image: int = 0, *, level: int = 0) -> np.ndarray:
        """Decode every plane of ``image`` at ``level`` into one array ``(T, C, Z, Y, X[, S])``.

        The array is allocated once and filled plane by plane. For large images prefer
        :meth:`to_dask` or :meth:`get_image_dask_data`, which read planes (or tiles) on demand.
        """
        shape = self.shape(image, level)
        out = np.empty(shape, dtype=self.dtype(image))
        for t in range(shape[0]):
            for c in range(shape[1]):
                for z in range(shape[2]):
                    out[t, c, z] = self._plane_view(image, c, z, t, out.shape[3:], level=level)
        return out

    def _plane_view(
        self,
        image: int,
        c: int,
        z: int,
        t: int,
        expect: Tuple[int, ...],
        *,
        level: int = 0,
        region: Optional[Region] = None,
    ) -> np.ndarray:
        w, h, spp, dtype, data = self._n.read_plane(image, c, z, t, level, region)
        view = _plane_bytes_to_array(w, h, spp, dtype, data)
        if view.shape != tuple(expect):
            from ._errors import CorruptFileError

            raise CorruptFileError(
                f"plane (image={image}, c={c}, z={z}, t={t}, level={level}) decoded as "
                f"{view.shape}, but the header declares {tuple(expect)}"
            )
        return view

    def to_dask(
        self,
        image: int = 0,
        *,
        level: int = 0,
        chunks: Union[str, Tuple[int, int], None] = None,
    ) -> dask.array.Array:
        """A lazy dask array over ``image`` at pyramid ``level``, shaped ``(T, C, Z, Y, X[, S])``.

        Each chunk is one (t, c, z) plane, split in Y and X along the file's own tiles for large
        tiled planes (``chunks=None``: tiles grouped into chunks of about 16 MiB; planes up to
        that size stay whole). ``chunks="plane"`` keeps whole planes; ``chunks=(cy, cx)`` sets
        the Y/X chunk size. Only the chunks a computation needs are decoded, and only the
        tiles under them. Requires the ``dask`` extra (``pip install 'openreadout[dask]'``).
        """
        from ._lazy import to_dask

        return to_dask(self, image, level=level, chunks=chunks)

    def pyramid(
        self, image: int = 0, *, chunks: Union[str, Tuple[int, int], None] = None
    ) -> List[dask.array.Array]:
        """Every resolution level of ``image`` as lazy dask arrays (full resolution first), e.g.
        for ``napari.view_image(f.pyramid(0), multiscale=True)``."""
        return [self.to_dask(image, level=lv["level"], chunks=chunks) for lv in self.levels(image)]

    def to_xarray(
        self, image: int = 0, *, level: int = 0, delayed: bool = True
    ) -> xarray.DataArray:
        """``image`` at pyramid ``level`` as an :class:`xarray.DataArray` with dims
        ``T, C, Z, Y, X[, S]``.

        Coordinates: channel names on ``C``; physical positions in µm on ``Z``/``Y``/``X`` and
        seconds on ``T`` when the file records a pixel size or time increment. At a
        downsampled level the positions are those of the level's pixel centres in the
        full-resolution frame (pixel ``i`` covers full-resolution pixels ``i·f .. (i+1)·f - 1``,
        centred at ``(i + 0.5)·f - 0.5``), so levels overlay each other. ``attrs`` holds the
        image's normalized metadata. With ``delayed=True`` (default) the data is a dask array
        read on demand; otherwise it is read now. Requires the ``xarray`` extra.
        """
        from ._lazy import to_xarray

        return to_xarray(self, image, level=level, delayed=delayed)

    def get_image_dask_data(
        self,
        dimension_order_out: Optional[str] = None,
        *,
        image: int = 0,
        level: int = 0,
        **selection: Selection,
    ) -> dask.array.Array:
        """A lazy, reordered and sub-selected view of ``image`` (bioio-style).

        Args:
            dimension_order_out: Output dims, a subset of :meth:`dims` in any order, e.g.
                ``"ZYX"`` or ``"CYX"``. Default: all dims in native order.
            image: Which image (scene) to read.
            level: Pyramid level (0 = full resolution).
            **selection: Per-dimension index: an int, a slice, or a sequence of ints, e.g.
                ``T=0, C=[0, 2], Z=slice(10, 20), Y=slice(0, 512)``. Dimensions left out of
                ``dimension_order_out`` and not selected take index 0.

        Example::

            f.get_image_dask_data("ZYX", image=1, T=0, C=2).compute()
        """
        from ._lazy import select_dims

        native = self.dims(image)
        return select_dims(self.to_dask(image, level=level), native, dimension_order_out, selection)

    def get_image_data(
        self,
        dimension_order_out: Optional[str] = None,
        *,
        image: int = 0,
        level: int = 0,
        **selection: Selection,
    ) -> np.ndarray:
        """Like :meth:`get_image_dask_data` but computed; only the needed planes (and tiles) are
        decoded."""
        lazy = self.get_image_dask_data(dimension_order_out, image=image, level=level, **selection)
        return np.asarray(lazy.compute(scheduler="synchronous"))  # type: ignore[no-untyped-call]

    # ----- export ----------------------------------------------------------------------------

    def export(self, output: PathLike, *, to: Optional[str] = None, **options: Any) -> ExportReport:
        """Export to an open format (``openreadout export``): a new file, written under a
        temporary name, read back and verified, then renamed into place; the source is never
        modified.

        ``to`` is ``"ome-tiff"``, ``"ome-zarr"``, ``"mzml"`` or ``"rdml"``; by default it is
        told from ``output`` (``.ome.tif[f]``, ``.zarr``, ``.mzML``, ``.rdml``). Tables, traces
        and spectra go to Arrow with :meth:`to_arrow` (then Parquet with pyarrow). The keyword
        options per format:

        - ``ome-tiff``: ``image``, ``select`` (``["c=0", "z=1-3"]``), ``compression``
          (``"deflate"``, ``"lzw"``, ``"none"``), ``overwrite``, ``embed_vendor``, ``level``,
          ``region`` (``(x, y, width, height)``), ``pyramid`` (``"auto"``, ``"source"``,
          ``"mean"``, ``"none"``), ``levels``, ``tile`` (a multiple of 16);
        - ``ome-zarr``: the same, with ``chunk`` (the y/x chunk edge) instead of ``tile`` and
          ``compression`` ``"deflate"`` (the Zarr gzip codec) or ``"none"``;
        - ``mzml``: ``run``, ``centroid`` (the instrument's centroid lists), ``overwrite``;
        - ``rdml`` (qPCR files): ``overwrite``.

        Returns the export report; ``report["verified"]`` is True when everything read back
        equal.
        """
        fmt = to if to is not None else _export_format(output)
        if fmt == "ome-tiff":
            return self._export_ome_tiff(output, **options)
        if fmt == "ome-zarr":
            return self._export_ome_zarr(output, **options)
        if fmt == "mzml":
            return cast(ExportReport, self._export_mzml(output, **options))
        if fmt == "rdml":
            overwrite = bool(options.pop("overwrite", False))
            if options:
                raise UsageError(f"export to rdml takes only overwrite=, not {sorted(options)}")
            self._need_path("export to rdml")
            report = _native.export_rdml_json(self.path, os.fspath(output), overwrite)
            return cast(ExportReport, json.loads(report))
        raise UsageError(
            f"export to {fmt!r} is not available here: use 'ome-tiff', 'ome-zarr', 'mzml' or "
            "'rdml' (tables, traces and spectra: File.to_arrow())"
        )

    def _export_ome_tiff(
        self,
        output: PathLike,
        *,
        image: Optional[int] = None,
        select: Optional[Sequence[str]] = None,
        compression: str = "deflate",
        overwrite: bool = False,
        embed_vendor: bool = False,
        level: int = 0,
        region: Optional[Sequence[int]] = None,
        pyramid: str = "auto",
        levels: Optional[int] = None,
        tile: int = 512,
    ) -> ExportReport:
        """OME-TIFF (BigTIFF, OME-XML 2016-06). ``pyramid="auto"`` copies the source's own
        pyramid when it has one and the whole image is exported from level 0; levels are
        written as ``SubIFDs``; pyramids, regions, levels and planes above 4 GiB are written
        tiled, streaming, in bounded memory."""
        if image is not None:
            self._image(image)
        report = json.loads(
            self._n.export_ome_tiff(
                os.fspath(output),
                image,
                list(select) if select is not None else None,
                compression,
                overwrite,
                embed_vendor,
                level,
                _check_region(region),
                pyramid,
                levels,
                tile,
            )
        )
        return cast(ExportReport, report)

    def _export_ome_zarr(
        self,
        output: PathLike,
        *,
        image: Optional[int] = None,
        select: Optional[Sequence[str]] = None,
        compression: str = "deflate",
        overwrite: bool = False,
        embed_vendor: bool = False,
        level: int = 0,
        region: Optional[Sequence[int]] = None,
        pyramid: str = "auto",
        levels: Optional[int] = None,
        chunk: int = 512,
    ) -> ExportReport:
        """OME-Zarr (OME-NGFF 0.5 on Zarr v3) with a multiscale pyramid. ``pyramid="auto"``
        copies the source's own levels when it has them, else computes 2 × 2 mean levels."""
        if image is not None:
            self._image(image)
        report = json.loads(
            self._n.export_ome_zarr(
                os.fspath(output),
                image,
                list(select) if select is not None else None,
                compression,
                overwrite,
                embed_vendor,
                level,
                _check_region(region),
                pyramid,
                levels,
                chunk,
            )
        )
        return cast(ExportReport, report)

    # ----- mass spectra ------------------------------------------------------------------------

    def spectra(
        self,
        *,
        run: int = 0,
        ms_level: Optional[int] = None,
        polarity: Optional[str] = None,
        rt_range: Optional[Tuple[float, float]] = None,
        precursor: Optional[float] = None,
        precursor_tol: Optional[float] = None,
        precursor_ppm: Optional[float] = None,
        charge: Optional[int] = None,
        activation: Optional[str] = None,
        scan_filter: Optional[str] = None,
        offset: int = 0,
        limit: Optional[int] = None,
    ) -> Dict[str, Any]:
        """Scan headers of a mass-spectrometry run, without decoding any peaks (``openreadout
        spectra``; one spectrum's arrays: :meth:`read_spectrum`). ``info["spectra"]`` lists the
        runs.

        Every scan passing the filters is counted (``matched``, ``ms_level_counts``,
        ``rt_range_s``); ``scans`` lists them from the ``offset``-th, at most ``limit`` (all by
        default), each with ``index``, ``scan_number``, ``ms_level``, ``rt_s`` (``None`` when the
        file states no retention time for the scan), ``polarity``,
        ``precursor_mz``, ``precursor_charge``, ``isolation_window_mz``, ``activation``,
        ``collision_energy``, ``scan_filter`` and the file's stored ``total_ion_current`` /
        ``base_peak_mz`` where it keeps them.

        Args:
            run: Run index for files with several runs.
            ms_level: Only scans of this MS level (2 = MS/MS).
            polarity: ``"positive"`` or ``"negative"``.
            rt_range: Retention-time window ``(start, end)`` in minutes.
            precursor: Only MS/MS scans with a precursor within ``precursor_tol`` (m/z,
                default 0.01) or ``precursor_ppm`` of this m/z.
            precursor_tol: Absolute precursor tolerance in m/z.
            precursor_ppm: Relative precursor tolerance in ppm.
            charge: Only precursors of this charge state.
            activation: ``"HCD"``, ``"CID"``, ... (case-insensitive).
            scan_filter: Only scans whose filter string contains this text.
            offset: Skip this many matching scans.
            limit: List at most this many (``0`` only counts).

        Returns:
            The ``ScanList`` document; ``pandas.DataFrame(f.spectra()["scans"])`` makes a table.
        """
        _check_nonnegative(run=run, offset=offset)
        if limit is not None:
            _check_nonnegative(limit=limit)
        filt: Dict[str, Any] = {}
        for key, val in (
            ("ms_level", ms_level),
            ("polarity", polarity.lower() if polarity else None),
            ("precursor_mz", precursor),
            ("precursor_tol_mz", precursor_tol),
            ("precursor_tol_ppm", precursor_ppm),
            ("charge", charge),
            ("activation", activation),
            ("filter_contains", scan_filter),
        ):
            if val is not None:
                filt[key] = val
        if rt_range is not None:
            start, end = rt_range
            filt["rt_min_s"] = float(start) * 60.0
            filt["rt_max_s"] = float(end) * 60.0
        out = self._n.scans_json(
            run, json.dumps(filt), offset, 2**64 - 1 if limit is None else limit
        )
        return cast(Dict[str, Any], json.loads(out))

    def read_spectrum(
        self,
        index: Optional[int] = None,
        *,
        scan: Optional[int] = None,
        run: int = 0,
        centroid: bool = False,
    ) -> Dict[str, Any]:
        """One mass spectrum (Thermo ``.raw`` and other mass-spectrometry formats).

        Args:
            index: Zero-based spectrum index. Give this or ``scan``.
            scan: Scan number as the instrument counts it (1-based in Thermo files).
            run: Run index for files with several runs.
            centroid: Return the instrument's stored centroid list instead of the profile when
                a scan has both.

        Returns:
            The ``Spectrum`` document (``scan_number``, ``ms_level``, ``rt_s`` (``None`` when
            the file states no retention time for the scan), ``polarity``,
            ``centroided``, ``precursor_mz``, ``precursor_charge``, ``scan_filter``,
            ``total_ion_current``) with ``mz`` as a float64 and ``intensity`` as a float32
            NumPy array.
        """
        if (index is None) == (scan is None):
            raise UsageError("give exactly one of index= or scan=")
        _check_nonnegative(run=run)
        if scan is not None:
            _check_nonnegative(scan=scan)
            # the reader's scan-number map when it has one, else contiguous numbering
            meta, mz, it = self._n.read_spectrum_scan(run, scan, centroid)
        else:
            assert index is not None
            _check_nonnegative(index=index)
            meta, mz, it = self._n.read_spectrum(run, index, centroid)
        out: Dict[str, Any] = json.loads(meta)
        out["mz"] = mz
        out["intensity"] = it
        return out

    def to_arrow(
        self,
        *,
        table: Optional[int] = None,
        trace: Optional[int] = None,
        sweep: Optional[int] = None,
        rows: Optional[Tuple[int, Optional[int]]] = None,
        spectra: bool = False,
        run: int = 0,
        centroid: bool = False,
        per_scan: bool = False,
        max_rows: Optional[int] = None,
    ) -> pyarrow.Table:
        """A table, trace or mass-spectrometry run as a :class:`pyarrow.Table` (needs ``pyarrow``).

        The columns and metadata are those of ``openreadout export --format parquet``:

        - tables (FCS events, spike/event tables, plate reads, peak tables): one column per
          table column in its stored type; plate ``well`` columns are dictionaries of well names;
        - traces: ``sweep``, the abscissa (``time_s``, or e.g. ``chemical_shift_ppm``) and one
          float64 column per channel in physical units; every sweep unless ``sweep`` is given;
        - spectra (``spectra=True``): one row per point (``scan, rt_s, ms_level, precursor_mz,
          mz, intensity``), or with ``per_scan=True`` one row per scan.

        With no selection the spectra of run 0 are returned for mass-spectrometry files, else
        table 0, else trace 0. ``rows=(first, last)`` (inclusive, ``last=None`` to the end)
        selects rows of a table or samples of each sweep. Field metadata carry ``unit``,
        ``label`` and ``openreadout.provenance``; the schema metadata carry
        ``openreadout.info`` (the ``info`` JSON), ``openreadout.source_format`` and
        ``openreadout.instrument``. The batches reach pyarrow through the Arrow C data
        interface, without a copy.
        """
        try:
            import pyarrow  # noqa: F401
        except ImportError as e:  # pragma: no cover - exercised without pyarrow only
            raise ImportError(
                "File.to_arrow() needs pyarrow: pip install pyarrow (or openreadout[arrow])"
            ) from e
        first, last = rows if rows is not None else (None, None)
        for name, v in (("table", table), ("trace", trace), ("sweep", sweep), ("run", run)):
            if v is not None:
                _check_nonnegative(**{name: v})
        return self._n.to_arrow(
            table, trace, sweep, first, last, spectra, run, centroid, per_scan, max_rows
        )

    def to_pandas(self, **kwargs: Any) -> Any:
        """:meth:`to_arrow` as a :class:`pandas.DataFrame` (needs ``pyarrow`` and ``pandas``);
        takes the same keyword arguments. Column units and provenance stay in the Arrow
        schema: use :meth:`to_arrow` when you need them."""
        return self.to_arrow(**kwargs).to_pandas()

    def _export_mzml(
        self,
        output: PathLike,
        *,
        run: int = 0,
        centroid: bool = False,
        overwrite: bool = False,
    ) -> Dict[str, Any]:
        """Indexed mzML 1.1.0 of the spectra of ``run`` (zlib, 64-bit m/z, 32-bit intensity),
        read back (XML, index offsets, SHA-1 checksum and every array) before the rename."""
        return cast(
            Dict[str, Any],
            json.loads(self._n.export_mzml(os.fspath(output), run, centroid, overwrite)),
        )

    def _need_path(self, what: str) -> None:
        if self._buffer is not None or self._from_fileobj:
            raise UsageError(f"{what} needs a file opened from a path")

    def analyze(self, kind: str, **options: Any) -> Dict[str, Any]:
        """An analysis of this file (``openreadout analyze KIND``), the same dictionary the CLI
        prints under ``data``. See :func:`openreadout.analyze` for the kinds and options; the
        kinds that open their files themselves (``qpcr``, ``assay``, ``gate``) need a file
        opened from a path.
        """
        if kind in _PATH_KINDS:
            self._need_path(f"analysis {kind!r}")
            return analyze_path(self.path, kind, options)
        out = cast(Dict[str, Any], json.loads(self._n.analyze_json(kind, _options_json(options))))
        return _arrays(kind, out)


#: Analyses that open their files themselves (by path) rather than one opened data set.
_PATH_KINDS = ("qpcr", "assay", "gate")


def _options_json(options: Dict[str, Any]) -> str:
    """The analysis options as JSON: ``from_`` is ``from`` (a Python keyword), paths are
    strings and ``None`` means not given."""
    clean: Dict[str, Any] = {}
    for k, v in options.items():
        if v is None:
            continue
        if isinstance(v, os.PathLike):
            v = os.fspath(v)
        clean[k[:-1] if k.endswith("_") else k] = v
    return json.dumps(clean)


def _arrays(kind: str, out: Dict[str, Any]) -> Dict[str, Any]:
    if kind == "chromatogram":
        for c in out.get("chromatograms", []):
            c["rt_min"] = np.asarray(c["rt_min"], dtype=np.float64)
            c["intensity"] = np.asarray(c["intensity"], dtype=np.float64)
    return out


def analyze_path(path: PathLike, kind: str, options: Dict[str, Any]) -> Dict[str, Any]:
    """:func:`openreadout.analyze` on a path."""
    out, png = _native.analyze_json(os.fspath(path), kind, _options_json(options))
    result = _arrays(kind, cast(Dict[str, Any], json.loads(out)))
    if options.get("plot"):
        result["plot_png"] = png
    return result


def _export_format(output: PathLike) -> str:
    name = os.fspath(output).lower().rstrip("/\\")
    if name.endswith((".ome.tif", ".ome.tiff", ".tif", ".tiff")):
        return "ome-tiff"
    if name.endswith(".zarr"):
        return "ome-zarr"
    if name.endswith(".mzml"):
        return "mzml"
    if name.endswith(".rdml"):
        return "rdml"
    raise UsageError(
        f"cannot tell the export format from {os.path.basename(name)!r}: pass "
        "to='ome-tiff', 'ome-zarr', 'mzml' or 'rdml'"
    )


_DTYPES = {
    "int8": "int8",
    "int16": "int16",
    "int32": "int32",
    "uint8": "uint8",
    "uint16": "uint16",
    "uint32": "uint32",
    "float": "float32",
    "double": "float64",
}


def _numpy_dtype(pixel_type: str) -> np.dtype:
    return np.dtype(_DTYPES[pixel_type])
