#!/usr/bin/env python3
"""Getting pixels into Python: the `openreadout` package vs bioio's own readers.

Each measurement runs in its own Python process (so peak RSS is that of the task alone, measured
with `resource.getrusage` in the child) and is repeated `--reps` times; the median wall time and
the largest peak RSS are reported. Tasks:

  full    every plane of one image into one NumPy array
          ours: `File.read_image(i)`; bioio: `BioImage(path, reader=R).get_image_data("TCZYX[S]")`
  dask    the mean of one image through dask (threaded scheduler)
          ours: `File.to_dask(i).mean().compute()`;
          bioio: `BioImage(...).dask_data.mean().compute()`
  region  a 512 x 512 window at the centre of full resolution (whole-slide images)
          ours: `File.read_plane(i, region=...)`; bioio: `get_image_dask_data("YX[S]")[window]`

Comparators (run as black boxes; bioio-czi wraps the GPL pylibCZIrw):
bioio-czi (CZI), bioio-nd2 (ND2), bioio-tifffile (TIFF family). A comparator that fails, runs
out of time (`--timeout`) or memory (`--rss-cap`) is reported as such.

    oracle/.venv/bin/python bench/python_arrays.py run [--reps 3] [--only czi-kidney]
    oracle/.venv/bin/python bench/python_arrays.py report       # Markdown table
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, Dict, List, Optional

ROOT = Path(__file__).resolve().parent.parent
CORPUS = Path(os.environ.get("OPENREADOUT_CORPUS_DIR", ROOT / "corpus" / "files"))
OUT = ROOT / "target" / "reports" / "bench" / "python_arrays.json"

# (id, file, image, comparator reader module, tasks)
CASES = [
    ("czi-small", "aics-s-3-t-1-c-3-z-5.czi", 0, "bioio_czi", ["full", "dask"]),
    (
        "czi-kidney",
        "zenodo10577621-Kidney-RAC-3color.czi",
        0,
        "bioio_czi",
        ["full", "dask", "region"],
    ),
    ("czi-young-mouse", "zenodo10577621-Young-mouse.czi", 0, "bioio_czi", ["region"]),
    ("nd2-karl", "ome-karl-sample-image.nd2", 0, "bioio_nd2", ["full", "dask"]),
    ("nd2-jobs", "zenodo21162526-nd2jobs.nd2", 0, "bioio_nd2", ["full", "dask"]),
    ("ome-tiff", "aics-s-3-t-1-c-3-z-5.ome.tiff", 0, "bioio_tifffile", ["full", "dask"]),
    ("svs-cmu1", "openslide-aperio-CMU-1.svs", 0, "bioio_tifffile", ["region"]),
    ("qptiff", "ome-qptiff-HandEcompressed_Scan1.qptiff", 0, "bioio_tifffile", ["region"]),
]

CHILD = r"""
import json, resource, sys, time
import numpy as np
tool, task, path, image, reader_mod = sys.argv[1:6]
image = int(image)
t0 = time.perf_counter()
out = {}
if tool == "openreadout":
    import openreadout
    f = openreadout.File(path)
    im = f.images[image]
    if task == "full":
        a = f.read_image(image)
        out["shape"] = list(a.shape)
        out["sum"] = float(a.sum(dtype=np.float64))
    elif task == "dask":
        out["mean"] = float(f.to_dask(image).mean().compute())
    else:
        w, h = im["size_x"], im["size_y"]
        a = f.read_plane(image, region=((w - 512) // 2, (h - 512) // 2, 512, 512))
        out["shape"] = list(a.shape); out["sum"] = float(a.sum(dtype=np.float64))
else:
    import importlib
    from bioio import BioImage
    R = importlib.import_module(reader_mod).Reader
    img = BioImage(path, reader=R)
    img.set_scene(image)
    if task == "full":
        dims = "TCZYXS" if "S" in img.dims.order else "TCZYX"
        a = img.get_image_data(dims)
        out["shape"] = list(a.shape)
        out["sum"] = float(a.sum(dtype=np.float64))
    elif task == "dask":
        out["mean"] = float(img.dask_data.mean().compute())
    else:
        dims = "YXS" if "S" in img.dims.order else "YX"
        d = img.get_image_dask_data(dims)
        h, w = d.shape[0], d.shape[1]
        y0, x0 = (h - 512) // 2, (w - 512) // 2
        a = np.asarray(d[y0:y0 + 512, x0:x0 + 512].compute())
        out["shape"] = list(a.shape); out["sum"] = float(a.sum(dtype=np.float64))
out["seconds"] = time.perf_counter() - t0
rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
out["peak_rss_mb"] = rss / (1 << 20) if sys.platform == "darwin" else rss / 1024
print(json.dumps(out))
"""


def measure(
    tool: str, task: str, path: Path, image: int, reader: str, timeout: float, rss_cap_mb: float
) -> Dict[str, Any]:
    """Run one task in a child process; kill it above `rss_cap_mb` resident or after `timeout`."""
    import psutil

    t0 = time.perf_counter()
    proc = subprocess.Popen(
        [sys.executable, "-c", CHILD, tool, task, str(path), str(image), reader],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    ps = psutil.Process(proc.pid)
    killed = None
    while proc.poll() is None:
        try:
            rss = ps.memory_info().rss / (1 << 20)
        except psutil.Error:
            rss = 0.0
        if rss > rss_cap_mb:
            killed = f"killed above {rss_cap_mb:.0f} MB resident"
        elif time.perf_counter() - t0 > timeout:
            killed = f"timeout after {timeout:.0f} s"
        if killed:
            proc.kill()
            proc.wait()
            return {"error": killed}
        time.sleep(0.05)
    stdout, stderr = proc.communicate()
    wall = time.perf_counter() - t0
    if proc.returncode != 0:
        last = (stderr.strip().splitlines() or ["?"])[-1]
        return {"error": last[:300], "wall_seconds": wall}
    out = json.loads(stdout.strip().splitlines()[-1])
    out["wall_seconds"] = wall
    return out


def run(args: argparse.Namespace) -> None:
    results: Dict[str, Any] = json.loads(OUT.read_text()) if OUT.exists() else {}
    for cid, name, image, reader, tasks in CASES:
        if args.only and cid not in args.only:
            continue
        path = CORPUS / name
        if not path.exists():
            print(f"skip {cid}: {name} not in the corpus", file=sys.stderr)
            continue
        for task in tasks:
            for tool in ("openreadout", reader):
                runs = [
                    measure(tool, task, path, image, reader, args.timeout, args.rss_cap)
                    for _ in range(args.reps)
                ]
                ok = [r for r in runs if "error" not in r]
                key = f"{cid}/{task}/{tool}"
                if ok:
                    results[key] = {
                        "seconds": statistics.median(r["seconds"] for r in ok),
                        "peak_rss_mb": max(r["peak_rss_mb"] for r in ok),
                        "check": {k: ok[0].get(k) for k in ("shape", "sum", "mean") if k in ok[0]},
                        "reps": len(ok),
                    }
                else:
                    results[key] = {"error": runs[0]["error"]}
                print(key, json.dumps(results[key]), file=sys.stderr)
                OUT.parent.mkdir(parents=True, exist_ok=True)
                OUT.write_text(json.dumps(results, indent=1, sort_keys=True) + "\n")


def report(_: argparse.Namespace) -> None:
    results = json.loads(OUT.read_text())
    print("| file | task | openreadout s | MB | bioio reader | s | MB | same pixels |")
    print("| --- | --- | ---: | ---: | --- | ---: | ---: | --- |")
    for cid, name, _, reader, tasks in CASES:
        for task in tasks:
            a = results.get(f"{cid}/{task}/openreadout")
            b = results.get(f"{cid}/{task}/{reader}")
            if not a:
                continue

            def cell(r: Optional[Dict[str, Any]]) -> List[str]:
                if not r:
                    return ["-", "-"]
                if "error" in r:
                    return [f"failed: {r['error'][:60]}", "-"]
                return [f"{r['seconds']:.3f}", f"{r['peak_rss_mb']:.0f}"]

            same = "-"
            if a and b and "check" in a and "check" in b:
                ca, cb = a["check"], b["check"]
                if "sum" in ca and "sum" in cb:
                    same = (
                        "yes"
                        if ca["sum"] == cb["sum"] and ca.get("shape") == cb.get("shape")
                        else f"sum {ca['sum']:.6g} vs {cb['sum']:.6g}"
                    )
                elif "mean" in ca and "mean" in cb:
                    same = (
                        "yes"
                        if abs(ca["mean"] - cb["mean"]) <= 1e-9 * max(1.0, abs(cb["mean"]))
                        else f"mean {ca['mean']:.6g} vs {cb['mean']:.6g}"
                    )
            print(
                f"| {name} | {task} | {' | '.join(cell(a))} | {reader.replace('_', '-')} "
                f"| {' | '.join(cell(b))} | {same} |"
            )


def main() -> None:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run")
    r.add_argument("--reps", type=int, default=3)
    r.add_argument("--only", nargs="*")
    r.add_argument("--timeout", type=float, default=600.0)
    r.add_argument("--rss-cap", type=float, default=6000.0, help="MB; a task above it is killed")
    r.set_defaults(fn=run)
    rp = sub.add_parser("report")
    rp.set_defaults(fn=report)
    a = ap.parse_args()
    a.fn(a)


if __name__ == "__main__":
    main()
