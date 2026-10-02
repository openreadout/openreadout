"""Visual tier: questions where looking at the data (an overview, then a zoomed region) is the
natural route to the answer, with answers a number can still check.

"Where on this slide is the largest piece of tissue?", "which of these z positions is in best
focus?", "which wells look empty?", "is there mains hum in this recording?": a scientist glances
at a picture to answer them. Each still has one objectively checkable answer, computed here in the
oracle venv with third-party readers plus NumPy/SciPy, never with OpenReadout: tifffile (BSD-3)
for Aperio SVS, Hamamatsu NDPI, Vectra QPTIFF and the Harmony plane; czifile (BSD-3) for CZI; nd2
(BSD-3); dcimg (MIT, with the oracle's NumPy-2 shim); pyABF (MIT); allotropy (MIT) for plate
exports; rainbow-api (LGPL, run as a black box) for a ChemStation signal. Every fact records its
reader, the method, the parameters and a robustness margin (how far the answer is from the
runner-up, or that it does not change over a grid of reasonable thresholds); questions whose
answer was not robust were dropped (see the README's "Visual tier").

Answer types added for this tier (`score.py`): `bbox` (IoU against the ground-truth box),
`point` (inside a ground-truth mask, stored as row runs at a pyramid level, or within a radius)
and `choice` (named options: one or several accepted, any other named option fails the answer).

The values go to `evals/facts/visual.json`; `generate.py` builds `questions/visual.jsonl`
(category `visual`, with a `subcategory`) from `VISUAL_SPECS` below.

    oracle/.venv/bin/python evals/visual.py          # recompute evals/facts/visual.json (needs the corpus)
    oracle/.venv/bin/python evals/visual.py --check  # exit 1 if the committed facts differ
"""

from __future__ import annotations

import argparse
import json
import re
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
OUT = Path(__file__).resolve().parent / "facts" / "visual.json"

SVS = "openslide-aperio-cmu-1"
NDPI = "openslide-hamamatsu-cmu-1"
QPTIFF = "ome-qptiff-hande-compressed-scan1"
OVERVIEW = "aics-OverViewScan"
ZEISS_SLIDE = "openslide-zeiss-5-jxr"
BEADS = [f"dcimg-zenodo14268554-bead-bot4-560-00000-{i:05d}" for i in range(11)]
BEAD_STAGED = [f"sample_{i + 1:02d}.dcimg" for i in range(len(BEADS))]  # neutral names, z order kept
KARL = "ome-karl-sample-image"
BUT3 = "aics-ND2-aryeh-but3-cont200-1"
POR = "ome-aryeh-por003"
HARMONY = "hcs-harmony-zenodo7841360-folder"
ABF_MAINS = "pyabf-2020-07-29-0062"
CHROM = "chromhandler-ca10-100um-d-dad1b-ch"
PLATE_COLUMN = "kaleido-abs-endpoint"


def manifest_file(corpus_id: str) -> str:
    """The corpus file of an id: its `input` entry, or its `part` entry (a signal file inside a
    vendor directory, staged on its own)."""
    try:
        return facts.manifest_file(corpus_id, "input")
    except KeyError:
        return facts.manifest_file(corpus_id, "part")


def path(corpus_id: str) -> Path:
    p = facts.corpus_dir() / manifest_file(corpus_id)
    if not p.exists():
        raise SystemExit(f"{corpus_id}: {p} is missing; fetch the corpus first (cargo xtask corpus fetch)")
    return p


def rd(*packages: str) -> str:
    return " + ".join(f"{p} {version(p)}" for p in packages)


# ---------------------------------------------------------------- image helpers


def otsu(x) -> float:
    """Otsu's threshold over a 256-bin histogram (NumPy; the textbook between-class variance)."""
    import numpy as np

    x = np.asarray(x, dtype="float64").ravel()
    h, e = np.histogram(x, bins=256)
    c = (e[:-1] + e[1:]) / 2
    w0 = np.cumsum(h)
    w1 = w0[-1] - w0
    s0 = np.cumsum(h * c)
    m0 = s0 / np.maximum(w0, 1)
    m1 = (s0[-1] - s0) / np.maximum(w1, 1)
    return float(c[int(np.argmax(w0 * w1 * (m0 - m1) ** 2))])


def components(mask, min_frac: float = 0.0):
    """[(area, bbox (x0, y0, x1, y1) inclusive, label image index)] largest first, areas ≥
    min_frac of the image."""
    import numpy as np
    from scipy import ndimage as ndi

    lab, n = ndi.label(mask)
    if n == 0:
        return [], lab
    areas = np.bincount(lab.ravel())[1:]
    objs = ndi.find_objects(lab)
    out = []
    for i, (sl, a) in enumerate(zip(objs, areas, strict=True)):
        if a < min_frac * mask.size:
            continue
        ys, xs = sl
        out.append((int(a), (xs.start, ys.start, xs.stop - 1, ys.stop - 1), i + 1))
    out.sort(key=lambda t: -t[0])
    return out, lab


def to_full(bbox, sx: float, sy: float) -> list[int]:
    """A level bbox (inclusive pixel indices) as a full-resolution [x, y, width, height]."""
    x0, y0, x1, y1 = bbox
    fx0, fy0 = round(x0 * sx), round(y0 * sy)
    return [fx0, fy0, round((x1 + 1) * sx) - fx0, round((y1 + 1) * sy) - fy0]


def iou(a: list[float], b: list[float]) -> float:
    ax0, ay0, aw, ah = a
    bx0, by0, bw, bh = b
    ix = max(0.0, min(ax0 + aw, bx0 + bw) - max(ax0, bx0))
    iy = max(0.0, min(ay0 + ah, by0 + bh) - max(ay0, by0))
    inter = ix * iy
    union = aw * ah + bw * bh - inter
    return inter / union if union > 0 else 0.0


