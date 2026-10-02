"""CZI pyramid levels for the oracles (`gen.py`, `gen_regions.py`), from czifile (BSD-3-Clause).

A level is one downsampling factor f for a whole scene (docs/provenance/czi.md, 2026-09-24):

- its subblocks are the pyramid subblocks whose X ratio `size / stored_size` rounds to f in
  log2 (ZEN halves level by level, so a subblock's own ratio is off by its stored-size rounding:
  128.158 for a 1/128 tile); czifile keys levels by the exact ratios instead, which splits the
  coarse levels of large scans into one level per subblock;
- its pixel grid is pylibCZIrw's at `read(roi=scene, zoom=1/f)` (LGPL, observed as a black box):
  `floor(w / f) x floor(h / f)` pixels anchored at the scene's level-0 origin `(x0, y0)`, a
  subblock starting at level-0 `start` landing at `floor((start - x0) / f)`;
- its pixels are czifile's: the composite czifile makes of those subblocks (`CziImage`), moved
  into that grid when every subblock starts on it (`start` a multiple of f), else each subblock
  decoded by czifile and pasted at its grid position in tile (M) order, valid-pixel masks
  honoured, stored pixels unscaled (pylibCZIrw resamples a subblock whose ratio is not exactly f
  to its logical footprint; the level shows the stored pixels).
"""

from __future__ import annotations

import math
from typing import Any, Dict, List, Sequence, Tuple

import numpy as np


def level_groups(sc: Any) -> List[Tuple[int, List[Any]]]:
    """`(factor, directory entries)` of every pyramid level of a czifile scene, finest first.

    ZEN builds pyramids by a constant minification factor, 2 in most files and 3 in some ZEN 3.7
    slide scans (subblock ratios 3, 9, 27, 81; `zenodo12509122-PSR-LXR-KO-WT-1054`): the base is
    3 when the finest pyramid ratio rounds to a multiple of 3, else 2, and each subblock's ratio
    is snapped to the nearest power of that base. pylibCZIrw's `read(zoom=1/3^k)` returns the
    same `floor(w / f) x floor(h / f)` grid (observed as a black box, 2026-09-26)."""
    ratios = []
    for lv in sc.levels[1:]:
        for e in lv.directory_entries:
            i = e.dims.index("X")
            if e.stored_shape[i]:
                r = e.shape[i] / e.stored_shape[i]
                if r > 1.0:
                    ratios.append((r, e))
    if not ratios:
        return []
    base = 3 if round(min(r for r, _ in ratios)) % 3 == 0 else 2
    by: Dict[int, List[Any]] = {}
    for r, e in ratios:
        by.setdefault(base ** round(math.log(r, base)), []).append(e)
    return sorted(by.items())


def level_sizes(sc: Any, groups: Sequence[Tuple[int, List[Any]]]) -> List[Tuple[int, int]]:
    """`(width, height)` of each level of `groups`."""
    sizes = dict(sc.sizes)
    w0, h0 = sizes.get("X", 1), sizes.get("Y", 1)
    return [(max(1, w0 // fk), max(1, h0 // fk)) for fk, _ in groups]


class Level:
    """Rectangles of one pyramid level of a scene (see the module docstring)."""

    def __init__(self, f: Any, sc: Any, fk: int, entries: List[Any]) -> None:
        import czifile

        self.f, self.sc, self.fk, self.entries = f, sc, fk, entries
        self.bx, self.by = sc.bbox[0], sc.bbox[1]
        self.dims = list(sc.dims)
        self.start = dict(zip(sc.dims, sc.start))
        self.on_grid = all(dict(zip(e.dims, e.start))["X"] % fk == 0 and dict(zip(e.dims, e.start))["Y"] % fk == 0
                           for e in entries)
        self.image = czifile.CziImage(f, tuple(entries), name=f"level 1/{fk}")
        self._cache: Dict[Tuple[int, int, int], np.ndarray] = {}

    def sel(self, c: int, z: int, t: int) -> Dict[str, int]:
        """czifile selects by absolute coordinate: the dimension's start plus the index."""
        return {d: self.start.get(d, 0) + v for d, v in (("C", c), ("Z", z), ("T", t)) if d in self.dims}

    def read(self, c: int, z: int, t: int, x: int, y: int, w: int, h: int) -> np.ndarray:
        """Rectangle `(x, y, w, h)` of plane `(c, z, t)`, `(h, w[, s])`."""
        sel = self.sel(c, z, t)
        if not self.on_grid:
            return self._compose(sel, x, y, w, h)
        key = (c, z, t)
        if key not in self._cache:
            self._cache.clear()
            img = self.image
            sub = img(**{k: v for k, v in sel.items() if k in img.dims}) if sel else img
            arr = np.asarray(sub.asarray(maxworkers=1))
            self._cache[key] = arr[tuple(slice(None) if d in "YXS" else 0 for d in sub.dims)]
        arr = self._cache[key]
        # czifile's array starts at its subblocks' bounding box (`bbox`, level pixels); the
        # level grid at ceil(x0 / f).
        fk = self.fk
        ox = -(-self.bx // fk) - self.image.bbox[0]
        oy = -(-self.by // fk) - self.image.bbox[1]
        out = np.zeros((h, w) + arr.shape[2:], dtype=arr.dtype)
        ys0, xs0 = max(0, -(y + oy)), max(0, -(x + ox))
        ys1, xs1 = min(h, arr.shape[0] - (y + oy)), min(w, arr.shape[1] - (x + ox))
        if ys1 > ys0 and xs1 > xs0:
            out[ys0:ys1, xs0:xs1] = arr[y + oy + ys0:y + oy + ys1, x + ox + xs0:x + ox + xs1]
        return out

    def _compose(self, sel: Dict[str, int], x: int, y: int, w: int, h: int) -> np.ndarray:
        fk = self.fk
        chosen = []
        for e in self.entries:
            d = dict(zip(e.dims, e.start))
            if all(d.get(k, v) == v for k, v in sel.items()):
                chosen.append((d.get("M", 0), e.file_position, e, d))
        out = None
        for _, _, e, d in sorted(chosen, key=lambda t: (t[0], t[1])):
            seg = e.read_segment_data(self.f)
            a = np.asarray(seg.data())
            # `data()` returns RGB sample order for the Bgr pixel types, as czifile's composites.
            a = a[tuple(slice(None) if k in "YXS" else 0 for k in e.dims)]
            if out is None:
                out = np.zeros((h, w) + a.shape[2:], dtype=a.dtype)
            px = (d["X"] - self.bx) // fk - x
            py = (d["Y"] - self.by) // fk - y
            m = seg.mask()
            th, tw = a.shape[0], a.shape[1]
            y0, x0 = max(0, -py), max(0, -px)
            y1, x1 = min(th, h - py), min(tw, w - px)
            if y1 <= y0 or x1 <= x0:
                continue
            src = a[y0:y1, x0:x1]
            dst = out[py + y0:py + y1, px + x0:px + x1]
            if m is not None and m.shape == (th, tw):
                mm = m[y0:y1, x0:x1]
                dst[mm] = src[mm]
            else:
                dst[...] = src
        if out is None:
            out = np.zeros((h, w), dtype=self.sc.dtype)
        return out
