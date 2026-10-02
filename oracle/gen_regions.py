"""Ground truth for region and pyramid-level reads (`Dataset::read_region`, `--region`, `--level`).

For each corpus file below, a few rectangles of a few planes at several resolution levels are
read with an independent reader and summarised:

  xxh3   xxh3-128 of the region's little-endian samples, (Y, X[, S]) row-major, as the corpus
         harness hashes planes;
  mean, std, min, max of all samples;
  grid   the mean of each cell of an 8 x 8 grid over the region (all samples of the cell), so
         lossy codecs (JPEG) can be compared within a tolerance while a misplaced tile still fails.

Readers (third-party, run as black boxes):

  CZI            czifile (BSD-3-Clause): `scene(roi=...)` at level 0, the composite of each pyramid
                 level's subblocks placed in the level grid pylibCZIrw (LGPL, black box) shows at
                 `read(roi, zoom=1/f)`; pylibCZIrw as the second reader
  TIFF flavours  tifffile (BSD-3-Clause) `aszarr(series, level)` sliced with zarr (MIT); JPEG 2000,
                 WebP and JPEG XL tiles decoded by imagecodecs (BSD-3-Clause: OpenJPEG, libwebp,
                 libjxl)
  Harmony plate  tifffile reading the field's per-channel TIFF (a screening field: no tiles or
                 pyramid, the generic crop path)
  VSI            Bio-Formats 8.5.0 `bfconvert -series S -crop X,Y,W,H` (GPL, black box)
  Imaris .ims    h5py (BSD-3-Clause) + hdf5plugin (MIT) slicing `DataSet/ResolutionLevel L/...`
  MIRAX .mrxs    OpenSlide (LGPL, black box) `read_region`, alpha composited over the slide's
                 background colour; besides the standard rectangles, 512 x 512 windows centred on
                 camera photos that hold tissue (the standard ones often fall on empty glass)
  OME-Zarr       zarr-python 3 (MIT) slicing the multiscales dataset of level L
  ND2            nd2 (BSD-3-Clause) to_xarray sliced (a format without tiles: the generic path)

Regions are in the pixel coordinates of the level (level 0 = full resolution), relative to the
image's top-left pixel. Output: `corpus/oracle/regions/<manifest id>.json`.

    cd oracle && uv run python gen_regions.py [id ...]     # default: every file below present
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Sequence, Tuple

import numpy as np
import xxhash

ROOT = Path(__file__).resolve().parents[1]
CORPUS = Path(os.environ.get("OPENREADOUT_CORPUS_DIR", ROOT / "corpus" / "files"))
OUT = ROOT / "corpus" / "oracle" / "regions"

# Largest level (Y * X) read whole to crop from; bigger levels are only read by region.
MAX_WHOLE_PIXELS = int(os.environ.get("ORACLE_MAX_WHOLE_PIXELS", str(64_000_000)))


def xxh3(a: np.ndarray) -> str:
    le = np.ascontiguousarray(a).astype(a.dtype.newbyteorder("<"), copy=False)
    return xxhash.xxh3_128_hexdigest(le.tobytes())


def summary(a: np.ndarray) -> Dict[str, Any]:
    f = a.astype(np.float64)
    h, w = a.shape[0], a.shape[1]
    ys = np.linspace(0, h, 9).astype(int)
    xs = np.linspace(0, w, 9).astype(int)
    grid = []
    for i in range(8):
        row = []
        for j in range(8):
            cell = f[ys[i]:max(ys[i + 1], ys[i] + 1), xs[j]:max(xs[j + 1], xs[j] + 1)]
            row.append(round(float(cell.mean()), 4) if cell.size else None)
        grid.append(row)
    return {
        "xxh3": xxh3(a),
        "mean": float(f.mean()),
        "std": float(f.std()),
        "min": float(f.min()),
        "max": float(f.max()),
        "grid": grid,
    }


def pick_regions(w: int, h: int, tw: int, th: int, whole: bool = True) -> List[Tuple[int, int, int, int]]:
    """Centre 512 x 512, a tile-aligned corner, a rectangle straddling tile boundaries, the
    bottom-right edge, and the whole level when it is small."""
    out: List[Tuple[int, int, int, int]] = []

    def add(x: int, y: int, rw: int, rh: int) -> None:
        rw, rh = min(rw, w), min(rh, h)
        x, y = min(max(x, 0), w - rw), min(max(y, 0), h - rh)
        r = (int(x), int(y), int(rw), int(rh))
        if rw > 0 and rh > 0 and r not in out:
            out.append(r)

    add((w - 512) // 2, (h - 512) // 2, 512, 512)
    add(0, 0, 256, 256)
    tw, th = tw or 256, th or 256
    add(tw + tw // 2 - 37, th + th // 2 - 11, 300, 200)
    add(w - 333, h - 211, 333, 211)
    if whole and w * h <= 4_000_000:
        add(0, 0, w, h)
    return out


class Target:
    """One image of one corpus file and how to read regions of it."""

    def __init__(self, image: int, levels: List[Tuple[int, int, int, int, int]], planes: List[Tuple[int, int, int]],
                 read: Callable[[int, int, int, int, int, int, int, int], np.ndarray], use_levels: Sequence[int],
                 whole: bool = True,
                 alt: Optional[Tuple[str, Callable[[int, int, int, int, int, int, int, int], Optional[np.ndarray]]]] = None,
                 disputed: Optional[Callable[[int, Tuple[int, int, int, int]], Optional[str]]] = None,
                 extra_regions: Optional[Callable[[int, int, int], List[Tuple[int, int, int, int]]]] = None) -> None:
        self.image = image
        self.levels = levels  # (level, w, h, tile_w, tile_h)
        self.planes = planes  # (c, z, t)
        self.read = read  # (level, c, z, t, x, y, w, h) -> (h, w[, s])
        self.use_levels = use_levels
        self.whole = whole
        # A second reader (name, read) consulted when it can read the rectangle; when the two
        # disagree both hashes are recorded and a match with either is accepted.
        self.alt = alt
        # (level, region) -> why the oracle's value there is not taken as ground truth.
        self.disputed = disputed
        # (level, w, h) -> rectangles read besides pick_regions' standard ones.
        self.extra_regions = extra_regions


# ----- readers ------------------------------------------------------------------------------------


def tiff_targets(path: Path, series_images: Sequence[Tuple[int, int]], use_levels: Callable[[int], Sequence[int]],
                 plane_list: Optional[List[Tuple[int, int, int]]] = None) -> Tuple[str, List[Target]]:
    import tifffile
    import zarr

    tf = tifffile.TiffFile(path)
    targets = []
    for series, image in series_images:
        s = tf.series[series]
        axes = s.axes
        # Planar-separate samples without a C axis are channels to OpenReadout (one sample per
        # plane), so sample `c` is read for channel `c`.
        samples_as_channels = "S" in axes and "C" not in axes and int(s.keyframe.planarconfig) == 2
        levels = []
        for li, lv in enumerate(s.levels):
            shp = dict(zip(axes, lv.shape))
            kf = lv.keyframe
            levels.append((li, shp["X"], shp["Y"], kf.tilewidth or shp["X"], kf.tilelength or (kf.rowsperstrip or shp["Y"])))
        stores: Dict[int, Any] = {}

        def read(level: int, c: int, z: int, t: int, x: int, y: int, w: int, h: int,
                 series: int = series, axes: str = axes, stores: Dict[int, Any] = stores,
                 s_is_c: bool = samples_as_channels) -> np.ndarray:
            if level not in stores:
                stores[level] = zarr.open(tf.aszarr(series=series, level=level), mode="r")
            arr = stores[level]
            key = []
            for ax in axes:
                key.append({"C": c, "Z": z, "T": t, "Y": slice(y, y + h), "X": slice(x, x + w),
                            "S": c if s_is_c else slice(None)}.get(ax, 0))
            return np.asarray(arr[tuple(key)])

        shp0 = dict(zip(axes, s.shape))
        planes = plane_list or [(0, 0, 0)]
        n_c = shp0["S"] if samples_as_channels else shp0.get("C", 1)
        if plane_list is None and n_c > 1:
            planes.append((n_c - 1, shp0.get("Z", 1) - 1, 0))
        targets.append(Target(image, levels, planes, read, use_levels(len(levels))))
    return f"tifffile {tifffile.__version__} + zarr {zarr.__version__}", targets


def czi_targets(path: Path, scenes: Sequence[int], use_levels: Callable[[int], Sequence[int]]) -> Tuple[str, List[Target]]:
    """czifile at level 0, pyramid levels as `czi_levels.Level` defines them (czifile's pixels
    in pylibCZIrw's level grid); pylibCZIrw's own reads as the second reader (it resamples, and
    where a subblock's grid position is fractional it leaves the subblock's first row or column
    to its neighbour or the background: docs/provenance/czi.md, 2026-09-24)."""
    import czifile

    import czi_levels

    f = czifile.CziFile(path)
    targets = []
    keys = list(f.scenes.keys())
    for s in scenes:
        sc = f.scenes[keys[s]]
        dims = list(sc.dims)
        start = dict(zip(sc.dims, sc.start))
        sizes = dict(sc.sizes)
        bx, by = sc.bbox[0], sc.bbox[1]
        groups = czi_levels.level_groups(sc)
        factors = [fk for fk, _ in groups]
        levels = [(0, sizes.get("X", 1), sizes.get("Y", 1), 0, 0)] + [
            (k + 1, w, h, 0, 0) for k, (w, h) in enumerate(czi_levels.level_sizes(sc, groups))]
        pyr = [czi_levels.Level(f, sc, fk, es) for fk, es in groups]

        def read(level: int, c: int, z: int, t: int, x: int, y: int, w: int, h: int,
                 sc: Any = sc, dims: List[str] = dims, bx: int = bx, by: int = by,
                 start: Dict[str, int] = start, pyr: List[Any] = pyr) -> np.ndarray:
            if level > 0:
                return pyr[level - 1].read(c, z, t, x, y, w, h)
            # czifile selects by absolute coordinate: the dimension's start plus the index.
            sel = {d: start.get(d, 0) + v for d, v in (("C", c), ("Z", z), ("T", t)) if d in dims}
            try:
                img = sc(roi=(bx + x, by + y, w, h), **sel)
            except ValueError as e:
                if "matches no subblocks" not in str(e):
                    raise
                # No subblock under the rectangle: czifile's fill value (0) everywhere.
                s_ = dict(sc.sizes).get("S", 1)
                return np.zeros((h, w, s_) if s_ > 1 else (h, w), dtype=sc.dtype)
            arr = np.asarray(img.asarray(maxworkers=1))
            key2 = [slice(None) if d in "YXS" else 0 for d in img.dims]
            return arr[tuple(key2)]

        C, Z = sizes.get("C", 1), sizes.get("Z", 1)
        planes = [(0, 0, 0)]
        if C > 1 or Z > 1:
            planes.append((C - 1, Z - 1, 0))
        targets.append(Target(s, levels, planes, read, use_levels(len(levels)),
                              alt=("pylibCZIrw (LGPL, black box)", _libczi_reader(path, bx, by, start, factors))))
    return f"czifile {czifile.__version__}; level geometry from pylibCZIrw", targets


def _libczi_reader(path: Path, bx: int, by: int, start: Dict[str, int],
                   factors: Sequence[int] = ()) -> Callable[..., Optional[np.ndarray]]:
    """Rectangles through pylibCZIrw, the ZEISS libCZI binding (LGPL; run as a black box).
    Level 0: czifile does not apply subblock valid-pixel masks at full
    resolution in some mosaics (zenodo10577621-Young-mouse), where libCZI does. Level k: the
    level-0 rectangle `(x, y, w, h) * f` read at zoom `1/f`, which libCZI composes from the
    pyramid subblocks of that factor."""

    def read(level: int, c: int, z: int, t: int, x: int, y: int, w: int, h: int) -> Optional[np.ndarray]:
        from pylibCZIrw import czi as pyczi

        f = 1 if level == 0 else factors[level - 1]
        plane = {"C": start.get("C", 0) + c, "Z": start.get("Z", 0) + z, "T": start.get("T", 0) + t}
        with pyczi.open_czi(str(path)) as d:
            a = d.read(roi=(bx + x * f, by + y * f, w * f, h * f), plane=plane, zoom=1.0 / f)
        if a.shape[:2] != (h, w):
            raise RuntimeError(f"pylibCZIrw returned {a.shape[:2]} for a {h}x{w} region at 1/{f}")
        if a.shape[-1] == 3:
            return a[..., ::-1]  # BGR -> RGB
        return a[..., 0]

    return read


def ims_targets(path: Path) -> Tuple[str, List[Target]]:
    import h5py
    import hdf5plugin  # noqa: F401  (registers the LZ4 filter)

    f = h5py.File(path, "r")

    def text(v: Any) -> str:
        if isinstance(v, np.ndarray):
            return b"".join(bytes(x) for x in v.tolist()).decode(errors="replace")
        return v.decode(errors="replace") if isinstance(v, bytes) else str(v)

    names = sorted((int(k.split()[-1]) for k in f["DataSet"].keys() if k.startswith("ResolutionLevel")))
    levels = []
    for li, l in enumerate(names):
        a = f[f"DataSet/ResolutionLevel {l}/TimePoint 0/Channel 0"].attrs
        # Level extents: the channel group's ImageSizeX/Y (the stored arrays are padded to chunks).
        sx, sy = (int(text(a[k]).strip("\0 ")) for k in ("ImageSizeX", "ImageSizeY"))
        d = f[f"DataSet/ResolutionLevel {l}/TimePoint 0/Channel 0/Data"]
        levels.append((li, sx, sy, d.chunks[2], d.chunks[1]))

    def read(level: int, c: int, z: int, t: int, x: int, y: int, w: int, h: int) -> np.ndarray:
        d = f[f"DataSet/ResolutionLevel {names[level]}/TimePoint {t}/Channel {c}/Data"]
        return np.asarray(d[z, y:y + h, x:x + w])

    n_c = len([k for k in f["DataSet/ResolutionLevel 0/TimePoint 0"].keys() if k.startswith("Channel")])
    return f"h5py {h5py.__version__}", [Target(0, levels, [(0, 10, 0), (n_c - 1, 25, 0)], read, range(len(levels)))]


def zarr_targets(path: Path, group: str) -> Tuple[str, List[Target]]:
    import zarr

    import zipfile

    # Zenodo zips hold the `.zarr` folder at the archive root or inside one top-level folder.
    names = zipfile.ZipFile(path).namelist()
    top = ""
    if not any(n in ("zarr.json", ".zgroup", ".zattrs") for n in names):
        tops = {n.split("/", 1)[0] for n in names if "/" in n}
        if len(tops) == 1:
            top = tops.pop()
    root = zarr.open_group(zarr.storage.ZipStore(path, mode="r"), mode="r", path=top or None)
    g = root[group] if group else root
    attrs = dict(g.attrs)
    ms = (attrs.get("ome") or attrs)["multiscales"][0]
    axes = [a["name"] if isinstance(a, dict) else a for a in ms["axes"]]
    arrays = [g[d["path"]] for d in ms["datasets"]]
    levels = []
    for li, a in enumerate(arrays):
        shp = dict(zip(axes, a.shape))
        ch = dict(zip(axes, a.chunks))
        levels.append((li, shp["x"], shp["y"], ch["x"], ch["y"]))

    def read(level: int, c: int, z: int, t: int, x: int, y: int, w: int, h: int) -> np.ndarray:
        key = tuple({"c": c, "z": z, "t": t, "y": slice(y, y + h), "x": slice(x, x + w)}.get(ax, 0) for ax in axes)
        return np.asarray(arrays[level][key])

    shp0 = dict(zip(axes, arrays[0].shape))
    planes = [(0, 0, 0)]
    if shp0.get("z", 1) > 1:
        planes.append((0, shp0["z"] - 1, 0))
    return f"zarr {zarr.__version__}", [Target(0, levels, planes, read, range(len(levels)))]


def nd2_targets(path: Path) -> Tuple[str, List[Target]]:
    """No tiles: exercises the generic path (whole plane read, then cropped)."""
    import nd2

    f = nd2.ND2File(path)
    xa = f.to_xarray(delayed=True)
    dims = list(xa.dims)
    shp = dict(zip(dims, xa.shape))

    def read(level: int, c: int, z: int, t: int, x: int, y: int, w: int, h: int) -> np.ndarray:
        key = {"C": c, "Z": z, "T": t, "Y": slice(y, y + h), "X": slice(x, x + w)}
        sel = tuple(key.get(d, slice(None) if d == "S" else 0) for d in dims)
        return np.asarray(xa.data[sel])

    C, Z = shp.get("C", 1), shp.get("Z", 1)
    return f"nd2 {nd2.__version__}", [Target(0, [(0, shp["X"], shp["Y"], 0, 0)], [(0, 0, 0), (C - 1, Z - 1, 0)], read, [0])]


def vsi_targets(path: Path, image_levels: Sequence[Tuple[int, Sequence[int]]]) -> Tuple[str, List[Target]]:
    """Bio-Formats (GPL): run as a black box. Flattened series = one per (stack, resolution)."""
    import tifffile

    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import gen

    noflat = gen._bf_series(gen._showinf(path, "-noflat", "-nometa"))
    flat = gen._bf_series(gen._showinf(path, "-nometa"))
    targets = []
    flat_i = 0
    k_img = 0
    wanted = dict(image_levels)
    for s in noflat:
        nres = s.get("resolutions", 1)
        first = flat_i
        flat_i += nres
        if s.get("thumbnail") == "true":
            continue
        image = k_img
        k_img += 1
        if image not in wanted:
            continue
        levels = [(r, flat[first + r]["width"], flat[first + r]["height"], 512, 512) for r in range(nres)]

        def read(level: int, c: int, z: int, t: int, x: int, y: int, w: int, h: int, first: int = first) -> np.ndarray:
            # The slide stacks are single RGB planes (c = z = t = 0): no plane selection (bfconvert's
            # -channel refuses RGB series).
            del c, z, t
            with tempfile.TemporaryDirectory() as td:
                out = Path(td) / "r.tif"
                subprocess.run([gen._bftools("bfconvert"), "-no-upgrade", "-overwrite", "-series", str(first + level),
                                "-crop", f"{x},{y},{w},{h}", str(path), str(out)], capture_output=True, text=True, timeout=3600, check=True)
                return tifffile.imread(out)

        def disputed(level: int, r: Tuple[int, int, int, int], levels: List[Any] = levels) -> Optional[str]:
            _, lw, lh, _, _ = levels[level]
            if level == 0 and r[0] + r[2] == lw and r[1] + r[3] == lh:
                return ("Bio-Formats returns 0 in the unstored bottom-right corner of full resolution but the "
                        "background colour in the same corner of every other level and elsewhere at full "
                        "resolution; OpenReadout fills unstored tiles with the ETS background everywhere")
            return None

        targets.append(Target(image, levels, [(0, 0, 0)], read, wanted[image], whole=False, disputed=disputed))
    return "Bio-Formats 8.5.0 bfconvert -crop (GPL, black box)", targets


# ----- the files ----------------------------------------------------------------------------------


def harmony_targets(images: Path, image: int, field: str, channels: int) -> Tuple[str, List[Target]]:
    """Image `image` of a Harmony export is the field whose per-channel TIFFs are
    `<field>-ch<c+1>sk1fk1fl1.tiff` (one plane each)."""
    import tifffile

    arrays = [tifffile.imread(images / f"{field}-ch{c + 1}sk1fk1fl1.tiff") for c in range(channels)]
    h, w = arrays[0].shape

    def read(level: int, c: int, z: int, t: int, x: int, y: int, rw: int, rh: int) -> np.ndarray:
        return arrays[c][y:y + rh, x:x + rw]

    planes = [(0, 0, 0)] + ([(channels - 1, 0, 0)] if channels > 1 else [])
    return f"tifffile {tifffile.__version__}", [Target(image, [(0, w, h, w, h)], planes, read, [0])]


def mirax_targets(path: Path, use_levels: Callable[[int], Sequence[int]]) -> Tuple[str, List[Target]]:
    """MIRAX via OpenSlide (LGPL, black box). A fluorescence slide's RGB is split into one plane
    per filter as gen.py does (component 2 - STORING_CHANNEL_NUMBER)."""
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from gen import _mirax_storing_channels

    return openslide_targets(path, use_levels, _mirax_storing_channels(path))


def openslide_targets(path: Path, use_levels: Callable[[int], Sequence[int]],
                      storing: Optional[List[int]] = None) -> Tuple[str, List[Target]]:
    """A slide through OpenSlide (LGPL, black box): RGBA composited over the slide's background
    colour (white when it declares none), standard rectangles plus windows on tissue."""
    import openslide

    o = openslide.OpenSlide(str(path))
    props = o.properties
    bg = np.array([int(props.get("openslide.background-color", "FFFFFF")[i:i + 2], 16) for i in (0, 2, 4)], float)
    tw = int(props.get("openslide.level[0].tile-width", 256) or 256)
    th = int(props.get("openslide.level[0].tile-height", 256) or 256)
    levels = [(li, w, h, tw, th) for li, (w, h) in enumerate(o.level_dimensions)]
    # Camera photos with tissue: OpenSlide's bounds, sampled on a coarse level for dark pixels.
    lo = o.level_count - 1
    thumb = np.asarray(o.read_region((0, 0), lo, o.level_dimensions[lo])).astype(float)
    alpha = thumb[..., 3] / 255
    lum = (thumb[..., :3].mean(axis=2) * alpha + (1 - alpha) * bg.mean())
    dev = np.abs(lum - bg.mean())
    ys, xs = np.nonzero((dev > 12) & (alpha > 0.99))
    rng = np.random.default_rng(7)
    picks = [(int(xs[k]), int(ys[k])) for k in rng.choice(len(xs), size=min(3, len(xs)), replace=False)] if len(xs) else []
    f0 = o.level_downsamples[lo]

    def extra(level: int, w: int, h: int) -> List[Tuple[int, int, int, int]]:
        out = []
        for (px, py) in picks:
            cx = int(px * f0 / o.level_downsamples[level])
            cy = int(py * f0 / o.level_downsamples[level])
            rw, rh = min(512, w), min(512, h)
            out.append((min(max(cx - rw // 2, 0), w - rw), min(max(cy - rh // 2, 0), h - rh), rw, rh))
        return out

    def read(level: int, c: int, z: int, t: int, x: int, y: int, w: int, h: int) -> np.ndarray:
        ds = o.level_downsamples[level]
        rgba = np.asarray(o.read_region((int(round(x * ds)), int(round(y * ds))), level, (w, h))).astype(np.float64)
        a = rgba[..., 3:4] / 255.0
        rgb = np.floor(rgba[..., :3] * a + (1 - a) * bg + 0.5).clip(0, 255).astype(np.uint8)
        if storing is None:
            return rgb
        return np.ascontiguousarray(rgb[..., 2 - storing[c]])

    planes = [(0, 0, 0)] if storing is None else [(c, 0, 0) for c in range(len(storing))]
    return (f"OpenSlide {openslide.__library_version__} (openslide-python {openslide.__version__}; LGPL, black box)",
            [Target(0, levels, planes, read, use_levels(o.level_count), whole=True, extra_regions=extra)])


def last_and_mid(n: int) -> List[int]:
    return sorted({0, n // 2, n - 1})


FILES: Dict[str, Tuple[str, bool, Callable[[Path], Tuple[str, List[Target]]]]] = {
    # file name: (relative path, lossy, targets)
    "zenodo10577621-Young-mouse.czi": ("zenodo10577621-Young-mouse.czi", True,
                                       lambda p: czi_targets(p, [0], lambda n: [0, 5, n - 1])),
    "zenodo10577621-Kidney-RAC-3color.czi": ("zenodo10577621-Kidney-RAC-3color.czi", False,
                                             lambda p: czi_targets(p, [0], last_and_mid)),
    "aics-variable-scene-shape-first-scene-pyramid.czi": ("aics-variable-scene-shape-first-scene-pyramid.czi", False,
                                                          lambda p: czi_targets(p, [0], last_and_mid)),
    "aics-s-3-t-1-c-3-z-5.czi": ("aics-s-3-t-1-c-3-z-5.czi", False, lambda p: czi_targets(p, [1], lambda n: [0])),
    "openslide-aperio-CMU-1.svs": ("openslide-aperio-CMU-1.svs", True,
                                   lambda p: tiff_targets(p, [(0, 0)], lambda n: range(n))),
    "openslide-aperio-CMU-1-Small-Region.svs": ("openslide-aperio-CMU-1-Small-Region.svs", True,
                                                lambda p: tiff_targets(p, [(0, 0)], lambda n: range(n))),
    "ome-qptiff-HandEcompressed_Scan1.qptiff": ("ome-qptiff-HandEcompressed_Scan1.qptiff", True,
                                                lambda p: tiff_targets(p, [(0, 0)], lambda n: [0, 2, n - 1])),
    "openslide-hamamatsu-CMU-1.ndpi": ("openslide-hamamatsu-CMU-1.ndpi", True,
                                       lambda p: tiff_targets(p, [(0, 0)], lambda n: range(1, n))),
    "ome-subresolutions-retina_large.ome.tiff": ("ome-subresolutions-retina_large.ome.tiff", False,
                                                 lambda p: tiff_targets(p, [(0, 0)], lambda n: range(n), [(0, 20, 0), (1, 40, 0)])),
    "retina_large.ims": ("ome-imaris/retina_large.ims", False, ims_targets),
    "zenodo13305156-cardio-cycle1.zarr.zip": ("zenodo13305156-cardio-cycle1.zarr.zip", False,
                                              lambda p: zarr_targets(p, "B/03/0")),
    "ome-karl-sample-image.nd2": ("ome-karl-sample-image.nd2", False, nd2_targets),
    "Image_L2410_Part3_1_H+E_20X.vsi": ("zenodo8161864-vs200/Image_L2410_Part3_1_H+E_20X.vsi", True,
                                        lambda p: vsi_targets(p, [(2, [0, 3])])),
    "HE_Bone.vsi": ("zenodo17453126-he-bone/HE_Bone.vsi", True, lambda p: vsi_targets(p, [(3, [0, 4])])),
    # JPEG 2000 tiles (Aperio 33003 YCbCr, 33005 RGB; level 0 of CMU-1-JP2K is 4.5 GB: regions only)
    "openslide-aperio-JP2K-33003-1.svs": ("openslide-aperio-JP2K-33003-1.svs", True,
                                          lambda p: tiff_targets(p, [(0, 0)], lambda n: range(n))),
    "openslide-aperio-CMU-1-JP2K-33005.svs": ("openslide-aperio-CMU-1-JP2K-33005.svs", True,
                                              lambda p: tiff_targets(p, [(0, 0)], lambda n: [0, 1, n - 1])),
    "gdal-rgbsmall_WEBP_tiled.tif": ("gdal-rgbsmall_WEBP_tiled.tif", True,
                                     lambda p: tiff_targets(p, [(0, 0)], lambda n: range(n))),
    "gdal-rgbsmall_JXL_tiled_separate.tif": ("gdal-rgbsmall_JXL_tiled_separate.tif", True,
                                             lambda p: tiff_targets(p, [(0, 0)], lambda n: range(n))),
    # Leica SCN: the scanned region (image 1, 5 levels, level 0 is 4.2 GB: regions only) and the
    # 3-channel fluorescence region (image 2: one page per channel and level) with an overview
    "openslide-Leica-1.scn": ("openslide-Leica-1.scn", True,
                              lambda p: tiff_targets(p, [(1, 1)], lambda n: range(n))),
    "openslide-Leica-Fluorescence-1.scn": ("openslide-Leica-Fluorescence-1.scn", True,
                                           lambda p: tiff_targets(p, [(0, 0), (2, 2)], lambda n: range(n))),
    # Ventana BIF (DP 200): tiles on their stored grid, as tifffile reads them (level 0 is 1.5 GB)
    "openslide-Ventana-1.bif": ("openslide-Ventana-1.bif", True,
                                lambda p: tiff_targets(p, [(0, 0)], lambda n: [0, 1, n // 2, n - 1])),
    # 3DHISTECH MIRAX: camera photos placed at their positions (OpenSlide as the black box)
    "openslide-mirax-cmu-1-saved-1-16-zip/CMU-1-Saved-1_16.mrxs": ("openslide-mirax-cmu-1-saved-1-16-zip/CMU-1-Saved-1_16.mrxs", True,
                                                                    lambda p: mirax_targets(p, lambda n: range(n))),
    "openslide-mirax-cmu-1-saved-1-2-zip/CMU-1-Saved-1_2.mrxs": ("openslide-mirax-cmu-1-saved-1-2-zip/CMU-1-Saved-1_2.mrxs", True,
                                                                  lambda p: mirax_targets(p, lambda n: [0, 1, 3, n - 1])),
    "openslide-mirax-cmu-1-zip/CMU-1.mrxs": ("openslide-mirax-cmu-1-zip/CMU-1.mrxs", True,
                                             lambda p: mirax_targets(p, lambda n: [0, 2, 5, n - 1])),
    "openslide-mirax2-fluorescence-1-zip/Mirax2-Fluorescence-1.mrxs": ("openslide-mirax2-fluorescence-1-zip/Mirax2-Fluorescence-1.mrxs", True,
                                                                        lambda p: mirax_targets(p, lambda n: [0, 1, 4, n - 1])),
    "openslide-mirax2-fluorescence-2-zip/Mirax2-Fluorescence-2.mrxs": ("openslide-mirax2-fluorescence-2-zip/Mirax2-Fluorescence-2.mrxs", True,
                                                                        lambda p: mirax_targets(p, lambda n: [0, 3, 6, n - 1])),
    "openslide-mirax2-2-4-bmp-zip/Mirax2.2-4-BMP.mrxs": ("openslide-mirax2-2-4-bmp-zip/Mirax2.2-4-BMP.mrxs", True,
                                                          lambda p: mirax_targets(p, lambda n: [0, 2, 6, n - 1])),
    "openslide-mirax2-2-4-png-zip/Mirax2.2-4-PNG.mrxs": ("openslide-mirax2-2-4-png-zip/Mirax2.2-4-PNG.mrxs", True,
                                                          lambda p: mirax_targets(p, lambda n: [0, 1, 4, n - 1])),
    # Philips TIFF: OpenSlide renders unstored tiles transparent (white here), as openreadout
    # returns them; tifffile returns zeros there
    "openslide-Philips-1.tiff": ("openslide-Philips-1.tiff", True,
                                 lambda p: openslide_targets(p, lambda n: [0, 2, 5, n - 1])),
    "openslide-Philips-4.tiff": ("openslide-Philips-4.tiff", True,
                                 lambda p: openslide_targets(p, lambda n: [0, 1, 4, n - 1])),
    # a high-content screening field (A01 field 1, 4 channels)
    "hcs/harmony-idr0034/Images/Index.idx.xml": ("hcs/harmony-idr0034/Images/Index.idx.xml", False,
                                                 lambda p: harmony_targets(p.parent, 0, "r01c01f01p01", 4)),
}


def manifest_ids() -> Dict[str, str]:
    m = tomllib.loads((ROOT / "corpus" / "manifest.toml").read_text())
    return {f["filename"]: f["id"] for f in m["file"] if "filename" in f}


def run(key: str) -> Optional[Path]:
    rel, lossy, make = FILES[key]
    path = CORPUS / rel
    if not path.exists():
        print(f"skip {rel}: not in the corpus", file=sys.stderr)
        return None
    ident = manifest_ids().get(rel)
    if ident is None:
        raise SystemExit(f"{rel} is not in corpus/manifest.toml")
    reader, targets = make(path)
    out: Dict[str, Any] = {"id": ident, "file": rel, "reader": reader, "lossy": lossy, "regions": []}
    for tg in targets:
        by_level = {lv[0]: lv for lv in tg.levels}
        for level in tg.use_levels:
            _, w, h, tw, th = by_level[level]
            regs = pick_regions(w, h, tw, th, tg.whole and w * h <= MAX_WHOLE_PIXELS)
            if tg.extra_regions is not None:
                regs += [r for r in tg.extra_regions(level, w, h) if r not in regs]
            for (c, z, t) in tg.planes:
                for (x, y, rw, rh) in regs:
                    a = tg.read(level, c, z, t, x, y, rw, rh)
                    if a.shape[:2] != (rh, rw):
                        raise SystemExit(f"{rel}: oracle returned {a.shape} for region {(x, y, rw, rh)} of level {level}")
                    why = tg.disputed(level, (x, y, rw, rh)) if tg.disputed else None
                    entry = {"image": tg.image, "level": level, "level_size": [w, h], "c": c, "z": z, "t": t,
                             "region": [x, y, rw, rh], **summary(a)}
                    if why:
                        entry["disputed"] = why
                    if tg.alt is not None:
                        b = tg.alt[1](level, c, z, t, x, y, rw, rh)
                        if b is not None and xxh3(b) != entry["xxh3"]:
                            entry["xxh3_alt"] = xxh3(b)
                            entry["alt_reader"] = tg.alt[0]
                    out["regions"].append(entry)
                    print(f"{ident} image {tg.image} level {level} c={c} z={z} t={t} {x},{y},{rw},{rh}: mean {entry['mean']:.3f}", file=sys.stderr)
    OUT.mkdir(parents=True, exist_ok=True)
    dest = OUT / f"{ident}.json"
    dest.write_text(json.dumps(out, indent=1) + "\n")
    return dest


def main(argv: List[str]) -> None:
    keys = argv or list(FILES)
    for k in keys:
        if k not in FILES:
            raise SystemExit(f"unknown file {k}; known: {', '.join(FILES)}")
        run(k)


if __name__ == "__main__":
    main(sys.argv[1:])