def runs(mask) -> list[list[int]]:
    """Row runs [row, x_start, x_end_exclusive] of a boolean mask (compact enough for JSON)."""
    import numpy as np

    out = []
    for y in np.flatnonzero(mask.any(axis=1)):
        row = np.concatenate([[0], mask[y].astype("int8"), [0]])
        d = np.diff(row)
        for s, e in zip(np.flatnonzero(d == 1), np.flatnonzero(d == -1), strict=True):
            out.append([int(y), int(s), int(e)])
    return out


def tiff_level(cid: str, level: int):
    """(level array YXS, sx, sy): full-resolution pixels per level pixel along x and y."""
    import numpy as np
    import tifffile

    with tifffile.TiffFile(path(cid)) as tf:
        s = tf.series[0]
        assert s.axes == "YXS", s.axes
        h0, w0 = s.levels[0].shape[:2]
        a = np.asarray(s.levels[level].asarray(maxworkers=1))
    h, w = a.shape[:2]
    return a, w0 / w, h0 / h


def brightfield_tissue(rgb, factor: float = 1.0):
    """Tissue of a brightfield slide: colour saturation (max − min of R, G, B, Gaussian σ = 2 level
    pixels) above factor × its Otsu threshold, closed (3 iterations), holes filled, opened (2)."""
    from scipy import ndimage as ndi

    a = rgb[..., :3].astype("float64")
    sat = ndi.gaussian_filter(a.max(-1) - a.min(-1), 2)
    m = sat > otsu(sat) * factor
    m = ndi.binary_closing(m, iterations=3)
    m = ndi.binary_fill_holes(m)
    return ndi.binary_opening(m, iterations=2)


# ---------------------------------------------------------------- whole-slide facts

SECTION_MIN_FRAC = 0.005  # a tissue piece is at least 0.5 % of the slide image (dust and specks are far smaller)
FACTORS = (0.7, 1.0, 1.3)  # Otsu multipliers of the sensitivity check


def slide_sections(cid: str, level: int):
    a, sx, sy = tiff_level(cid, level)
    per = {}
    for f in FACTORS:
        comps, _ = components(brightfield_tissue(a, f), SECTION_MIN_FRAC)
        per[f] = comps
    return a, sx, sy, per


def section_count(cid: str, level: int):
    """Tissue pieces ≥ 0.5 % of the level image; the count must not change between 0.7 and 1.3 ×
    the Otsu threshold. Margin: the smallest piece against the largest blob left out."""
    a, _, _, per = slide_sections(cid, level)
    counts = {str(f): len(c) for f, c in per.items()}
    if len(set(counts.values())) != 1:
        raise SystemExit(f"{cid}: section count depends on the threshold: {counts}")
    n = counts["1.0"]
    every, _ = components(brightfield_tissue(a))
    ev = {
        "level": level,
        "counts_by_otsu_factor": counts,
        "min_area_px": int(SECTION_MIN_FRAC * a.shape[0] * a.shape[1]),
        "smallest_piece_px": min(c[0] for c in per[1.0]),
        "largest_blob_left_out_px": every[n][0] if len(every) > n else 0,
    }
    return n, rd("tifffile"), ev


def folded_section_bbox(cid: str, level: int, factors=(0.8, 1.0, 1.15)):
    """Bounding box of the tissue piece that holds the fold (the piece containing most of the fold
    mask of `slide_fold`), at `factors` × the Otsu threshold (this scan's background is tinted, so
    above 1.2 × the pieces start to fragment; 0.8–1.15 is the stable range)."""
    import numpy as np

    a, sx, sy = tiff_level(cid, level)
    fold = fold_mask(a)[0]
    boxes, shares = {}, {}
    for f in factors:
        comps, lab = components(brightfield_tissue(a, f), SECTION_MIN_FRAC)
        hits = [(int((lab[fold] == k).sum()), c) for c in comps for k in [c[2]]]
        n, best = max(hits, key=lambda t: t[0])
        boxes[str(f)] = to_full(best[1], sx, sy)
        shares[str(f)] = round(n / int(fold.sum()), 3)
        if f == 1.0:
            others = [round(iou(to_full(best[1], sx, sy), to_full(c[1], sx, sy)), 3) for c in comps if c is not best]
            count = len(comps)
    ref = boxes["1.0"]
    ious = {k: round(iou(ref, b), 3) for k, b in boxes.items()}
    if min(ious.values()) < 0.9 or min(shares.values()) < 0.9:
        raise SystemExit(f"{cid}: folded-section box is not stable: {ious} {shares}")
    ev = {
        "level": level,
        "scale_xy": [round(sx, 4), round(sy, 4)],
        "pieces": count,
        "iou_across_otsu_factors": ious,
        "share_of_fold_inside_the_piece": shares,
        "iou_with_the_other_pieces": others,
        "fold_level_px": int(np.count_nonzero(fold)),
    }
    return ref, rd("tifffile"), ev


def slide_tissue_bbox(cid: str, level: int):
    """Bounding box of every tissue piece ≥ 0.5 % of the image together."""
    a, sx, sy = tiff_level(cid, level)
    boxes = {}
    for f in FACTORS:
        comps, _ = components(brightfield_tissue(a, f), SECTION_MIN_FRAC)
        xs0 = min(c[1][0] for c in comps)
        ys0 = min(c[1][1] for c in comps)
        xs1 = max(c[1][2] for c in comps)
        ys1 = max(c[1][3] for c in comps)
        boxes[str(f)] = (to_full((xs0, ys0, xs1, ys1), sx, sy), len(comps), to_full(comps[0][1], sx, sy))
    ref = boxes["1.0"][0]
    ious = {k: round(iou(ref, b[0]), 3) for k, b in boxes.items()}
    if min(ious.values()) < 0.9:
        raise SystemExit(f"{cid}: tissue box depends on the threshold: {ious}")
    ev = {
        "level": level,
        "scale_xy": [round(sx, 4), round(sy, 4)],
        "iou_across_otsu_factors": ious,
        "pieces": boxes["1.0"][1],
        "iou_largest_piece_vs_all": round(iou(ref, boxes["1.0"][2]), 3),
    }
    return ref, rd("tifffile"), ev


