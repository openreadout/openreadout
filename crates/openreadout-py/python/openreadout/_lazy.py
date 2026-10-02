"""Lazy (dask) and labelled (xarray) views. Both libraries are optional dependencies."""

from __future__ import annotations

import os
import zlib
from typing import TYPE_CHECKING, Any, Dict, List, Mapping, Optional, Tuple, Union, cast

import numpy as np

from ._errors import UsageError

if TYPE_CHECKING:
    import dask.array
    import xarray

    from ._file import File, Selection

__all__ = ["select_dims", "to_dask", "to_xarray"]


def _require(module: str, extra: str) -> Any:
    try:
        return __import__(module, fromlist=["_"])
    except ImportError as e:  # pragma: no cover - exercised only without the extra
        raise ImportError(
            f"{module} is required for this; install it with `pip install 'openreadout[{extra}]'`"
        ) from e


class _PlaneStack:
    """Array-like over one image at one pyramid level that decodes planes (or the part of a
    plane a chunk covers) on ``__getitem__``.

    ``dask.array.from_array`` slices it one chunk at a time: one (t, c, z) plane, possibly
    split in Y and X. A chunk smaller than the plane is read as a region, so only the tiles
    under it are decoded. Picklable: it carries the :class:`File`, which reopens its path when
    unpickled in another process.
    """

    def __init__(self, file: File, image: int, level: int = 0) -> None:
        self.file = file
        self.image = image
        self.level = level
        self.shape: Tuple[int, ...] = file.shape(image, level)
        self.dtype = file.dtype(image)
        self.ndim = len(self.shape)

    def __getitem__(self, key: Any) -> np.ndarray:
        if not isinstance(key, tuple):
            key = (key,)
        key = key + (slice(None),) * (self.ndim - len(key))
        tcz: List[range] = []
        for axis in range(3):
            k = key[axis]
            if isinstance(k, (int, np.integer)):
                tcz.append(range(int(k), int(k) + 1))
            elif isinstance(k, slice):
                tcz.append(range(*k.indices(self.shape[axis])))
            else:
                raise TypeError(f"unsupported index {k!r}")
        # Y and X: read the covering rectangle, apply any step afterwards.
        yx: List[Tuple[int, int, int]] = []
        for axis in (3, 4):
            k = key[axis]
            n = self.shape[axis]
            if isinstance(k, (int, np.integer)):
                i = int(k) % n if -n <= int(k) < n else int(k)
                yx.append((i, i + 1, 1))
            elif isinstance(k, slice):
                a, b, step = k.indices(n)
                if step < 0:
                    raise TypeError("negative steps are not supported")
                yx.append((a, max(a, b), step))
            else:
                raise TypeError(f"unsupported index {k!r}")
        (y0, y1, ys), (x0, x1, xs) = yx
        h, w = self.shape[3], self.shape[4]
        full = (y0, y1, x0, x1) == (0, h, 0, w)
        region = None if full else (x0, y0, x1 - x0, y1 - y0)
        plane_shape = (y1 - y0, x1 - x0, *self.shape[5:])
        out = np.empty((len(tcz[0]), len(tcz[1]), len(tcz[2]), *plane_shape), dtype=self.dtype)
        if y1 > y0 and x1 > x0:
            for i, t in enumerate(tcz[0]):
                for j, c in enumerate(tcz[1]):
                    for k, z in enumerate(tcz[2]):
                        out[i, j, k] = self.file._plane_view(
                            self.image, c, z, t, plane_shape, level=self.level, region=region
                        )
        # Drop axes indexed by an int, apply Y/X steps and ints, and any sample index.
        squeeze = tuple(
            0 if isinstance(key[a], (int, np.integer)) else slice(None) for a in range(3)
        )
        inplane: Tuple[Any, ...] = tuple(
            0 if isinstance(key[a], (int, np.integer)) else slice(None, None, st)
            for a, st in ((3, ys), (4, xs))
        )
        return out[squeeze + inplane + tuple(key[5:])]


def _token(file: File, image: int, level: int = 0, chunks: Any = None) -> str:
    from dask.base import tokenize

    buffer = getattr(file, "_buffer", None)
    stamp: Tuple[Any, ...]
    if buffer is not None:
        # Opened from bytes: the content identifies it (cheap checksum, stable across pickling).
        stamp = ("bytes", len(buffer), zlib.crc32(buffer))
    elif getattr(file, "_from_fileobj", False):
        # A file object has no stable identity outside this process.
        stamp = ("fileobj", id(file))
    else:
        try:
            st = os.stat(file.path)
            stamp = (st.st_size, st.st_mtime_ns)
        except OSError:
            stamp = ()
    parts: Tuple[Any, ...] = (os.path.abspath(file.path), stamp, image)
    if level or chunks is not None:
        parts += (level, chunks)
    return "openreadout-" + tokenize(*parts)


#: Target size of one dask chunk when a plane is split along its tiles.
CHUNK_BYTES = 16 << 20


