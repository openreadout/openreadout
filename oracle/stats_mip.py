"""Ground truth for `stats --mip z`: maximum-intensity projections along z with NumPy.

For each corpus file every image (scene / stage position) is read with an independent reader
(czifile, nd2; BSD), projected with `max(axis=Z)` per channel and time point, and summarized:
count, min, max, mean, population std, median. Writes corpus/oracle/stats/mip.json. Run from
oracle/: `uv run python stats_mip.py` (or ../oracle/.venv/bin/python stats_mip.py).
"""

import json
import os
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
FILES = Path(os.environ.get("OPENREADOUT_CORPUS_DIR", ROOT / "corpus" / "files"))
OUT = ROOT / "corpus" / "oracle" / "stats" / "mip.json"


def summarize(p: np.ndarray) -> dict:
    x = p.astype("float64").ravel()
    return {
        "count": int(x.size),
        "min": float(x.min()),
        "max": float(x.max()),
        "mean": float(x.mean()),
        "std": float(x.std()),
        "median": float(np.percentile(x, 50)),
    }


def czi(name: str) -> list[dict]:
    """[{image, c, t, stats}] for every scene."""
    import czifile

    out = []
    with czifile.CziFile(FILES / name) as f:
        scenes = [f.scenes[k] for k in f.scenes] or [f]
        for i, s in enumerate(scenes):
            a = np.asarray(s.asarray())
            dims = list(s.dims)
            get = {d: (dims.index(d) if d in dims else None) for d in "CZTYX"}
            for d in ("C", "T", "Z"):
                if get[d] is None:
                    a = a[np.newaxis]
                    dims.insert(0, d)
            a = np.moveaxis(a, [dims.index(d) for d in ("C", "T", "Z", "Y", "X")], [0, 1, 2, 3, 4])
            a = a.reshape(a.shape[:5])
            for c in range(a.shape[0]):
                for t in range(a.shape[1]):
                    out.append({"image": i, "c": c, "t": t, "stats": summarize(a[c, t].max(axis=0))})
    return out


def nd2(name: str) -> list[dict]:
    import nd2 as nd2lib

    out = []
    with nd2lib.ND2File(FILES / name) as f:
        a = np.asarray(f.asarray())
        dims = list(f.sizes)
        for d in ("P", "C", "T", "Z"):
            if d not in dims:
                a = a[np.newaxis]
                dims.insert(0, d)
        a = np.moveaxis(a, [dims.index(d) for d in ("P", "C", "T", "Z", "Y", "X")], range(6))
        for p in range(a.shape[0]):
            for c in range(a.shape[1]):
                for t in range(a.shape[2]):
                    out.append({"image": p, "c": c, "t": t, "stats": summarize(a[p, c, t].max(axis=0))})
    return out


CASES = {
    "zenodo7015307-Z-5-CH-2": ("zenodo7015307-Z-5-CH-2.czi", czi),
    "aics-ND2-dims-p4z5t3c2y32x32": ("aics-ND2-dims-p4z5t3c2y32x32.nd2", nd2),
}


def main() -> None:
    out = {"how": "per image, channel and time point: max over z per pixel, then NumPy float64 statistics", "files": {}}
    for cid, (name, fn) in CASES.items():
        if not (FILES / name).exists():
            print("missing", name)
            continue
        out["files"][cid] = {"filename": name, "reader": fn.__name__, "projections": fn(name)}
        print(cid, len(out["files"][cid]["projections"]), "projections")
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(out, indent=1) + "\n")
    print("wrote", OUT)


if __name__ == "__main__":
    main()