FOLD_GREY = 100  # of 255: the doubled-over strip is darker than any single-thickness tissue
FOLD_DILATE = 4  # level pixels accepted around the fold (64 full-resolution pixels at level 2)


def fold_mask(a, grey_max: float = FOLD_GREY):
    """(mask, components): the largest region of the tissue whose mean R, G, B (Gaussian σ = 2)
    is below `grey_max`, opened twice."""
    from scipy import ndimage as ndi

    grey = ndi.gaussian_filter(a[..., :3].astype("float64").mean(-1), 2)
    m = ndi.binary_opening((grey < grey_max) & brightfield_tissue(a), iterations=2)
    comps, lab = components(m)
    return lab == comps[0][2], comps


def slide_fold(cid: str, level: int):
    """The fold: the largest region inside the tissue whose mean R, G, B (Gaussian σ = 2 level
    pixels) is below 100, opened twice. The answer is a point inside it (dilated by 4 level
    pixels); the point stored as `value` is its centroid snapped to the nearest mask pixel."""
    import numpy as np
    from scipy import ndimage as ndi

    a, sx, sy = tiff_level(cid, level)
    mask, comps = fold_mask(a)
    area = comps[0][0]
    ys, xs = np.nonzero(mask)
    cy, cx = ys.mean(), xs.mean()
    j = int(np.argmin((ys - cy) ** 2 + (xs - cx) ** 2))
    px, py = int(xs[j]), int(ys[j])
    accept = ndi.binary_dilation(mask, iterations=FOLD_DILATE)
    # the fold found at other grey thresholds must fall inside the accepted region
    checks = {}
    for thr in (80, 90, 110):
        m2, _ = fold_mask(a, thr)
        checks[str(thr)] = round(float(accept[m2].mean()), 3)
    if min(checks.values()) < 0.8:
        raise SystemExit(f"{cid}: fold region depends on the threshold: {checks}")
    value = [round((px + 0.5) * sx), round((py + 0.5) * sy)]
    ev = {
        "level": level,
        "scale_xy": [round(sx, 4), round(sy, 4)],
        "fold_area_level_px": area,
        "second_dark_region_area_ratio": round(comps[1][0] / area, 3) if len(comps) > 1 else 0.0,
        "share_of_fold_at_grey_thresholds_inside_accepted_region": checks,
        "mask": {"level": level, "scale": [sx, sy], "runs": runs(accept)},
    }
    return value, rd("tifffile"), ev


# ---------------------------------------------------------------- CZI overview and slide scan


def czi_level(cid: str, level: int):
    """(scene 0 at pyramid level `level` squeezed to 2D/YXS, sx, sy)."""
    import czifile
    import numpy as np

    with czifile.CziFile(path(cid)) as f:
        sc = f.scenes[next(iter(f.scenes.keys()))]
        w0, h0 = sc.sizes["X"], sc.sizes["Y"]
        a = np.asarray(sc.levels[level].asarray(maxworkers=1))
    a = np.squeeze(a)
    return a, w0 / a.shape[1], h0 / a.shape[0]


def fluorescence_sections(a, factor: float = 1.0):
    """Tissue of a fluorescence overview: Gaussian σ = 2 above factor × Otsu, closed, holes filled."""
    from scipy import ndimage as ndi

    s = ndi.gaussian_filter(a.astype("float64"), 2)
    m = s > otsu(s) * factor
    m = ndi.binary_closing(m, iterations=2)
    return ndi.binary_fill_holes(m), m


# A DAPI overview's Otsu threshold sits high (sections are much brighter than the slide), and above
# it the dimmer sections split along their ventricles: the stable range is 0.5–1.0 × Otsu.
FL_FACTORS = (0.5, 0.7, 1.0)
FL_REF = "0.7"


def overview_count(cid: str, level: int):
    a = czi_level(cid, level)[0]
    counts = {}
    for f in FL_FACTORS:
        filled, _ = fluorescence_sections(a, f)
        counts[str(f)] = len(components(filled, SECTION_MIN_FRAC)[0])
    if len(set(counts.values())) != 1:
        raise SystemExit(f"{cid}: section count depends on the threshold: {counts}")
    return counts[FL_REF], rd("czifile"), {"level": level, "counts_by_otsu_factor": counts}


HOLE_DILATE = 2


def overview_hole(cid: str, level: int):
    """The largest hole: background pixels enclosed by a tissue section (filled mask minus the
    tissue mask), opened twice, at 0.5, 0.7 and 1.0 × Otsu; accepted: the union of the three
    largest holes dilated by 2 level pixels, and the three must overlap (the same hole)."""
    import numpy as np
    from scipy import ndimage as ndi

    a, sx, sy = czi_level(cid, level)
    per = {}
    for f in FL_FACTORS:
        filled, tissue = fluorescence_sections(a, f)
        comps, lab = components(ndi.binary_opening(filled & ~tissue, iterations=2))
        per[str(f)] = (comps, lab == comps[0][2])
    comps, mask = per[FL_REF]
    area = comps[0][0]
    union = np.logical_or.reduce([m for _, m in per.values()])
    accept = ndi.binary_dilation(union, iterations=HOLE_DILATE)
    overlap = {f: round(float((m & mask).sum() / m.sum()), 3) for f, (_, m) in per.items()}
    ratios = {f: round(c[1][0] / c[0][0], 3) if len(c) > 1 else 0.0 for f, (c, _) in per.items()}
    if min(overlap.values()) < 0.5 or max(ratios.values()) > 0.5:
        raise SystemExit(f"{cid}: the largest hole is not clear: {overlap}, {ratios}")
    ys, xs = np.nonzero(mask)
    j = int(np.argmin((ys - ys.mean()) ** 2 + (xs - xs.mean()) ** 2))
    value = [round((int(xs[j]) + 0.5) * sx), round((int(ys[j]) + 0.5) * sy)]
    ev = {
        "level": level,
        "scale_xy": [round(sx, 4), round(sy, 4)],
        "hole_area_level_px": area,
        "second_hole_area_ratio_by_otsu_factor": ratios,
        "share_of_each_largest_hole_overlapping_the_reference": overlap,
        "mask": {"level": level, "scale": [sx, sy], "runs": runs(accept)},
    }
    return value, rd("czifile"), ev


