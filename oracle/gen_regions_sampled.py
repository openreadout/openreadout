"""Sampled ground truth for whole-slide levels too large to read as one plane.

`planes` refuses a plane over 4 GiB, so the full resolution of the largest development slides
is never decoded whole and `gen_regions.py` checks only a few small rectangles of it. This script
reads 16 rectangles of 2048 x 2048 pixels of level 0 of each slide below, one per cell of a 4 x 4
grid, with an independent reader, and summarises them as
`gen_regions.py` does (xxh3-128, mean, std, min, max, 8 x 8 grid of cell means). Slides are
mostly empty glass, so each rectangle is centred on the tissue nearest its cell's centre, found
on a coarse level that the same reader reads whole. A cell without tissue takes tissue from
elsewhere on the slide. The region
corpus test (`crates/openreadout-corpus-tests/tests/regions.rs`) compares OpenReadout's
`read_region` with every rectangle.

Readers (third-party, run as black boxes), as in `gen_regions.py`:

  SVS, PhenoCycler   tifffile (BSD-3-Clause) `aszarr` sliced with zarr (MIT); JPEG 2000 tiles
                     decoded by imagecodecs (OpenJPEG)
  NDPI, MIRAX,       OpenSlide (LGPL) `read_region`, composited over the slide's background
  Philips TIFF       colour. tifffile reads NDPI level 0 only as one JPEG stripe per row band,
                     which is impractical at this size
  VSI                Bio-Formats 8.5.0 `bfconvert -series S -crop X,Y,W,H` (GPL)
  CZI                czifile (BSD-3-Clause), pylibCZIrw (LGPL) as the second reader

Output: `corpus/oracle/regions-sampled/<manifest id>.json`, in the format of
`corpus/oracle/regions/`.

    cd oracle && uv run python gen_regions_sampled.py [id ...]     # default: every slide present
"""

from __future__ import annotations

import json
import sys

import numpy as np
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Tuple

import gen_regions as g

OUT = g.ROOT / "corpus" / "oracle" / "regions-sampled"

# Rectangle edge and grid size.
EDGE = 2048
GRID = 4

# id: (path under the corpus directory, lossy, the reader's targets at level 0)
SLIDES: Dict[str, Tuple[str, bool, Callable[[Path], Tuple[str, List[g.Target]]]]] = {
    "openslide-aperio-cmu-1": ("openslide-aperio-CMU-1.svs", True,
                               lambda p: g.tiff_targets(p, [(0, 0)], lambda n: [0])),
    "openslide-aperio-cmu-1-jp2k-33005": ("openslide-aperio-CMU-1-JP2K-33005.svs", True,
                                          lambda p: g.tiff_targets(p, [(0, 0)], lambda n: [0])),
    "zenodo14243592-adhm-svs": ("zenodo14243592-adhm-svs.svs", True,
                                lambda p: g.tiff_targets(p, [(0, 0)], lambda n: [0])),
    "zenodo15757766-phenocycler-mask": ("zenodo15757766-phenocycler-mask.tif", False,
                                        lambda p: g.tiff_targets(p, [(0, 0)], lambda n: [0])),
    "openslide-hamamatsu-cmu-1": ("openslide-hamamatsu-CMU-1.ndpi", True,
                                  lambda p: g.openslide_targets(p, lambda n: [0])),
    "openslide-philips-1": ("openslide-Philips-1.tiff", True,
                            lambda p: g.openslide_targets(p, lambda n: [0])),
    "openslide-philips-4": ("openslide-Philips-4.tiff", True,
                            lambda p: g.openslide_targets(p, lambda n: [0])),
    "openslide-mirax-cmu-1-saved-1-2": ("openslide-mirax-cmu-1-saved-1-2-zip/CMU-1-Saved-1_2.mrxs", True,
                                        lambda p: g.mirax_targets(p, lambda n: [0])),
    "openslide-mirax-cmu-1": ("openslide-mirax-cmu-1-zip/CMU-1.mrxs", True,
                              lambda p: g.mirax_targets(p, lambda n: [0])),
    "openslide-mirax2-fluorescence-1": ("openslide-mirax2-fluorescence-1-zip/Mirax2-Fluorescence-1.mrxs", True,
                                        lambda p: g.mirax_targets(p, lambda n: [0])),
    "openslide-mirax2-fluorescence-2": ("openslide-mirax2-fluorescence-2-zip/Mirax2-Fluorescence-2.mrxs", True,
                                        lambda p: g.mirax_targets(p, lambda n: [0])),
    "openslide-mirax2-2-4-bmp": ("openslide-mirax2-2-4-bmp-zip/Mirax2.2-4-BMP.mrxs", True,
                                 lambda p: g.mirax_targets(p, lambda n: [0])),
    "openslide-mirax2-2-4-png": ("openslide-mirax2-2-4-png-zip/Mirax2.2-4-PNG.mrxs", True,
                                 lambda p: g.mirax_targets(p, lambda n: [0])),
    "bia2666-vsi-24B0759-t6": ("bia2666-vsi-24B0759/24B0759_T6_he_2024-11-15.vsi", True,
                               lambda p: g.vsi_targets(p, [(1, [0])])),
    "zenodo17453126-he-bone-vsi": ("zenodo17453126-he-bone/HE_Bone.vsi", True,
                                   lambda p: g.vsi_targets(p, [(2, [0]), (3, [0])])),
    "zenodo8161864-vs200-he-20x": ("zenodo8161864-vs200/Image_L2410_Part3_1_H+E_20X.vsi", True,
                                   lambda p: g.vsi_targets(p, [(2, [0])])),
    "zenodo10577621-Young-mouse": ("zenodo10577621-Young-mouse.czi", True,
                                   lambda p: g.czi_targets(p, [0], lambda n: [0])),
}


