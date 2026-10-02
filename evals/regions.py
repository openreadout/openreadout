"""Whole-slide questions: answers that need a region or a pyramid level, because the full-resolution
image is far too large to read whole (a 46 000 × 32 914 RGB Aperio slide is 4.5 GB decoded; the
stitched scan of a 3.7 GB CZI is 39.6 GB).

"What is the mean intensity of the 512 × 512 region at the centre of the full-resolution slide?",
"how many pyramid levels are there and what is the pixel size at level 2?": questions a pathologist
or an image analyst asks first, and the practical way to answer them is to read a window or a
level. The answers are computed here in the oracle venv with third-party readers, never with
OpenReadout: tifffile (BSD-3) through its zarr store for the TIFF flavours (Aperio SVS, Hamamatsu
NDPI), czifile (BSD-3) with a region of interest for CZI; the Aperio pixel size comes from the
`MPP` field of the slide's own ImageDescription as tifffile parses it.

The values go to `evals/facts/regions.json`; `generate.py` builds the questions (category
`analysis`) like the analysis tier.

    oracle/.venv/bin/python evals/regions.py          # recompute evals/facts/regions.json
    oracle/.venv/bin/python evals/regions.py --check  # exit 1 if the committed facts differ
"""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Callable
from dataclasses import dataclass
from importlib.metadata import version
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
import facts
import generate as g

ROOT = Path(__file__).resolve().parent.parent
OUT = Path(__file__).resolve().parent / "facts" / "regions.json"

SVS = "openslide-aperio-cmu-1"
NDPI = "openslide-hamamatsu-cmu-1"
CZI = "zenodo10577621-Young-mouse"


def path(corpus_id: str) -> Path:
    p = facts.corpus_dir() / facts.manifest_file(corpus_id, "input")
    if not p.exists():
        raise SystemExit(f"{corpus_id}: {p} is missing; fetch the corpus first (cargo xtask corpus fetch)")
    return p


def rd(*packages: str) -> str:
    return " + ".join(f"{p} {version(p)}" for p in packages)


def tiff_centre_mean(cid: str, edge: int) -> tuple[float, str]:
    """Mean of every sample (R, G and B) of the `edge` × `edge` window at the centre of the
    full-resolution level: x0 = (width - edge) // 2, y0 = (height - edge) // 2."""
    import numpy as np
    import tifffile
    import zarr

    with tifffile.TiffFile(path(cid)) as tf:
        s = tf.series[0]
        z = zarr.open(tf.aszarr(series=0, level=0), mode="r")
        assert s.axes == "YXS", s.axes
        h, w = z.shape[0], z.shape[1]
        y0, x0 = (h - edge) // 2, (w - edge) // 2
        a = np.asarray(z[y0 : y0 + edge, x0 : x0 + edge])
        return float(a.mean(dtype="float64")), rd("tifffile", "zarr")


def svs_levels(cid: str) -> tuple[int, str]:
    import tifffile

    with tifffile.TiffFile(path(cid)) as tf:
        return len(tf.series[0].levels), rd("tifffile")


def svs_level_pixel_um(cid: str, level: int) -> tuple[float, str]:
    """Aperio MPP (µm per pixel at full resolution) × the level's downsampling (full width over
    the level's width)."""
    import tifffile

    with tifffile.TiffFile(path(cid)) as tf:
        meta = tifffile.tifffile.svs_description_metadata(tf.pages[0].description)
        s = tf.series[0]
        w0 = s.levels[0].shape[s.axes.index("X")]
        wl = s.levels[level].shape[s.axes.index("X")]
        return float(meta["MPP"]) * w0 / wl, rd("tifffile")