CORNERS = ("top-left", "top-right", "bottom-left", "bottom-right")


def slide_empty_corners(cid: str):
    """Corners of scene 0 (the coarsest pyramid level) that the scanner left empty: the share of
    exactly-zero pixels (every sample 0) in each corner quarter-block (a quarter of the width by a
    quarter of the height); empty when more than a quarter of the block is zero."""
    import czifile
    import numpy as np

    with czifile.CziFile(path(cid)) as f:
        sc = f.scenes[next(iter(f.scenes.keys()))]
        a = np.asarray(sc.levels[-1].asarray(maxworkers=1))
    g_ = np.squeeze(a)
    g_ = g_.max(-1) if g_.ndim == 3 else g_
    h, w = g_.shape
    qh, qw = h // 4, w // 4
    blocks = {
        "top-left": g_[:qh, :qw],
        "top-right": g_[:qh, w - qw :],
        "bottom-left": g_[h - qh :, :qw],
        "bottom-right": g_[h - qh :, w - qw :],
    }
    zero = {k: round(float((b == 0).mean()), 3) for k, b in blocks.items()}
    empty = [k for k in CORNERS if zero[k] > 0.25]
    full = [zero[k] for k in CORNERS if k not in empty]
    ev = {
        "level_shape": [h, w],
        "zero_share_per_corner_block": zero,
        "margin": [min(zero[k] for k in empty), max(full)],
    }
    return empty, rd("czifile") + " (JPEG XR via imagecodecs)", ev


# ---------------------------------------------------------------- focus


def focus_metrics(stack) -> dict[str, list[float]]:
    """Per plane: variance of the Laplacian of a Gaussian-smoothed (σ = 1) plane over mean², the
    normalised variance (variance / mean) and the Tenengrad (mean squared Sobel gradient / mean²)."""
    import numpy as np
    from scipy import ndimage as ndi

    out: dict[str, list[float]] = {"laplacian_variance": [], "normalised_variance": [], "tenengrad": []}
    for pl in stack:
        a = np.asarray(pl, dtype="float64")
        m = a.mean()
        out["laplacian_variance"].append(float(ndi.laplace(ndi.gaussian_filter(a, 1.0)).var() / m**2))
        out["normalised_variance"].append(float(a.var() / m))
        out["tenengrad"].append(float((np.hypot(ndi.sobel(a, 0), ndi.sobel(a, 1)) ** 2).mean() / m**2))
    return out


def best_focus(stack) -> tuple[list[int], dict]:
    """Accepted planes: the peak of each focus measure and every plane between them (0-based)."""
    import numpy as np

    m = focus_metrics(stack)
    peaks = {k: int(np.argmax(v)) for k, v in m.items()}
    lo, hi = min(peaks.values()), max(peaks.values())
    rel = {k: [round(x / max(v), 3) for x in v] for k, v in m.items()}
    return list(range(lo, hi + 1)), {"peaks_0_based": peaks, "relative_to_peak": rel}


def dcimg_planes(cids: list[str]):
    import dcimg
    import numpy as np

    sys.path.insert(0, str(ROOT / "oracle"))
    from gen import _dcimg_numpy2_shim

    _dcimg_numpy2_shim(dcimg)
    planes = []
    for cid in cids:
        f = dcimg.DCIMGFile(str(path(cid)))
        try:
            planes.append(np.array(f.mma[0]))  # the stored frame (these files store no corrected pixels)
        finally:
            f.close()
    return planes


def bead_focus():
    """The camera file (one z position each) in best focus; accepted: the peaks of the three focus
    measures and the files between them, widened by one file on each side when the runner-up of
    the peak-intensity ranking (another common criterion) is a neighbour."""
    import numpy as np

    planes = dcimg_planes(BEADS)
    acc, ev = best_focus(planes)
    peak = [int(p.max()) for p in planes]
    order = list(np.argsort(peak)[::-1])
    ev["peak_intensity"] = peak
    acc = sorted(set(acc) | {int(order[0]), int(order[1])} | {acc[0] - 1, acc[-1] + 1})
    acc = [i for i in acc if 0 <= i < len(planes)]
    names = [Path(manifest_file(c)).name for c in BEADS]
    ev["files"] = names
    return [names[i] for i in acc], rd("dcimg") + " (MIT; oracle NumPy-2 shim)", ev


def nd2_zstack_focus(cid: str, channel: int):
    import nd2
    import numpy as np

    with nd2.ND2File(str(path(cid))) as f:
        dims = list(f.sizes)
        assert dims == ["Z", "C", "Y", "X"], dims
        name = f.metadata.channels[channel].channel.name
        stack = np.asarray(f.to_dask()[:, channel])
    acc, ev = best_focus(stack)
    ev["channel"] = name
    # a z index counted from 1: the accepted planes must be contiguous (a ± 0.5-wide number window)
    return [i + 1 for i in acc], rd("nd2"), ev


