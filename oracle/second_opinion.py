"""Second-opinion plane hashes from a reader independent of the primary oracle (docs/assurance.md,
"differential cross-checks"). The primary oracles (oracle/gen.py) use czifile and liffile; this
script runs a second, independently written reader on the same development files and records
its plane hashes in the same shape, so the corpus test `second_opinions_agree` compares
OpenReadout against both:

  .czi -> pylibCZIrw (LGPL; ZEISS's libCZI; run as a black box)
  .lif -> readlif (GPL-3.0; run as a black box)

Hashes are xxh3-128 of the plane's little-endian samples, planes in (c, z, t) order, at most
ORACLE_MAX_PLANES (64) per image, as in gen.py. RGB samples are hashed interleaved in R, G, B
order (pylibCZIrw returns B, G, R and is reordered). Mosaic (tile-scan) LIF images are skipped:
readlif returns tiles, not the stitched plane.

Also Bio-Formats (GPL, black box) for ND2, TIFF-family, MRC, DM and OIR files; images are
matched by series index, and a file whose series Bio-Formats groups differently from us is
reported as skipped by the corpus test.

Usage: uv run python second_opinion.py [ID_SUBSTRING|""] [FORMATS]   (development inputs only; never held-out)
Writes corpus/oracle/second/<id>.json.
"""
import json, os, sys, tomllib
from pathlib import Path
import numpy as np, xxhash

ROOT = Path(__file__).resolve().parent.parent
FILES = Path(os.environ.get("OPENREADOUT_CORPUS_DIR", ROOT / "corpus" / "files"))
OUT = ROOT / "corpus" / "oracle" / "second"
MAX_PLANES = int(os.environ.get("ORACLE_MAX_PLANES", "64"))
MAX_PIXELS = int(os.environ.get("ORACLE_MAX_PIXELS", str(120_000_000)))


def h(a: np.ndarray) -> str:
    a = np.ascontiguousarray(a)
    if a.dtype.byteorder == ">":
        a = a.byteswap().view(a.dtype.newbyteorder("<"))
    return xxhash.xxh3_128_hexdigest(a.tobytes())


def czi(p: Path) -> dict:
    from importlib.metadata import version
    from pylibCZIrw import czi as pyczi
    images = []
    with pyczi.open_czi(str(p)) as d:
        box = d.total_bounding_box_no_pyramid
        scenes = d.scenes_bounding_rectangle_no_pyramid
        rects = [scenes[k] for k in sorted(scenes)] if scenes else [d.total_bounding_rectangle_no_pyramid]
        keys = sorted(scenes) if scenes else [None]
        C = box.get("C", (0, 1))[1] - box.get("C", (0, 1))[0]
        Z = box.get("Z", (0, 1))[1] - box.get("Z", (0, 1))[0]
        T = box.get("T", (0, 1))[1] - box.get("T", (0, 1))[0]
        for i, (k, r) in enumerate(zip(keys, rects)):
            img = {"index": i, "size_x": max(r.w, 0), "size_y": max(r.h, 0), "planes": []}
            if r.w <= 0 or r.h <= 0:
                img["skip"] = f"pylibCZIrw reports an empty bounding rectangle ({r.w} x {r.h})"
                images.append(img)
                continue
            if r.w * r.h > MAX_PIXELS:
                img["skip"] = "larger than ORACLE_MAX_PIXELS"
                images.append(img)
                continue
            n = 0
            for c in range(C):
                for z in range(Z):
                    for t in range(T):
                        if n >= MAX_PLANES:
                            break
                        kw = {"plane": {"C": c, "Z": z, "T": t}, "roi": (r.x, r.y, r.w, r.h)}
                        if k is not None:
                            kw["scene"] = k
                        a = d.read(**kw)
                        if a.ndim == 3 and a.shape[2] == 1:
                            a = a[:, :, 0]
                        elif a.ndim == 3 and a.shape[2] == 3:
                            a = a[:, :, ::-1]  # B, G, R -> R, G, B
                        img["planes"].append({"c": c, "z": z, "t": t, "xxh3": h(a)})
                        n += 1
            images.append(img)
    return {"reader": f"pylibCZIrw {version('pylibCZIrw')} (LGPL, black box)", "images": images}


