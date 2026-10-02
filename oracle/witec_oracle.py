#!/usr/bin/env python
"""Ground truth for WITec Project `.wip` / WITec Data `.wid` files
(`crates/openreadout-corpus-tests/tests/hyperspec_oracle/mod.rs` compares).

Sources, never our reader:
- witio 0.2 (MIT-0, https://github.com/iCalculate/witio, a Python port of wit_io), run as a black
  box: every graph (spectra) with its calibrated x axis, every image and video image;
- the WITec software's own text export of a data file (`--export`): the x column (as printed,
  six significant digits) and one column per spectrum, compared as exported.

witio is not in the shared oracle environment. Run with a separate venv holding it:

    uv venv .venv --python 3.12 && uv pip install --python .venv/bin/python witio numpy xxhash pillow
    .venv/bin/python oracle/witec_oracle.py --id ID corpus/files/FILE.wip
    .venv/bin/python oracle/witec_oracle.py --id ID corpus/files/FILE.wid --export corpus/files/FILE.txt \
        --export-sweeps 0,1,2,349,350,62999

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
SWEEPS = 6
ROWS = 24


def h128(values: np.ndarray) -> str:
    v = np.asarray(values, dtype="<f8").copy()
    v[np.isnan(v)] = np.nan  # canonical quiet NaN
    return xxhash.xxh3_128_hexdigest(v.tobytes())


def pick(n: int, k: int) -> list[int]:
    if n <= k:
        return list(range(n))
    return sorted({round(i * (n - 1) / (k - 1)) for i in range(k)})


def fnum(v: float):
    return None if not math.isfinite(v) else float(v)


def graph_traces(pr) -> list[dict]:
    out = []
    ti = 0
    for d in pr.data:
        if d.class_name != "TDGraph":
            continue
        try:
            arr = d.array()  # (size_x, size_y, points)
            x, unit = d.x_axis()
        except Exception as e:  # noqa: BLE001 - recorded, not hidden
            out.append({"trace": ti, "name": d.caption, "error": f"witio: {e!r}"})
            ti += 1
            continue
        sx, sy, n = arr.shape
        count = sx * sy
        entry = {"trace": ti, "name": d.caption, "sweeps": int(count), "n": int(n),
                 "x_unit_witio": unit, "sweep_hashes": {}, "rows": {}}
        for k in pick(count, SWEEPS):
            xi, yi = k % sx, k // sx
            y = np.asarray(arr[xi, yi, :], dtype="f8")
            entry["sweep_hashes"][str(k)] = h128(y)
            entry["rows"][str(k)] = [[int(i), float(x[i]), fnum(y[i])] for i in pick(n, ROWS)]
        out.append(entry)
        ti += 1
    return out


def images(pr) -> list[dict]:
    """Images in our order: maps of graphs (both sizes > 1), then images and video images in
    the order of the data list."""
    maps, rasters = [], []
    for d in pr.data:
        if d.class_name == "TDGraph":
            pl = d.payload
            sx, sy = int(pl["SizeX"].scalar()), int(pl["SizeY"].scalar())
            if sx > 1 and sy > 1:
                arr = d.array()
                c = arr.shape[2] // 2
                plane = np.asarray(arr[:, :, c], dtype="f4").astype("f8").T  # (y, x)
                maps.append({"name": d.caption, "width": sx, "height": sy, "channel": int(c),
                             "xxh3_f64": h128(plane.ravel()),
                             "samples": [[int(xx), int(yy), fnum(plane[yy, xx])] for yy, xx in
                                         zip(pick(sy, 8), pick(sx, 8))]})
        elif d.class_name == "TDImage":
            arr = d.array()
            plane = np.asarray(arr, dtype="f8").T
            sy, sx = plane.shape
            rasters.append({"name": d.caption, "width": sx, "height": sy, "channel": 0,
                            "xxh3_f64": h128(plane.ravel()),
                            "samples": [[int(xx), int(yy), fnum(plane[yy, xx])] for yy, xx in
                                        zip(pick(sy, 8), pick(sx, 8))]})
        elif d.class_name == "TDBitmap":
            try:
                arr = d.array()
            except Exception as e:  # noqa: BLE001
                rasters.append({"name": d.caption, "error": f"witio: {e!r}"})
                continue
            if pr.version is not None and pr.version <= 5:
                rgb = np.asarray(arr, dtype="u1")  # PIL: (rows, cols, 3), top row first
            else:
                rgb = np.asarray(arr, dtype="u1").transpose(1, 0, 2)  # (x, y, 3) -> (y, x, 3)
            sy, sx, _ = rgb.shape
            rasters.append({"name": d.caption, "width": sx, "height": sy, "rgb": True,
                            "xxh3_rgb8": xxhash.xxh3_128_hexdigest(np.ascontiguousarray(rgb).tobytes())})
    out = []
    for k, im in enumerate(maps + rasters):
        im["image"] = k
        out.append(im)
    return out


def export_rows(path: Path, sweeps: list[int]) -> tuple[list[dict], int, int]:
    """The WITec text export: tab-separated, first column x, one column per spectrum."""
    rows: dict[int, list] = {k: [] for k in sweeps}
    n = 0
    cols = 0
    xs = []
    with path.open("r", encoding="latin-1") as fh:
        for line in fh:
            f = line.rstrip("\r\n").split("\t")
            if len(f) < 2:
                continue
            cols = len(f) - 1
            x = float(f[0])
            xs.append(x)
            for k in sweeps:
                if k < cols:
                    rows[k].append([n, x, float(f[k + 1])])
            n += 1
    out = []
    for k in sweeps:
        if k >= cols:
            continue
        sampled = [rows[k][i] for i in pick(n, 64)]
        out.append({"sweep": k, "rows": sampled, "all_y": [r[2] for r in rows[k]]})
    return out, n, cols


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("file")
    ap.add_argument("--id", required=True)
    ap.add_argument("--out", default=str(OUT))
    ap.add_argument("--export")
    ap.add_argument("--export-sweeps", default="0,1,2")
    ap.add_argument("--export-trace", type=int, default=0)
    ap.add_argument("--export-x", default="nm", help="what the export's x column is: nm (wavelength) or axis (our x)")
    a = ap.parse_args()
    p = Path(a.file)
    data = p.read_bytes() if p.stat().st_size < 2_000_000_000 else None
    o: dict = {"id": a.id, "format": "witec-project", "file": p.name, "size": p.stat().st_size,
               "sha256": hashlib.sha256(data).hexdigest() if data is not None else None}
    if a.export:
        ex = Path(a.export)
        sweeps = [int(s) for s in a.export_sweeps.split(",") if s]
        rows, n, cols = export_rows(ex, sweeps)
        o["reader"] = "WITec software text export (" + ex.name + ")"
        o["independent"] = True
        o["export"] = {"trace": a.export_trace, "points": n, "spectra": cols, "x": a.export_x,
                       "x_tol_rel": 1.5e-6, "sweeps": rows}
    else:
        import witio
        pr = witio.read(p)
        o["reader"] = f"witio {getattr(witio, '__version__', '?')} (MIT-0)"
        o["independent"] = True
        o["version"] = pr.version
        o["traces"] = graph_traces(pr)
        o["trace_count"] = len(o["traces"])
        o["images"] = images(pr)
        facts = []
        sm = pr.system_metadata
        if sm.get("system_id"):
            facts.append({"path": "instrument.serial", "value": sm["system_id"]})
        o["facts"] = facts
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    (out / f"{a.id}.json").write_text(json.dumps(o, indent=1) + "\n")
    print(f"wrote {out / (a.id + '.json')}: {len(o.get('traces', []))} traces, {len(o.get('images', []))} images")


if __name__ == "__main__":
    main()