def czi_centre_green_mean(cid: str, edge: int) -> tuple[float, str]:
    """Scene 0 of the CZI (its bounding box, from czifile, = the full-resolution image): mean of
    the green samples of the `edge` × `edge` window at its centre, composited by pylibCZIrw
    (ZEISS libCZI, LGPL, run as a black box), which honours the subblocks' valid-pixel masks
    where the mosaic tiles overlap. czifile (BSD-3), which does not apply them here, must agree
    within 0.5 % (the question's tolerance)."""
    import czifile
    import numpy as np
    from pylibCZIrw import czi as pyczi

    with czifile.CziFile(path(cid)) as f:
        sc = f.scenes[next(iter(f.scenes.keys()))]
        bx, by, bw, bh = sc.bbox
        x0, y0 = bx + (bw - edge) // 2, by + (bh - edge) // 2
        img = sc(roi=(x0, y0, edge, edge))
        a = np.asarray(img.asarray(maxworkers=1))
        assert list(img.dims)[-1] == "S", img.dims
        other = float(a[..., 1].mean(dtype="float64"))
    with pyczi.open_czi(str(path(cid))) as d:
        b = d.read(roi=(x0, y0, edge, edge), plane={"C": 0, "Z": 0, "T": 0})
    value = float(b[..., 1].mean(dtype="float64"))  # B, G, R: green is in the middle either way
    if abs(value - other) > 0.005 * value:
        raise SystemExit(f"{cid}: libCZI {value} and czifile {other} disagree beyond 0.5 %")
    return value, rd("pylibCZIrw") + f" (black box); czifile {version('czifile')} gives {other:.6g}"