# Largest coarse level (pixels) read whole to find the tissue on.
COARSE_PIXELS = 4_000_000


def content_mask(a: np.ndarray) -> np.ndarray:
    """Pixels of a coarse level that differ from the background (the level's median)."""
    f = a.astype(np.float64)
    lum = f.mean(axis=2) if f.ndim == 3 else f
    bg = float(np.median(lum))
    span = float(lum.max() - lum.min())
    thr = 12.0 if a.dtype == np.uint8 else max(span * 0.05, 0.0)
    return np.abs(lum - bg) > thr


def sampled(w: int, h: int, mask: Optional[np.ndarray]) -> List[Tuple[int, int, int, int]]:
    """One EDGE x EDGE rectangle per cell of a GRID x GRID grid over a w x h level, centred on
    the `mask` pixel nearest the cell's centre (`mask` is a coarse level's content). A cell
    without content takes a content pixel elsewhere (a fixed random pick), and keeps its own
    centre when the level has no content at all."""
    rw, rh = min(EDGE, w), min(EDGE, h)
    out: List[Tuple[int, int, int, int]] = []
    spare: List[Tuple[int, int]] = []
    if mask is not None:
        ys, xs = np.nonzero(mask)
        if len(xs):
            rng = np.random.default_rng(7)
            order = rng.permutation(len(xs))[:GRID * GRID]
            spare = [(int(xs[k]), int(ys[k])) for k in order]
    for i in range(GRID):
        for j in range(GRID):
            cx = (w * (2 * j + 1)) // (2 * GRID)
            cy = (h * (2 * i + 1)) // (2 * GRID)
            if mask is not None:
                mh, mw = mask.shape
                y0, y1 = mh * i // GRID, max(mh * (i + 1) // GRID, mh * i // GRID + 1)
                x0, x1 = mw * j // GRID, max(mw * (j + 1) // GRID, mw * j // GRID + 1)
                ys, xs = np.nonzero(mask[y0:y1, x0:x1])
                if len(xs):
                    mx, my = cx * mw / w - x0, cy * mh / h - y0
                    k = int(np.argmin((xs - mx) ** 2 + (ys - my) ** 2))
                    cx = int((x0 + xs[k] + 0.5) * w / mw)
                    cy = int((y0 + ys[k] + 0.5) * h / mh)
                elif spare:
                    px, py = spare.pop(0)
                    cx = int((px + 0.5) * w / mw)
                    cy = int((py + 0.5) * h / mh)
            x = min(max(cx - rw // 2, 0), w - rw)
            y = min(max(cy - rh // 2, 0), h - rh)
            if (x, y, rw, rh) not in out:
                out.append((x, y, rw, rh))
    return out


def coarse_content(tg: g.Target, c: int, z: int, t: int) -> Optional[np.ndarray]:
    """The content mask of the largest level of `tg` up to COARSE_PIXELS, read whole."""
    small = [lv for lv in tg.levels if lv[0] > 0 and lv[1] * lv[2] <= COARSE_PIXELS]
    if not small:
        return None
    level, lw, lh, _, _ = max(small, key=lambda lv: lv[1] * lv[2])
    return content_mask(tg.read(level, c, z, t, 0, 0, lw, lh))


def run(ident: str) -> Optional[Path]:
    rel, lossy, make = SLIDES[ident]
    path = g.CORPUS / rel
    if not path.exists():
        print(f"skip {ident}: {rel} not in the corpus", file=sys.stderr)
        return None
    if ident not in set(g.manifest_ids().values()):
        raise SystemExit(f"{ident} is not in corpus/manifest.toml")
    reader, targets = make(path)
    out: Dict[str, Any] = {"id": ident, "file": rel, "reader": reader, "lossy": lossy, "regions": []}
    for tg in targets:
        by_level = {lv[0]: lv for lv in tg.levels}
        _, w, h, _, _ = by_level[0]
        for (c, z, t) in tg.planes:
            for (x, y, rw, rh) in sampled(w, h, coarse_content(tg, c, z, t)):
                a = tg.read(0, c, z, t, x, y, rw, rh)
                if a.shape[:2] != (rh, rw):
                    raise SystemExit(f"{ident}: oracle returned {a.shape} for region {(x, y, rw, rh)}")
                entry = {"image": tg.image, "level": 0, "level_size": [w, h], "c": c, "z": z, "t": t,
                         "region": [x, y, rw, rh], **g.summary(a)}
                why = tg.disputed(0, (x, y, rw, rh)) if tg.disputed else None
                if why:
                    entry["disputed"] = why
                if tg.alt is not None:
                    b = tg.alt[1](0, c, z, t, x, y, rw, rh)
                    if b is not None and g.xxh3(b) != entry["xxh3"]:
                        entry["xxh3_alt"] = g.xxh3(b)
                        entry["alt_reader"] = tg.alt[0]
                out["regions"].append(entry)
                print(f"{ident} image {tg.image} c={c} {x},{y},{rw},{rh}: mean {entry['mean']:.3f}", file=sys.stderr)
    OUT.mkdir(parents=True, exist_ok=True)
    dest = OUT / f"{ident}.json"
    dest.write_text(json.dumps(out, indent=1) + "\n")
    return dest


def main(argv: List[str]) -> None:
    for ident in argv or list(SLIDES):
        if ident not in SLIDES:
            raise SystemExit(f"unknown slide {ident}; known: {', '.join(SLIDES)}")
        run(ident)


if __name__ == "__main__":
    main(sys.argv[1:])
