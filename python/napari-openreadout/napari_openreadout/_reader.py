"""The napari reader: every image of a file as a lazy (dask) image layer, multiscale when the file
stores a pyramid, one layer per channel with the channel's name and colour, physical scale in µm
(and seconds for time)."""

from __future__ import annotations

import os
from typing import Any, Callable, Dict, List, Optional, Sequence, Tuple, Union

import openreadout

__all__ = ["get_reader", "layer_data", "read"]

PathOrPaths = Union[str, Sequence[str]]
LayerData = Tuple[Any, Dict[str, Any], str]
ReaderFunction = Callable[[PathOrPaths], List[LayerData]]

#: napari colormap names the channel colours are mapped to (nearest in RGB).
_COLORMAPS: Dict[str, Tuple[int, int, int]] = {
    "red": (255, 0, 0),
    "green": (0, 255, 0),
    "blue": (0, 0, 255),
    "cyan": (0, 255, 255),
    "magenta": (255, 0, 255),
    "yellow": (255, 255, 0),
    "gray": (255, 255, 255),
}
_DEFAULT_ORDER = ["green", "magenta", "cyan", "red", "blue", "yellow", "gray"]


def _one_path(path: PathOrPaths) -> Optional[str]:
    if isinstance(path, (list, tuple)):
        return os.fspath(path[0]) if len(path) == 1 else None
    return os.fspath(path)


def get_reader(path: PathOrPaths) -> Optional[ReaderFunction]:
    """napari hook: a reader for ``path`` when OpenReadout recognises it by its signature and it
    holds images; ``None`` otherwise (napari then asks other plugins)."""
    p = _one_path(path)
    if p is None:
        return None
    try:
        det = openreadout.info(p, view="format")
    except (openreadout.OpenReadoutError, OSError):
        return None
    if det.get("confidence") == "extension-only":
        return None
    return read


def _colormap(color: Optional[str], position: int) -> str:
    if color and len(color.lstrip("#")) == 6:
        h = color.lstrip("#")
        try:
            rgb = tuple(int(h[i : i + 2], 16) for i in (0, 2, 4))
        except ValueError:
            rgb = None
        if rgb is not None:
            return min(
                _COLORMAPS,
                key=lambda n: sum((a - b) ** 2 for a, b in zip(_COLORMAPS[n], rgb, strict=True)),
            )
    return _DEFAULT_ORDER[position % len(_DEFAULT_ORDER)]


def layer_data(f: openreadout.File, image: int) -> LayerData:
    """One image of an open file as napari layer data ``(data, kwargs, "image")``.

    ``data`` is a list of dask arrays (full resolution first) for pyramidal images, else one
    dask array, dims ``T, C, Z, Y, X`` (plus ``S`` for RGB). Channels are split into layers
    (``channel_axis=1``) named after the file's channels; ``scale`` is ``(T s, Z µm, Y µm, X µm)``
    where the file records them (1 otherwise).
    """
    im = f.images[image]
    levels = f.levels(image)
    multiscale = len(levels) > 1
    data: Any = f.pyramid(image) if multiscale else f.to_dask(image)
    rgb = im["samples_per_pixel"] == 3
    ps = im.get("physical_size") or {}
    dt = im.get("time_increment_s") or 1.0
    tzyx = (
        float(dt),
        float(ps.get("z") or 1.0),
        float(ps.get("y") or 1.0),
        float(ps.get("x") or 1.0),
    )
    base = im.get("name") or os.path.basename(f.path)
    kwargs: Dict[str, Any] = {
        "multiscale": multiscale,
        "rgb": rgb,
        "metadata": {
            "openreadout": {
                "path": f.path,
                "format": f.format,
                "image": image,
                "dims": f.dims(image),
                "levels": levels,
            }
        },
    }
    size_c = im["size_c"]
    if size_c > 1:
        by_index = {ch["index"]: ch for ch in im.get("channels", [])}
        names = []
        cmaps = []
        for c in range(size_c):
            ch = by_index.get(c, {})
            names.append(f"{base} {ch.get('name') or f'channel {c}'}")
            cmaps.append(_colormap(ch.get("color"), c))
        kwargs.update(channel_axis=1, name=names, scale=tzyx, blending="additive")
        if not rgb:
            kwargs["colormap"] = cmaps
    else:
        kwargs.update(name=base, scale=(tzyx[0], 1.0, *tzyx[1:]))
    return (data, kwargs, "image")


def read(path: PathOrPaths) -> List[LayerData]:
    """Open ``path`` and return one layer (or one per channel) per image in the file. The file
    stays open while the lazy arrays reference it."""
    p = _one_path(path)
    if p is None:
        return []
    f = openreadout.File(p)
    return [layer_data(f, i) for i in range(len(f.images))]