def _round_to(n: int, step: int) -> int:
    return max(step, (n // step) * step)


def plane_chunks(
    file: File, image: int, level: int, chunks: Union[str, Tuple[int, int], None]
) -> Tuple[int, int]:
    """Y and X chunk sizes of :meth:`File.to_dask` (see its docstring)."""
    shape = file.shape(image, level)
    h, w = shape[3], shape[4]
    if chunks == "plane":
        return (h, w)
    if chunks is not None:
        if isinstance(chunks, str):
            raise UsageError(f"chunks must be None, 'plane' or (cy, cx), not {chunks!r}")
        cy, cx = (int(v) for v in chunks)
        if cy < 1 or cx < 1:
            raise UsageError(f"chunks {chunks!r}: both sizes must be at least 1")
        return (min(cy, h), min(cx, w))
    bpp = file.dtype(image).itemsize * (shape[5] if len(shape) > 5 else 1)
    if h * w * bpp <= CHUNK_BYTES:
        return (h, w)
    lv = file._level(image, level)
    tw = int(lv.get("tile_width") or 0) or 512
    th = int(lv.get("tile_height") or 0) or 512
    edge = int((CHUNK_BYTES / bpp) ** 0.5)
    cx = w if tw >= w else min(w, _round_to(edge, tw))
    cy = min(h, _round_to(max(1, CHUNK_BYTES // (cx * bpp)), th))
    return (cy, cx)


def to_dask(
    file: File,
    image: int,
    *,
    level: int = 0,
    chunks: Union[str, Tuple[int, int], None] = None,
) -> dask.array.Array:
    """See :meth:`openreadout.File.to_dask`."""
    da = _require("dask.array", "dask")
    stack = _PlaneStack(file, image, level)
    cy, cx = plane_chunks(file, image, level, chunks)
    key = chunks if chunks is None or isinstance(chunks, str) else tuple(chunks)
    return cast(
        "dask.array.Array",
        da.from_array(
            stack,
            chunks=(1, 1, 1, cy, cx, *stack.shape[5:]),
            name=_token(file, image, level, key),
            meta=np.empty((0,) * stack.ndim, dtype=stack.dtype),
            asarray=False,
            fancy=False,
            lock=False,
        ),
    )


def _coords(file: File, image: int, dims: str, level: int = 0) -> Dict[str, Any]:
    im = file._image(image)
    lv = file._level(image, level)
    shape = file.shape(image, level)
    coords: Dict[str, Any] = {}
    names = []
    by_index = {ch["index"]: ch for ch in im["channels"]}
    for c in range(im["size_c"]):
        ch = by_index.get(c)
        name = ch.get("name") if ch is not None else None
        names.append(name if name else f"Channel:{image}:{c}")
    coords["C"] = names
    ps: Mapping[str, Any] = im["physical_size"]
    fz = im["size_z"] / shape[2] if shape[2] else 1.0
    for dim, size_key, n, f in (
        ("Z", "z", shape[2], fz),
        ("Y", "y", shape[3], float(lv.get("downsample_y") or 1.0)),
        ("X", "x", shape[4], float(lv.get("downsample_x") or 1.0)),
    ):
        step = ps.get(size_key)
        if step:
            # Centre of level pixel i in full-resolution pixels: (i + 0.5) * f - 0.5.
            coords[dim] = ((np.arange(n) + 0.5) * f - 0.5) * float(step)
    dt = im.get("time_increment_s")
    if dt:
        coords["T"] = np.arange(im["size_t"]) * float(dt)
    if "S" in dims and im["samples_per_pixel"] == 3:
        coords["S"] = ["R", "G", "B"]
    return coords


def to_xarray(file: File, image: int, *, level: int = 0, delayed: bool = True) -> xarray.DataArray:
    """See :meth:`openreadout.File.to_xarray`."""
    xr = _require("xarray", "xarray")
    dims = file.dims(image)
    data: Union[np.ndarray, dask.array.Array] = (
        to_dask(file, image, level=level) if delayed else file.read_image(image, level=level)
    )
    im = file._image(image)
    return cast(
        "xarray.DataArray",
        xr.DataArray(
            data,
            dims=list(dims),
            coords=_coords(file, image, dims, level),
            name=im.get("name") or f"Image:{image}",
            attrs={
                "openreadout": {
                    "path": file.path,
                    "format": file.format,
                    "image": im,
                    "level": level,
                },
                "units": {"Z": "µm", "Y": "µm", "X": "µm", "T": "s"},
            },
        ),
    )


def select_dims(
    arr: dask.array.Array,
    native: str,
    order_out: Optional[str],
    selection: Mapping[str, Selection],
) -> dask.array.Array:
    """Index ``arr`` (dims ``native``) by ``selection`` and transpose to ``order_out``."""
    order = native if order_out is None else order_out.upper()
    if len(set(order)) != len(order) or not set(order) <= set(native):
        raise UsageError(
            f"dimension_order_out={order_out!r} must use each of {native!r} at most once"
        )
    unknown = set(selection) - set(native)
    if unknown:
        raise UsageError(f"unknown dimension(s) {sorted(unknown)}; this image has {native!r}")
    kept: List[str] = []
    for dim in native:
        axis = len(kept)
        size = arr.shape[axis]
        sel: Any = selection.get(dim)
        if sel is None:
            sel = slice(None) if dim in order else 0
        if isinstance(sel, (int, np.integer)):
            idx = int(sel)
            if not -size <= idx < size:
                raise UsageError(f"{dim}={idx} is out of range for size {size}")
            if dim in order:
                arr = arr[(slice(None),) * axis + (slice(idx % size, idx % size + 1),)]
                kept.append(dim)
            else:
                arr = arr[(slice(None),) * axis + (idx,)]
            continue
        if dim not in order:
            raise UsageError(
                f"{dim} is selected with a range/list but is not in dimension_order_out={order!r}"
            )
        if not isinstance(sel, slice):
            sel = [int(i) for i in sel]
            if any(not -size <= i < size for i in sel):
                raise UsageError(f"{dim}={sel} has an index out of range for size {size}")
        arr = arr[(slice(None),) * axis + (sel,)]
        kept.append(dim)
    out = arr.transpose([kept.index(d) for d in order])  # type: ignore[no-untyped-call]
    return cast("dask.array.Array", out)