def czi_levels(cid: str) -> tuple[int, str]:
    """Full resolution plus one level per downsampling factor of the scan's pyramid subblocks
    (czifile's directory: each subblock's X ratio `size / stored_size`, rounded to the nearest
    power of two; ZEN halves level by level and a subblock's own ratio is off by its stored-size
    rounding, e.g. 128.158). czifile's `levels` keys levels by the exact ratios and lists 11 for
    this scan (its four coarsest are single tiles of factors 64, 128, 128 and 256). Checked with
    pylibCZIrw (black box): a read at zoom 1/f of the scene for the coarsest f returns
    floor(size / f) pixels."""
    import math

    import czifile
    from pylibCZIrw import czi as pyczi

    with czifile.CziFile(path(cid)) as f:
        sc = f.scenes[next(iter(f.scenes.keys()))]
        factors = set()
        for lv in sc.levels[1:]:
            for e in lv.directory_entries:
                i = e.dims.index("X")
                if e.stored_shape[i] and e.shape[i] > e.stored_shape[i]:
                    factors.add(2 ** round(math.log2(e.shape[i] / e.stored_shape[i])))
        w, h = sc.sizes["X"], sc.sizes["Y"]
    if sorted(factors) != [2**k for k in range(1, len(factors) + 1)]:
        raise SystemExit(f"{cid}: pyramid factors {sorted(factors)} are not 2, 4, 8, ...")
    top = max(factors)
    with pyczi.open_czi(str(path(cid))) as d:
        r = d.scenes_bounding_rectangle_no_pyramid[0]
        a = d.read(roi=(r.x, r.y, r.w, r.h), zoom=1 / top, plane={"C": 0, "Z": 0, "T": 0})
    if a.shape[:2] != (h // top, w // top):
        raise SystemExit(f"{cid}: libCZI returns {a.shape[:2]} at 1/{top}, expected {(h // top, w // top)}")
    return 1 + len(factors), rd("czifile") + " (subblock ratios); pylibCZIrw (black box) for the coarsest level"


@dataclass
class Fact:
    corpus_id: str
    name: str
    how: str
    compute: Callable[[], tuple[Any, str]]


FACTS: list[Fact] = [
    Fact(
        SVS,
        "level_count",
        "number of resolution levels of the slide image (full resolution included)",
        lambda: svs_levels(SVS),
    ),
    Fact(
        SVS,
        "level2_pixel_um",
        "µm per pixel of level 2 (full resolution = level 0): MPP × width0 / width2",
        lambda: svs_level_pixel_um(SVS, 2),
    ),
    Fact(
        SVS,
        "centre512_mean",
        "mean of R, G and B samples of the 512 × 512 window at the centre of level 0",
        lambda: tiff_centre_mean(SVS, 512),
    ),
    Fact(
        NDPI,
        "centre512_mean",
        "mean of R, G and B samples of the 512 × 512 window at the centre of level 0",
        lambda: tiff_centre_mean(NDPI, 512),
    ),
    Fact(
        CZI,
        "level_count",
        "number of resolution levels of scene 0 (full resolution included): one per power-of-two "
        "downsampling factor of the pyramid subblocks (1/2 ... 1/256)",
        lambda: czi_levels(CZI),
    ),
    Fact(
        CZI,
        "centre1024_green_mean",
        "mean green sample of the 1024 × 1024 window at the centre of scene 0",
        lambda: czi_centre_green_mean(CZI, 1024),
    ),
]


def tidy(v: Any) -> Any:
    if isinstance(v, int):
        return v
    return float(f"{float(v):.8g}")


def compute() -> dict:
    out: dict = {}
    for f in FACTS:
        value, reader = f.compute()
        entry = out.setdefault(
            f.corpus_id,
            {"file": facts.manifest_file(f.corpus_id, "input"), "extractor": "evals/regions.py", "facts": {}},
        )
        entry["facts"][f.name] = {"value": tidy(value), "reader": reader, "how": f.how}
        print(f"{f.corpus_id} {f.name} = {value!r}", file=sys.stderr)
    return out


def render(data: dict) -> str:
    return json.dumps(data, indent=1, ensure_ascii=False, sort_keys=True) + "\n"


# JPEG tiles decode slightly differently in every implementation (± a grey level): 0.5 % covers
# any decoder, a misplaced window is off by far more.
REGION_SPECS: list[g.Spec] = [
    g.Spec(
        "ana-wsi-svs-levels",
        SVS,
        "analysis",
        "This whole-slide image is stored as a resolution pyramid. How many resolution levels does the "
        "slide image have, counting the full resolution as one of them (label, macro and thumbnail "
        "images are not levels)?",
        lambda f, m: g.integer(f["level_count"]),
        "facts-regions: level_count (tifffile)",
        answer_hint="the number of levels",
    ),
    g.Spec(
        "ana-wsi-svs-level2-pixel-size",
        SVS,
        "analysis",
        "Number the pyramid levels of this slide from 0 (full resolution). What is the physical pixel "
        "size at level 2, in micrometres per pixel?",
        lambda f, m: g.number(f["level2_pixel_um"], "µm", rel=0.01),
        "facts-regions: level2_pixel_um (tifffile: Aperio MPP × downsampling)",
        answer_hint="µm per pixel",
    ),
    g.Spec(
        "ana-wsi-svs-centre-mean",
        SVS,
        "analysis",
        "Take the 512 × 512 pixel window at the centre of the full-resolution slide image (top-left "
        "corner at x = (width − 512) / 2 and y = (height − 512) / 2, rounded down). What is its mean raw "
        "value over all three colour samples (R, G and B)?",
        lambda f, m: g.number(f["centre512_mean"], None, rel=0.005),
        "facts-regions: centre512_mean (tifffile + zarr)",
        answer_hint="a number (raw 8-bit values as stored)",
    ),
    g.Spec(
        "ana-wsi-ndpi-centre-mean",
        NDPI,
        "analysis",
        "Take the 512 × 512 pixel window at the centre of the full-resolution slide image (top-left "
        "corner at x = (width − 512) / 2 and y = (height − 512) / 2, rounded down). What is its mean raw "
        "value over all three colour samples (R, G and B)?",
        lambda f, m: g.number(f["centre512_mean"], None, rel=0.005),
        "facts-regions: centre512_mean (tifffile + zarr)",
        answer_hint="a number (raw 8-bit values as stored)",
    ),
    g.Spec(
        "ana-wsi-czi-levels",
        CZI,
        "analysis",
        "This slide scan is stored with a resolution pyramid. How many resolution levels does the scan "
        "have, counting the full resolution as one of them?",
        lambda f, m: g.integer(f["level_count"]),
        "facts-regions: level_count (czifile subblock ratios as powers of two; pylibCZIrw)",
        answer_hint="the number of levels",
    ),
    g.Spec(
        "ana-wsi-czi-centre-green",
        CZI,
        "analysis",
        "Take the 1024 × 1024 pixel window at the centre of the full-resolution stitched scan (top-left "
        "corner at x = (width − 1024) / 2 and y = (height − 1024) / 2 from the scan's top-left corner, "
        "rounded down). What is the mean raw value of its green samples?",
        lambda f, m: g.number(f["centre1024_green_mean"], None, rel=0.005),
        "facts-regions: centre1024_green_mean (pylibCZIrw, black box; czifile within 0.2 %)",
        answer_hint="a number (raw 8-bit values as stored)",
    ),
]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true", help="exit 1 if evals/facts/regions.json is out of date")
    args = ap.parse_args()
    text = render(compute())
    if args.check:
        if not OUT.exists() or OUT.read_text(encoding="utf-8") != text:
            print("evals/facts/regions.json is out of date; run evals/regions.py", file=sys.stderr)
            return 1
        print("evals/facts/regions.json is up to date")
        return 0
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(text, encoding="utf-8")
    print(f"wrote {OUT.relative_to(ROOT)}: {len(FACTS)} facts")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