def lif(p: Path) -> dict:
    from readlif.reader import LifFile
    import readlif
    f = LifFile(str(p))
    images = []
    for i, im in enumerate(f.get_iter_image()):
        x, y, z, t, m = im.dims
        img = {"index": i, "name": im.name, "size_x": x, "size_y": y, "planes": []}
        if m > 1:
            img["skip"] = "mosaic: readlif returns tiles"
            images.append(img)
            continue
        n = 0
        try:
            for c in range(im.channels):
                for zz in range(z):
                    for tt in range(t):
                        if n >= MAX_PLANES:
                            break
                        a = np.asarray(im.get_frame(z=zz, t=tt, c=c))
                        img["planes"].append({"c": c, "z": zz, "t": tt, "xxh3": h(a)})
                        n += 1
        except Exception as e:  # readlif does not read every layout; record it and move on
            img["skip"] = f"readlif: {type(e).__name__}: {e}"
            img["planes"] = []
        images.append(img)
    return {"reader": f"readlif {getattr(readlif, '__version__', '?')} (GPL-3.0, black box)", "images": images}


BF_MAX_BYTES = int(os.environ.get("ORACLE_BF_MAX_BYTES", str(400 << 20)))


def bf_planes(gen, p: Path, info: dict, tmp: Path) -> list:
    """gen._bf_planes, but colour pages (bfconvert writes RGB planar, samples first) are hashed
    with interleaved samples, the layout OpenReadout and the primary oracles use."""
    import subprocess, tifffile
    out = tmp / f"s{info['series']}.ome.tif"
    subprocess.run([gen._bftools("bfconvert"), "-no-upgrade", "-overwrite", "-series", str(info["series"]), str(p), str(out)],
                   capture_output=True, text=True, timeout=7200, check=True)
    spp = info.get("samples", 1)
    C = info.get("size_c", 1) if spp == 1 else 1
    Z, T = info.get("size_z", 1), info.get("size_t", 1)
    planes = []
    with tifffile.TiffFile(out) as tf:
        order = [(c, z, t) for c in range(C) for z in range(Z) for t in range(T)]
        for k in gen._spread(len(order), MAX_PLANES):
            c, z, t = order[k]
            size, idx = {"C": C, "Z": Z, "T": T}, {"C": c, "Z": z, "T": t}
            page, stride = 0, 1
            for ax in info.get("dimension_order", "XYCZT")[2:]:
                page += idx[ax] * stride
                stride *= size[ax]
            a = tf.pages[page].asarray()
            if spp > 1 and a.ndim == 3 and a.shape[0] == spp:
                a = np.moveaxis(a, 0, -1)
            planes.append({"c": c, "z": z, "t": t, "xxh3": h(a)})
    out.unlink()
    return planes


def bioformats(p: Path) -> dict:
    """Bio-Formats 8.5 (GPL; bfconvert/showinf run as black boxes from oracle/bftools, source never
    read): one image per Bio-Formats series, planes hashed from bfconvert's output with gen.py's
    helpers (same (c, z, t) order and plane selection as the primary oracles)."""
    import tempfile
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import gen
    if p.stat().st_size > BF_MAX_BYTES:
        return {"error": "larger than ORACLE_BF_MAX_BYTES"}
    series = [s for s in gen._bf_series(gen._showinf(p, "-nometa")) if s.get("thumbnail") != "true"]
    images = []
    with tempfile.TemporaryDirectory() as td:
        for i, s in enumerate(series):
            img = {"index": i, "size_x": s["width"], "size_y": s["height"], "planes": []}
            try:
                img["planes"] = bf_planes(gen, p, s, Path(td))
            except Exception as e:
                img["skip"] = f"bfconvert: {type(e).__name__}: {e}"
            images.append(img)
    return {"reader": "Bio-Formats 8.5.0 (GPL, black box)", "images": images}


def main():
    only = sys.argv[1] if len(sys.argv) > 1 and sys.argv[1] else None
    m = tomllib.load(open(ROOT / "corpus" / "manifest.toml", "rb"))["file"]
    OUT.mkdir(parents=True, exist_ok=True)
    readers = {"czi": czi, "lif": lif, "nd2": bioformats, "tiff": bioformats, "mrc": bioformats,
               "dm": bioformats, "oir": bioformats}
    formats = sys.argv[2].split(",") if len(sys.argv) > 2 else list(readers)
    for e in m:
        if e.get("role") != "input" or e.get("tier") == "heldout" or e["format"] not in formats:
            continue
        if only and only not in e["id"]:
            continue
        p = FILES / e["filename"]
        if not p.exists() or p.is_dir() or (e["format"] in ("czi", "lif") and p.suffix.lower() not in (".czi", ".lif")):
            continue
        try:
            o = readers[e["format"]](p)
        except Exception as ex:
            o = {"error": f"{type(ex).__name__}: {ex}"}
        o = {"id": e["id"], "file": e["filename"], **o}
        (OUT / f"{e['id']}.json").write_text(json.dumps(o, indent=1) + "\n")
        n = sum(len(i.get("planes", [])) for i in o.get("images", []))
        print(f"{e['id']}: {o.get('error') or f'{n} planes'}", flush=True)


if __name__ == "__main__":
    main()
