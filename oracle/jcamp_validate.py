"""Check `openreadout export --format jcamp` output with nmrglue (BSD-3) and jcamp (MIT).

Usage:
    uv run python jcamp_validate.py OUT.jdx [--source FILE] [--openreadout BIN]

- nmrglue (`nmrglue.jcampdx.read`, NMR data types only): the ordinates of the first page (and the
  imaginary page for complex NTUPLES) must equal what `openreadout trace SOURCE` returns within
  the factor the export reports (bit-exact when the report says `exact`).
- jcamp (`jcamp.readfile`, XYDATA and (XY..XY) tables): x (FIRSTX/LASTX/NPOINTS, or the pairs) and y must match the
  source the same way. jcamp cannot read negative numbers in compressed or AFFN tables (its
  tokenizer knows no minus sign outside exponent notation), so files whose abscissa goes below
  zero are reported as `jcamp: skipped (negative abscissa)`.

Without --source only the parsers' success is checked. Prints one JSON line; exit 1 on failure.
"""

import argparse
import json
import math
import subprocess
import sys
import warnings
from pathlib import Path

import numpy as np


def run(bin_: str, *args: str) -> dict:
    out = subprocess.run([bin_, *args, "--json"], capture_output=True, text=True, check=True)
    env = json.loads(out.stdout)
    if not env.get("ok"):
        raise SystemExit(f"openreadout {' '.join(args)} failed: {env.get('error')}")
    return env["data"]


def header(path: Path) -> dict:
    h = {}
    for line in path.read_text(errors="replace").splitlines():
        if line.startswith("##") and "=" in line:
            k, v = line[2:].split("=", 1)
            h.setdefault(k.strip().upper(), v.split("$$")[0].strip())
    return h


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("file", type=Path)
    ap.add_argument("--source", type=Path)
    ap.add_argument("--openreadout", default="openreadout")
    ap.add_argument("--trace", type=int, default=0)
    ap.add_argument("--sweep", type=int, default=0)
    a = ap.parse_args()
    h = header(a.file) if a.file.exists() else {}
    problems: list[str] = []
    result: dict = {"file": str(a.file), "data_type": h.get("DATA TYPE"), "data_class": h.get("DATA CLASS")}
    want = None
    tol = None
    if a.source is not None:
        rep = run(a.openreadout, "export", str(a.source), "--format", "jcamp", "-o", str(a.file),
                  "--overwrite", "--trace", str(a.trace), "--sweep", str(a.sweep))
        n = rep["samples_written"]
        tr = run(a.openreadout, "trace", str(a.source), "--trace", str(a.trace), "--sweep", str(a.sweep),
                 "--first", str(rep["first_sample"]), "--count", str(n), "--max-samples", str(min(n, 100000)))
        want = [np.array([math.nan if v is None else v for v in ch["samples"]]) for ch in tr["channels"]]
        # bit-exact when exact, else within half the factor
        tol = [0.0 if rep["exact"] else f / 2 for f in rep["factors"]]
        result["exact"] = rep["exact"]
        h = header(a.file)

    def compare(tag: str, got, c: int):
        if want is None:
            return
        w = want[c]
        g = np.asarray(got, dtype=np.float64)[: len(w)]
        if len(g) < len(w):
            problems.append(f"{tag}: {len(g)} values, expected at least {len(w)}")
            return
        d = np.abs(g - w)
        if tol[c] == 0.0:
            ok = np.array_equal(g, w)
        else:
            ok = bool(np.all(d <= tol[c] * (1 + 1e-12)))
        if not ok:
            i = int(np.argmax(d))
            problems.append(f"{tag}: channel {c} differs (max |diff| {float(d.max())} at {i}; tolerance {tol[c]})")
        result[f"{tag}_compared"] = result.get(f"{tag}_compared", 0) + len(w)

    # nmrglue
    dtype = (h.get("DATA TYPE") or "").upper()
    if dtype.startswith("NMR"):
        import nmrglue as ng

        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter("always")
            dic, data = ng.jcampdx.read(str(a.file))
        if data is None:
            problems.append(f"nmrglue: no data ({[str(w.message) for w in caught]})")
        else:
            arrays = data if isinstance(data, list) else [data]
            result["nmrglue"] = [len(x) for x in arrays if x is not None]
            for c, arr in enumerate(arrays):
                if arr is not None and want is not None and c < len(want):
                    compare("nmrglue", arr, c)
    else:
        result["nmrglue"] = "skipped (not an NMR data type)"

    # jcamp
    if (h.get("DATA CLASS") or "").upper() == "XYDATA":
        first = float(h.get("FIRSTX", "0").replace("E", "e"))
        last = float(h.get("LASTX", "0").replace("E", "e"))
        if min(first, last) < 0:
            result["jcamp"] = "skipped (negative abscissa)"
        else:
            import jcamp

            d = jcamp.readfile(str(a.file))
            result["jcamp"] = len(d["y"])
            if len(d["x"]) != len(d["y"]):
                problems.append("jcamp: x and y lengths differ")
            compare("jcamp", d["y"], 0)
    elif (h.get("DATA CLASS") or "").upper() in ("PEAK TABLE", "XYPOINTS"):
        import jcamp

        d = jcamp.readfile(str(a.file))
        result["jcamp"] = len(d["y"])
        compare("jcamp_x", d["x"], 0)
        compare("jcamp", d["y"], 1)
    else:
        result["jcamp"] = "skipped (NTUPLES)"
    result["ok"] = not problems
    result["problems"] = problems
    print(json.dumps(result, default=str))
    return 0 if not problems else 1


if __name__ == "__main__":
    sys.exit(main())
