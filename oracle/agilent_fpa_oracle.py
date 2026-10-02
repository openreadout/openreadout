#!/usr/bin/env python
"""Ground truth for Agilent FT-IR imaging files (`.dat`, `.seq`, `.dms`, `.dmd`, `.drd` with
their `.bsp`/`.dmt` header), compared by `crates/openreadout-corpus-tests/tests/hyperspec_oracle/`.

Source, never our reader: agilent-format 0.4.7 (MIT, https://github.com/stuart-cls/python-agilent-file-formats),
run as a black box with `MAT=True` (image coordinates: the first row is the top):

- `.dat` / `.seq`: `agilentImage` / `agilentImageIFG` of the file;
- `.dms`: `agilentMosaic` of the `.dmt` beside it, i.e. the mosaic assembled from its `.dmd`
  tiles (not from the `.dms` itself);
- `.dmd` / `.drd`: one tile of `agilentMosaicTiles` / `agilentMosaicIFGTiles`, flipped to image
  coordinates as agilent-format does for `MAT=True`.

Per file: the spectrum count and point count, whole spectra of six pixels (xxh3-128 of the values
as f64) with 24 sampled rows `[i, x, y]` (x = spacing × (first index + i)), the plane at the
middle point (xxh3-128 as f64) and the settings agilent-format reads (as facts).

agilent-format is not in the shared oracle environment:

    uv pip install --python .venv/bin/python agilent-format numpy xxhash
    .venv/bin/python oracle/agilent_fpa_oracle.py --id ID corpus/files/DIR/FILE.dat

Writes `corpus/oracle/hyperspec/<ID>.json`.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
from pathlib import Path

import numpy as np
import xxhash

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "corpus" / "oracle" / "hyperspec"


def h128(values) -> str:
    v = np.asarray(values, dtype="<f8").copy()
    v[np.isnan(v)] = np.nan
    return xxhash.xxh3_128_hexdigest(v.tobytes())


def pick(n: int, k: int) -> list[int]:
    if n <= k:
        return list(range(n))
    return sorted({round(i * (n - 1) / (k - 1)) for i in range(k)})


def fnum(v: float):
    return None if not math.isfinite(v) else float(v)


def load(p: Path):
    """(data rows x cols x points in image coordinates, x values, info, reader)."""
    import agilent_format as ag

    ext = p.suffix.lower()
    if ext == ".dat":
        a = ag.agilentImage(str(p), MAT=True)
        x = np.asarray(a.info["wavenumbers"], dtype="f8")
        return np.asarray(a.data), x, a.info, "agilentImage"
    if ext == ".seq":
        a = ag.agilentImageIFG(str(p), MAT=True)
        i = a.info
        x = i["PtSep"] * (i["StartPt"] + np.arange(i["Npts"], dtype="f8"))
        return np.asarray(a.data), x, i, "agilentImageIFG"
    if ext == ".dms":
        dmt = p.with_suffix(".dmt")
        if not dmt.is_file():
            dmt = p.with_name(p.stem.lower() + ".dmt")
        a = ag.agilentMosaic(str(dmt), MAT=True)
        x = np.asarray(a.info["wavenumbers"], dtype="f8")
        return np.asarray(a.data), x, a.info, "agilentMosaic (assembled from the .dmd tiles)"
    if ext in (".dmd", ".drd"):
        base, tx, ty = p.stem.rsplit("_", 2)
        dmt = p.with_name(base + ".dmt")
        if not dmt.is_file():
            dmt = p.with_name(base.lower() + ".dmt")
        if ext == ".dmd":
            t = ag.agilentMosaicTiles(str(dmt), MAT=True)
            x = np.asarray(t.info["wavenumbers"], dtype="f8")
        else:
            t = ag.agilentMosaicIFGTiles(str(dmt), MAT=True)
            i = t.info
            x = i["PtSep"] * (i["StartPt"] + np.arange(i["Npts"], dtype="f8"))
        tile = np.flipud(t.tiles[int(tx), int(ty)]())
        return np.asarray(tile), x, t.info, "agilentMosaicTiles, one tile"
    raise SystemExit(f"not an Agilent data file: {p}")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("file")
    ap.add_argument("--id", required=True)
    ap.add_argument("--out", default=str(OUT))
    a = ap.parse_args()
    p = Path(a.file)
    data, x, info, how = load(p)
    rows, cols, n = data.shape
    count = rows * cols
    tr = {"trace": 0, "sweeps": int(count), "n": int(n), "sweep_hashes": {}, "rows": {}}
    for k in pick(count, 6):
        r, c = k // cols, k % cols
        y = np.asarray(data[r, c, :], dtype="f8")
        tr["sweep_hashes"][str(k)] = h128(y)
        tr["rows"][str(k)] = [[int(i), float(x[i]), fnum(y[i])] for i in pick(n, 24)]
    mid = n // 2
    plane = np.asarray(data[:, :, mid], dtype="f4").astype("f8")
    img = {"image": 0, "width": int(cols), "height": int(rows), "channel": int(mid),
           "xxh3_f64": h128(plane.ravel()),
           "samples": [[int(cc), int(rr), fnum(plane[rr, cc])] for rr, cc in zip(pick(rows, 8), pick(cols, 8))]}
    facts = []
    if "Resolution" in info:
        facts.append({"path": "method.parameters.resolution", "value": float(info["Resolution"])})
    if "Effective Laser Wavenumber" in info:
        facts.append({"path": "method.parameters.laser_wavenumber", "value": float(info["Effective Laser Wavenumber"])})
    o = {"id": a.id, "format": "agilent-fpa", "file": p.name, "size": p.stat().st_size,
         "sha256": hashlib.sha256(p.read_bytes()).hexdigest(),
         "reader": f"agilent-format 0.4.7 (MIT), {how}, MAT=True", "independent": True,
         "trace_count": 1, "traces": [tr], "images": [img], "facts": facts}
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    (out / f"{a.id}.json").write_text(json.dumps(o, indent=1) + "\n")
    print(f"wrote {out / (a.id + '.json')}: {count} spectra x {n} points")


if __name__ == "__main__":
    main()