# ---------------------------------------------------------------- positions, time points, fields


def nd2_position_cell(cid: str, position: int, channel: int):
    """The cell at one stage position: Gaussian σ = 2 above Otsu, opened twice, the largest
    component; accepted: that mask dilated by 10 pixels."""
    import nd2
    import numpy as np
    from scipy import ndimage as ndi

    with nd2.ND2File(str(path(cid))) as f:
        dims = list(f.sizes)
        assert dims == ["P", "C", "Y", "X"], dims
        name = f.metadata.channels[channel].channel.name
        a = np.asarray(f.to_dask()[position, channel]).astype("float64")
    s = ndi.gaussian_filter(a, 2)
    per = {}
    for fct in FACTORS:
        per[str(fct)] = components(ndi.binary_opening(s > otsu(s) * fct, iterations=2))
    comps, lab = per["1.0"]
    area, _, k = comps[0]
    mask = lab == k
    accept = ndi.binary_dilation(mask, iterations=10)
    ys, xs = np.nonzero(mask)
    cy, cx = ys.mean(), xs.mean()
    j = int(np.argmin((ys - cy) ** 2 + (xs - cx) ** 2))
    # at other thresholds the cell grows or shrinks (a dim halo), but its centre stays inside
    centres, areas = {}, {}
    for f_, (c2, lab2) in per.items():
        yy, xx = np.nonzero(lab2 == c2[0][2])
        c = (round(xx.mean()), round(yy.mean()))
        centres[f_] = [*c, bool(accept[c[1], c[0]])]
        areas[f_] = int(c2[0][0])
    if not all(v[2] for v in centres.values()):
        raise SystemExit(f"{cid}: cell centre depends on the threshold: {centres}")
    ev = {
        "channel": name,
        "cell_area_px": area,
        "second_object_area_ratio": round(comps[1][0] / area, 3) if len(comps) > 1 else 0.0,
        "cell_area_px_by_otsu_factor": areas,
        "cell_centre_by_otsu_factor_and_inside_accepted_region": centres,
        "mask": {"level": 0, "scale": [1, 1], "runs": runs(accept)},
    }
    return [int(xs[j]), int(ys[j])], rd("nd2"), ev


def nd2_channel_interval(cid: str, channel: int):
    """Time points at which a channel holds an image (any non-zero pixel) at every stage position;
    the answer is the spacing between them (the same at every position)."""
    import nd2
    import numpy as np

    with nd2.ND2File(str(path(cid))) as f:
        dims = list(f.sizes)
        assert dims == ["T", "P", "C", "Y", "X"], dims
        name = f.metadata.channels[channel].channel.name
        d = f.to_dask()
        per = {}
        for p in range(f.sizes["P"]):
            st = np.asarray(d[:, p, channel])
            mx = st.reshape(len(st), -1).max(1)
            per[p] = [int(t) for t in np.flatnonzero(mx > 0)]
        n_t = f.sizes["T"]
    if len({tuple(v) for v in per.values()}) != 1:
        raise SystemExit(f"{cid}: recorded frames differ between positions")
    rec = per[0]
    steps = sorted(set(np.diff(rec).tolist()))
    if len(steps) != 1:
        raise SystemExit(f"{cid}: uneven interval {steps}")
    ev = {"channel": name, "time_points": n_t, "recorded_0_based": rec, "blank_frames_are_all_zero": True}
    return int(steps[0]), rd("nd2"), ev


def nuclei_count():
    """Nuclei in the one Hoechst field: Gaussian σ, above factor × Otsu, holes filled, opened once,
    components of at least `min_area` pixels; the reference is the median over σ ∈ {1, 2},
    factor ∈ {0.8, 1.0, 1.2}, min_area ∈ {20, 50} px, and every grid value must lie within the
    question's tolerance of it."""
    import numpy as np
    import tifffile
    from scipy import ndimage as ndi

    p = path(HARMONY) / "Images" / "r03c07f01p01-ch1sk1fk1fl1.tiff"
    img = np.asarray(tifffile.imread(p)).astype("float64")
    grid = {}
    for sigma in (1.0, 2.0):
        s = ndi.gaussian_filter(img, sigma)
        t = otsu(s)
        for fct in (0.8, 1.0, 1.2):
            m = ndi.binary_opening(ndi.binary_fill_holes(s > t * fct), iterations=1)
            lab, _ = ndi.label(m)
            areas = np.bincount(lab.ravel())[1:]
            for min_area in (20, 50):
                grid[f"sigma={sigma:g},factor={fct:g},min_area={min_area}"] = int((areas >= min_area).sum())
    ref = int(np.median(list(grid.values())))
    spread = [min(grid.values()) / ref - 1, max(grid.values()) / ref - 1]
    if max(abs(x) for x in spread) > NUCLEI_TOL:
        raise SystemExit(f"nuclei count not robust: {grid}")
    ev = {"plane": "Images/r03c07f01p01-ch1sk1fk1fl1.tiff", "grid": grid, "spread_rel": [round(x, 3) for x in spread]}
    return ref, rd("tifffile"), ev


NUCLEI_TOL = 0.2


# ---------------------------------------------------------------- traces, chromatograms, plates


