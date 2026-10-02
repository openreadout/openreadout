#!/usr/bin/env python
"""Ground truth for `table --filter ... --count`: events meeting conditions, counted with FlowIO
(BSD-3, Scott White) and NumPy only.

Usage: python fcs_filter.py            (writes ../corpus/oracle/flow/filter-counts.json)
       python fcs_filter.py --check    (exit 1 when the committed file differs)

Values tested, per job:
* `raw`: FlowIO's event matrix as stored (`FlowData.events`, integer data masked per `$PnR`
  bits as FlowIO does), which is what `openreadout table` returns without processing;
* `compensated`: FlowIO's scale values (`as_array(preprocess=True)`: `$PnE` log decoding, `$PnG`
  gain, `$TIMESTEP`), then the file's `$SPILLOVER` matrix applied with NumPy
  (`x_comp = x[:, detectors] @ inv(spill)`), the matrix parsed from the keyword text here;
* `arcsinh`: the compensated values of the fluorescence detectors through NumPy
  `arcsinh(x / cofactor)`.
A condition `NAME OP NUMBER` is tested with NumPy comparisons; every condition must hold.
"""
import json
import os
import sys
import tomllib
from pathlib import Path

import flowio
import numpy as np

ROOT = Path(__file__).resolve().parent.parent
FILES = Path(os.environ.get("OPENREADOUT_CORPUS_DIR", ROOT / "corpus" / "files")).resolve()
OUT = ROOT / "corpus" / "oracle" / "flow" / "filter-counts.json"

with open(ROOT / "corpus" / "manifest.toml", "rb") as f:
    MANIFEST = {e["id"]: e for e in tomllib.load(f)["file"] if e.get("format") == "fcs"}

# (fcs id, values, [conditions], cofactor)
JOBS = [
    ("flowio-3fitc-4pe-004", "raw", ["FL1-H > 100"], None),
    ("flowio-3fitc-4pe-004", "raw", ["FL1-H > 100", "FL2-H <= 50"], None),
    ("flowio-3fitc-4pe-004", "raw", ["SSC-H >= 200", "FSC-H < 400"], None),
    ("flowkit-8c-e01", "raw", ["TNFa FITC FLR-A > 1000"], None),
    ("flowkit-8c-e01", "compensated", ["TNFa FITC FLR-A > 1000"], None),
    ("flowkit-8c-e01", "compensated", ["CD4 PE-Cy7 FLR-A > 2000", "CD8 PerCP-Cy55 FLR-A < 500"], None),
    ("flowkit-8c-e01", "arcsinh", ["CD3 APC-H7 FLR-A > 2.5"], 150.0),
]

OPS = {">": np.greater, ">=": np.greater_equal, "<": np.less, "<=": np.less_equal, "==": np.equal, "!=": np.not_equal}


def parse(cond):
    for sym in (">=", "<=", "!=", "==", ">", "<"):
        if sym in cond:
            name, value = cond.split(sym, 1)
            return name.strip(), sym, float(value)
    raise ValueError(cond)


def spill_matrix(fd):
    key = next(k for k in fd.text if k.lower() in ("spillover", "$spillover", "spill", "$spill"))
    parts = [p.strip() for p in fd.text[key].split(",")]
    n = int(parts[0])
    names = parts[1 : 1 + n]
    values = np.array([float(v) for v in parts[1 + n : 1 + n + n * n]]).reshape(n, n)
    return names, values


def values_of(path, kind, cofactor):
    fd = flowio.FlowData(str(path))
    names = [fd.channels[k]["pnn"] for k in sorted(fd.channels, key=int)]
    if kind == "raw":
        x = np.asarray(fd.as_array(preprocess=False), dtype=np.float64)
        return names, x, fd.event_count
    x = np.asarray(fd.as_array(preprocess=True), dtype=np.float64)
    det, m = spill_matrix(fd)
    idx = [names.index(d) for d in det]
    x[:, idx] = x[:, idx] @ np.linalg.inv(m)
    if kind == "arcsinh":
        x[:, idx] = np.arcsinh(x[:, idx] / cofactor)
    return names, x, fd.event_count


def build():
    out = {"reader": f"flowio {flowio.__version__} + NumPy {np.__version__}", "jobs": []}
    for fid, kind, conds, cofactor in JOBS:
        path = FILES / MANIFEST[fid]["filename"]
        names, x, total = values_of(path, kind, cofactor)
        mask = np.ones(x.shape[0], dtype=bool)
        for c in conds:
            name, sym, v = parse(c)
            mask &= OPS[sym](x[:, names.index(name)], v)
        out["jobs"].append({
            "id": fid,
            "values": kind,
            "cofactor": cofactor,
            "conditions": conds,
            "matched": int(mask.sum()),
            "total": int(total),
        })
    return out


def main(argv):
    out = build()
    text = json.dumps(out, indent=1) + "\n"
    if "--check" in argv:
        if not OUT.exists() or OUT.read_text() != text:
            print(f"{OUT} is out of date", file=sys.stderr)
            return 1
        print("up to date")
        return 0
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(text)
    for j in out["jobs"]:
        print(f"{j['id']} {j['values']} {j['conditions']}: {j['matched']} of {j['total']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
