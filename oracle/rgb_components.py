"""Ground truth for per-colour-component statistics of RGB images (`stats` → `components[]`).

For each corpus file the samples of every image and channel are read with an independent
reader (czifile, nd2, tifffile; all BSD) and split by colour component with NumPy: count, min,
max, mean, population std, median (NumPy's default linear percentile) and the number of samples at
the type's maximum. Every reader returns R, G, B order (czifile swaps the CZI's stored B, G, R, as
OpenReadout does).

Writes corpus/oracle/stats/rgb_components.json. Run from oracle/:
    uv run python rgb_components.py   (or ../oracle/.venv/bin/python rgb_components.py)
"""

import json
import os
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
FILES = Path(os.environ.get("OPENREADOUT_CORPUS_DIR", ROOT / "corpus" / "files"))
OUT = ROOT / "corpus" / "oracle" / "stats" / "rgb_components.json"


def comp_stats(a: np.ndarray) -> list[dict]:
    """a: (..., S) samples; one dict per component."""
    out = []
    top = np.iinfo(a.dtype).max if np.issubdtype(a.dtype, np.integer) else None
    for k in range(a.shape[-1]):
        x = a[..., k].astype("float64").ravel()
        out.append(
            {
                "count": int(x.size),
                "min": float(x.min()),
                "max": float(x.max()),
                "mean": float(x.mean()),
                "std": float(x.std()),
                "median": float(np.percentile(x, 50)),
                "at_type_max": None if top is None else int((a[..., k] == top).sum()),
            }
        )
    return out


def czi_images(name: str) -> list[np.ndarray]:
    """One (planes..., Y, X, S) array per scene, R, G, B order."""
    import czifile

    arrs = []
    with czifile.CziFile(FILES / name) as c:
        scenes = [c.scenes[k] for k in c.scenes] or [c]
        for s in scenes:
            a = np.asarray(s.asarray())
            dims = list(s.dims)
            a = np.moveaxis(a, dims.index("S"), -1)
            # czifile swaps the stored B, G, R (A) to R, G, B (A) on read.
            arrs.append(a)
    return arrs


def nd2_images(name: str) -> list[np.ndarray]:
    import nd2

    with nd2.ND2File(FILES / name) as f:
        a = np.asarray(f.asarray())
        assert f.sizes.get("S") == 3, f.sizes
        axis = list(f.sizes).index("S")
        # stored order is B, G, R (see oracle/gen.py `nd2_`): reverse to R, G, B like the reader
        a = np.moveaxis(a, axis, -1)[..., ::-1]
        if "P" in f.sizes:
            p = list(f.sizes).index("P")
            return [np.take(a, i, axis=p) for i in range(f.sizes["P"])]
        return [a]


def tiff_images(name: str) -> list[np.ndarray]:
    import tifffile

    with tifffile.TiffFile(FILES / name) as t:
        s = t.series[0]
        a = s.asarray()
        axes = s.axes
        return [np.moveaxis(a, axes.index("S"), -1)]


CASES = {
    "aics-RGB-8bit": ("aics-RGB-8bit.czi", czi_images),
    "openslide-zeiss-5-slidepreview-zstd0": ("openslide-zeiss-5-slidepreview-zstd0.czi", czi_images),
    "aics-ND2-dims-rgb": ("aics-ND2-dims-rgb.nd2", nd2_images),
}


def main() -> None:
    out = {
        "how": "per image, every sample of each colour component (R, G, B) over all planes; NumPy float64",
        "files": {},
    }
    for cid, (name, fn) in CASES.items():
        if not (FILES / name).exists():
            print("missing", name)
            continue
        imgs = fn(name)
        out["files"][cid] = {
            "filename": name,
            "reader": fn.__name__.split("_")[0],
            "images": [comp_stats(a) for a in imgs],
        }
        print(cid, [[round(c["mean"], 4) for c in i] for i in out["files"][cid]["images"]])
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(out, indent=1) + "\n")
    print("wrote", OUT)


if __name__ == "__main__":
    main()