def abf_mains(cid: str, channel: int):
    """Welch power spectrum (2 s segments) of the whole gap-free channel: the power at 50 and 60 Hz
    (and harmonics) relative to the median power between 30 and 200 Hz. Answer: the line frequency
    whose peak stands out."""
    import numpy as np
    import pyabf
    from scipy import signal

    abf = pyabf.ABF(str(path(cid)))
    abf.setSweep(0, channel=channel)
    y = np.asarray(abf.sweepY, dtype="float64")
    fs = float(abf.dataRate)
    f, p = signal.welch(y - y.mean(), fs=fs, nperseg=int(2 * fs))
    base = float(np.median(p[(f > 30) & (f < 200)]))

    def at(hz: float) -> float:
        i = int(np.argmin(abs(f - hz)))
        return float(p[max(0, i - 1) : i + 2].max() / base)

    rel = {f"{hz} Hz": round(at(hz), 1) for hz in (50, 60, 100, 120, 150, 180)}
    line = 60 if rel["60 Hz"] > rel["50 Hz"] else 50
    other = 50 if line == 60 else 60
    if rel[f"{line} Hz"] < 20 * max(rel[f"{other} Hz"], 1.0):
        raise SystemExit(f"{cid}: no clear line frequency {rel}")
    ev = {"channel": f"{abf.adcNames[channel]} ({abf.adcUnits[channel]})", "power_vs_median_30_200_hz": rel}
    return line, rd("pyabf", "scipy"), ev


def chrom_peak_count(cid: str):
    """Peaks of the signal after subtracting its median: scipy.signal.find_peaks with a prominence
    of 2, 5, 10 and 20 % of the tallest peak (all must agree); 1 % is recorded too."""
    import numpy as np
    import rainbow
    from scipy.signal import find_peaks

    d = rainbow.agilent.chemstation.parse_ch(str(path(cid)))
    y = np.asarray(d.data, dtype="float64").ravel()
    x = np.asarray(d.xlabels, dtype="float64")
    y = y - np.median(y)
    top = y.max()
    counts = {}
    rts = {}
    for frac in (0.01, 0.02, 0.05, 0.1, 0.2):
        pk, _ = find_peaks(y, prominence=frac * top)
        counts[f"{frac:g}"] = len(pk)
        rts[f"{frac:g}"] = [round(float(x[i]), 3) for i in pk]
    main = {counts[k] for k in ("0.02", "0.05", "0.1", "0.2")}
    if len(main) != 1:
        raise SystemExit(f"{cid}: peak count depends on the prominence: {counts}")
    ev = {"counts_by_prominence_fraction": counts, "apex_min_at_5pct": rts["0.05"], "apex_min_at_1pct": rts["0.01"]}
    return main.pop(), rd("rainbow-api") + " (black box) + " + rd("scipy"), ev


def _plate(cid: str) -> dict[str, float]:
    import analysis

    return analysis.plate_values(cid)


def _rc(well: str) -> tuple[str, int]:
    m = re.fullmatch(r"([A-Z]+)0*(\d+)", well)
    assert m, well
    return m.group(1), int(m.group(2))


def plate_blank_column(cid: str):
    """The column whose every well reads within 2× of the plate's lowest reading, while every other
    column reads more than 10× that in row A (the loaded rows)."""
    v = _plate(cid)
    lo = min(v.values())
    cols: dict[int, list[float]] = {}
    for w, x in v.items():
        cols.setdefault(_rc(w)[1], []).append(x)
    blank = [c for c, xs in cols.items() if max(xs) <= 2 * lo]
    if len(blank) != 1:
        raise SystemExit(f"{cid}: blank columns {blank}")
    b = blank[0]
    row_a = {c: v[w] for w, _ in v.items() for c in [_rc(w)[1]] if _rc(w)[0] == "A"}
    others = min(x for c, x in row_a.items() if c != b)
    ev = {
        "plate_minimum": round(lo, 5),
        "blank_column_max": round(max(cols[b]), 5),
        "lowest_row_A_value_of_other_columns": round(others, 5),
    }
    return b, rd("allotropy"), ev


# ---------------------------------------------------------------- the facts


@dataclass
class Fact:
    corpus_id: str
    name: str
    how: str
    compute: Callable[[], tuple[Any, str, dict]]


FACTS: list[Fact] = [
    Fact(
        SVS,
        "section_count",
        "tissue pieces ≥ 0.5 % of level 2 (colour saturation above Otsu)",
        lambda: section_count(SVS, 2),
    ),
    Fact(
        SVS,
        "fold_point",
        "a point in the tissue fold: the largest region of the tissue darker than 100/255 at level 2",
        lambda: slide_fold(SVS, 2),
    ),
    Fact(
        NDPI,
        "folded_section_bbox",
        "full-resolution [x, y, width, height] of the tissue piece holding the fold, at level 2, scaled up",
        lambda: folded_section_bbox(NDPI, 2),
    ),
    Fact(
        QPTIFF,
        "tissue_bbox",
        "full-resolution [x, y, width, height] of all tissue at level 4 (1/16), scaled up",
        lambda: slide_tissue_bbox(QPTIFF, 4),
    ),
    Fact(
        OVERVIEW,
        "section_count",
        "tissue sections ≥ 0.5 % of level 2 (DAPI above 0.7 × Otsu)",
        lambda: overview_count(OVERVIEW, 2),
    ),
    Fact(
        OVERVIEW,
        "hole_point",
        "a point in the largest hole enclosed by a section at level 2, scaled up",
        lambda: overview_hole(OVERVIEW, 2),
    ),
    Fact(
        ZEISS_SLIDE,
        "empty_corners",
        "corners of scene 0 the scanner left empty (zero pixels)",
        lambda: slide_empty_corners(ZEISS_SLIDE),
    ),
    Fact(BEADS[0], "best_focus_files", "camera files (z positions) accepted as best focus", bead_focus),
    Fact(
        KARL, "best_focus_z", "z-slices (from 1) accepted as best focus of channel 4", lambda: nd2_zstack_focus(KARL, 3)
    ),
    Fact(
        BUT3,
        "position3_cell_point",
        "a point in the cell at stage position 3 (from 1), DiO channel",
        lambda: nd2_position_cell(BUT3, 2, 1),
    ),
    Fact(
        POR,
        "dii_interval",
        "spacing (frames) of the time points at which the DiI channel was recorded",
        lambda: nd2_channel_interval(POR, 1),
    ),
    Fact(
        HARMONY, "nuclei_count", "nuclei in the field (threshold + label, median over a parameter grid)", nuclei_count
    ),
    Fact(
        ABF_MAINS,
        "line_frequency_hz",
        "line frequency standing out in the power spectrum of channel 1",
        lambda: abf_mains(ABF_MAINS, 0),
    ),
    Fact(
        CHROM,
        "peak_count",
        "peaks of the signal (find_peaks, prominence 2–20 % of the tallest)",
        lambda: chrom_peak_count(CHROM),
    ),
    Fact(
        PLATE_COLUMN,
        "blank_column",
        "the plate column reading at background in every row",
        lambda: plate_blank_column(PLATE_COLUMN),
    ),
]


def tidy(v: Any) -> Any:
    if isinstance(v, bool) or v is None or isinstance(v, str):
        return v
    if isinstance(v, int):
        return v
    if isinstance(v, float):
        return float(f"{v:.8g}")
    if isinstance(v, dict):
        return {str(k): tidy(x) for k, x in v.items()}
    if isinstance(v, list | tuple):
        return [tidy(x) for x in v]
    if hasattr(v, "item"):  # NumPy scalars
        return tidy(v.item())
    return v


def compute(only: list[str] | None = None) -> dict:
    old = json.loads(OUT.read_text()) if only and OUT.exists() else {}
    out: dict = old
    for f in FACTS:
        if only and f"{f.corpus_id}:{f.name}" not in only and f.name not in only:
            continue
        value, reader, evidence = f.compute()
        entry = out.setdefault(
            f.corpus_id,
            {"file": manifest_file(f.corpus_id), "extractor": "evals/visual.py", "facts": {}},
        )
        entry["facts"][f.name] = {"value": tidy(value), "reader": reader, "how": f.how, "evidence": tidy(evidence)}
        shown = {k: v for k, v in evidence.items() if k != "mask"}
        print(f"{f.corpus_id} {f.name} = {tidy(value)!r}  {json.dumps(tidy(shown))[:400]}", file=sys.stderr)
    return out


def render(data: dict) -> str:
    return json.dumps(data, indent=1, ensure_ascii=False, sort_keys=True) + "\n"


# ---------------------------------------------------------------- questions

LOCATE_BOX_HINT = (
    "the box as x, y, width, height in full-resolution pixels (x, y = its top-left corner, "
    "origin at the top-left of the image), e.g. x=1200, y=3400, width=5000, height=2000"
)
POINT_HINT = "the point as x, y in full-resolution pixels (origin at the top-left of the image), e.g. x=1200, y=3400"


def _spec(qid, cid, sub, question, answer, source, **kw) -> g.Spec:
    return g.Spec(qid, cid, "visual", question, answer, f"facts-visual: {source}", subcategory=sub, **kw)


def _mask(f: dict, name: str) -> dict:
    return f[name + "__evidence"]["mask"]


CORNER_ALIASES = {
    "top-left": ["upper-left", "top left", "upper left", "north-west", "northwest"],
    "top-right": ["upper-right", "top right", "upper right", "north-east", "northeast"],
    "bottom-left": ["lower-left", "bottom left", "lower left", "south-west", "southwest"],
    "bottom-right": ["lower-right", "bottom right", "lower right", "south-east", "southeast"],
}


VISUAL_SPECS: list[g.Spec] = [
    _spec(
        "vis-wsi-svs-section-count",
        SVS,
        "count",
        "How many separate pieces of tissue are on this slide?",
        lambda f, m: g.integer(f["section_count"]),
        "section_count (tifffile; saturation above Otsu at level 2, the same count at 0.7–1.3 × the threshold)",
        answer_hint="the number of tissue pieces",
    ),
    _spec(
        "vis-wsi-svs-fold",
        SVS,
        "locate",
        "One of the tissue sections on this slide has a fold, where the tissue doubled over into a darker "
        "strip. Where is the fold? Give a point in the middle of it.",
        lambda f, m: g.point(f["fold_point"], mask=_mask(f, "fold_point")),
        "fold_point (tifffile; darkest tissue region at level 2, mask dilated by 64 full-resolution pixels)",
        answer_hint=POINT_HINT,
    ),
    _spec(
        "vis-wsi-ndpi-folded-section",
        NDPI,
        "locate",
        "There are several tissue sections on this slide, and one of them has a fold (a darker strip where "
        "the tissue doubled over). Where is that section? Give the bounding box of the "
        "whole section, not just of the fold.",
        lambda f, m: g.bbox(f["folded_section_bbox"], min_iou=0.6),
        "folded_section_bbox (tifffile; saturation above Otsu at level 2, the piece holding the fold, scaled up; "
        "IoU ≥ 0.6)",
        answer_hint=LOCATE_BOX_HINT,
    ),
    _spec(
        "vis-wsi-qptiff-tissue-box",
        QPTIFF,
        "locate",
        "Where on this scanned slide is the tissue? Give the bounding box that encloses all of it.",
        lambda f, m: g.bbox(f["tissue_bbox"], min_iou=0.6),
        "tissue_bbox (tifffile; saturation above Otsu at level 4, scaled up; IoU ≥ 0.6)",
        answer_hint=LOCATE_BOX_HINT,
    ),
    _spec(
        "vis-czi-overview-section-count",
        OVERVIEW,
        "count",
        "This is an overview scan of a slide with brain sections. How many sections are on it?",
        lambda f, m: g.integer(f["section_count"]),
        "section_count (czifile; DAPI above 0.7 × Otsu at pyramid level 2, the same at 0.5–1.0 ×)",
        answer_hint="the number of sections",
    ),
    _spec(
        "vis-czi-overview-hole",
        OVERVIEW,
        "locate",
        "One of the brain sections on this overview scan has a large round hole in it (a bubble or a "
        "missing piece). Where is the hole? Give a point in its middle.",
        lambda f, m: g.point(f["hole_point"], mask=_mask(f, "hole_point")),
        "hole_point (czifile; largest enclosed background region at pyramid level 2, dilated by 2 level pixels)",
        answer_hint=POINT_HINT,
    ),
    _spec(
        "vis-czi-slide-empty-corners",
        ZEISS_SLIDE,
        "artifact",
        "Parts of this slide scan were never imaged: the scanner left some areas empty. In the first scene, "
        "which corners of the scanned area are empty?",
        lambda f, m: g.choice(f["empty_corners"], list(CORNERS), aliases=CORNER_ALIASES, multiple=True),
        "empty_corners (czifile; share of zero pixels per corner quarter-block of the coarsest level)",
        answer_hint="the empty corners, e.g. top-left, bottom-right",
    ),
    _spec(
        "vis-dcimg-bead-best-focus",
        BEADS[0],
        "focus",
        "These camera files are one focus series of a bead sample: each file was recorded at a different "
        "z position, in the order of their names. Which file is in best focus?",
        lambda f, m: g.choice(
            [BEAD_STAGED[f["best_focus_files__evidence"]["files"].index(n)] for n in f["best_focus_files"]],
            BEAD_STAGED,
            aliases={n: [n, n.rsplit(".", 1)[0]] for n in BEAD_STAGED},  # the name or its stem
        ),
        "best_focus_files (dcimg; peak of Laplacian variance, normalised variance and Tenengrad, ± 1 file; "
        "staged as sample_01.dcimg ... sample_11.dcimg in the corpus order)",
        stage_as=BEAD_STAGED[0],
        extra=[(c, s) for c, s in zip(BEADS[1:], BEAD_STAGED[1:], strict=True)],
        answer_hint="the file name",
    ),
    _spec(
        "vis-nd2-zstack-best-focus",
        KARL,
        "focus",
        "In the blue RNA channel of this z-stack, which z-slice is in best focus? Count slices from 1.",
        lambda f, m: g.number(
            sum(f["best_focus_z"]) / len(f["best_focus_z"]), None, abs_=(len(f["best_focus_z"]) - 1) / 2 + 0.01
        ),
        "best_focus_z (nd2; peaks of Laplacian variance, normalised variance and Tenengrad and the slices between)",
        answer_hint="the z-slice number (counting from 1)",
    ),
    _spec(
        "vis-nd2-position-cell",
        BUT3,
        "locate",
        "At the third stage position (counting from 1) there is one cell in the DiO fluorescence channel. "
        "Where is it? Give the centre of the cell.",
        lambda f, m: g.point(f["position3_cell_point"], mask=_mask(f, "position3_cell_point")),
        "position3_cell_point (nd2; largest object above Otsu, mask dilated by 10 pixels)",
        answer_hint="the point as x, y in pixels (origin at the top-left of the image), e.g. x=120, y=340",
    ),
    _spec(
        "vis-nd2-timelapse-channel-interval",
        POR,
        "blank",
        "In this time-lapse the DiI fluorescence channel looks empty in most frames. How often was it "
        "actually recorded: every how many time points?",
        lambda f, m: g.integer(f["dii_interval"]),
        "dii_interval (nd2; time points with any non-zero DiI pixel, the same at every position)",
        answer_hint="the number of time points between recordings",
    ),
    _spec(
        "vis-hcs-nuclei-count",
        HARMONY,
        "count",
        "This plate folder holds one field of view of Hoechst-stained cells. About how many nuclei are in the field?",
        lambda f, m: g.number(f["nuclei_count"], None, rel=NUCLEI_TOL),
        "nuclei_count (tifffile; threshold + label, median over σ, threshold and size grid; ± 20 %)",
        stage_as="sample_plate",
        answer_hint="the approximate number of nuclei",
    ),
    _spec(
        "vis-ephys-abf-mains-hum",
        ABF_MAINS,
        "artifact",
        "Is the membrane-potential recording in this file contaminated by mains (power-line) hum? If it "
        "is, at what frequency?",
        lambda f, m: g.number(f["line_frequency_hz"], "Hz", abs_=1),
        "line_frequency_hz (pyabf + scipy Welch spectrum of channel 1)",
        answer_hint="the hum frequency with its unit, or no",
    ),
    _spec(
        "vis-chrom-ch-peak-count",
        CHROM,
        "count",
        "How many clear peaks does this chromatogram show? Leave out small bumps under 5 % of the "
        "tallest peak's height.",
        lambda f, m: g.integer(f["peak_count"]),
        "peak_count (rainbow-api black box + scipy find_peaks at 2–20 % prominence)",
        answer_hint="the number of peaks",
    ),
    _spec(
        "vis-plate-blank-column",
        PLATE_COLUMN,
        "blank",
        "One column of this plate reads at background level from top to bottom, while the other columns "
        "show signal in their upper rows. Which column is it?",
        lambda f, m: g.integer(f["blank_column"]),
        "blank_column (allotropy)",
        answer_hint="the column number",
    ),
]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true", help="exit 1 if evals/facts/visual.json is out of date")
    ap.add_argument("--only", nargs="*", help="recompute only these facts (name or corpus_id:name)")
    args = ap.parse_args()
    text = render(compute(args.only))
    if args.check:
        if not OUT.exists() or OUT.read_text(encoding="utf-8") != text:
            print("evals/facts/visual.json is out of date; run evals/visual.py", file=sys.stderr)
            return 1
        print("evals/facts/visual.json is up to date")
        return 0
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(text, encoding="utf-8")
    print(f"wrote {OUT.relative_to(ROOT)}: {len(FACTS)} facts")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
