#!/usr/bin/env python
"""Ground truth for Rigaku `.ras` and `.rasx` files that geddes does not open, compared by
`crates/openreadout-corpus-tests/tests/corpus/series_oracle/` like the other series oracles.

Sources, never our reader:
- `.ras`: xrayutilities (GPL-2.0-or-later, https://github.com/dkriegner/xrayutilities), run as a
  black box: `xrayutilities.io.RASFile(path).scans`, every scan's first data column (the scan
  axis) and its `int` column. Its source is not read.
- `.rasx`: FAIRmat readers-xrd (`fairmat-readers-xrd` on PyPI, Apache-2.0,
  https://github.com/FAIRmat-NFDI/readers-xrd), `read_rigaku_rasx(path)`: `2Theta` and
  `intensity`. It reads `Data0/Profile0.txt` and its `MesurementConditions0.xml` and does not
  need `root.xml`. A reciprocal-space map comes back as a (scans x points) intensity array with
  one 2θ axis; its first, middle and last scans are compared as traces of the same index.

Usage (a venv with xrayutilities and fairmat-readers-xrd):
    python oracle/rigaku_oracle.py --id ID corpus/files/ID.ras [--out DIR]

Writes `<DIR>/<ID>.json` (default `corpus/oracle/series/`), the shape `series_oracle.py` writes:
rows `[i, x, y]` sampled at up to 256 evenly spaced indices per compared scan.
"""
from __future__ import annotations

import argparse
import json
import sys
import warnings
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import oracle_json  # noqa: E402
from series_oracle import pick  # noqa: E402

OUT = HERE.parent / "corpus" / "oracle" / "series"


def row_set(trace: int, x, y, src: str) -> dict:
    return {"trace": trace, "sweep": 0, "channel": "intensity", "n": int(len(y)),
            "samples": [[i, float(x[i]), float(y[i])] for i in pick(len(y))],
            "x_tol": 1e-6, "y_tol_rel": 1e-9, "y_tol_abs": 0.0, "source": src}


def ras_traces(path: Path) -> list[dict]:
    import numpy as np
    import xrayutilities as xu

    f = xu.io.RASFile(str(path))
    src = f"xrayutilities {xu.__version__} RASFile (GPL-2.0-or-later, black box)"
    out = []
    for k, s in enumerate(f.scans):
        d = s.data
        out.append(row_set(k, np.asarray(d[d.dtype.names[0]], float), np.asarray(d["int"], float), src))
    return out


def rasx_traces(path: Path) -> list[dict]:
    import numpy as np
    import fairmat_readers_xrd
    from fairmat_readers_xrd.readers import read_rigaku_rasx

    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        d = read_rigaku_rasx(str(path))
        x = np.asarray(getattr(d["2Theta"], "magnitude", d["2Theta"]), float).ravel()
        y = np.asarray(getattr(d["intensity"], "magnitude", d["intensity"]), float)
    version = getattr(fairmat_readers_xrd, "__version__", "0.0.10")
    src = f"fairmat-readers-xrd {version} read_rigaku_rasx (Apache-2.0)"
    if y.ndim == 1:
        return [row_set(0, x, y, src)]
    n = y.shape[0]
    return [row_set(k, x, y[k], src) for k in sorted({0, n // 2, n - 1})]


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--id", required=True)
    ap.add_argument("--out", type=Path, default=OUT)
    ap.add_argument("file", type=Path)
    a = ap.parse_args()
    fmt = "rigaku-rasx" if a.file.suffix.lower() == ".rasx" else "rigaku-ras"
    try:
        traces = rasx_traces(a.file) if fmt == "rigaku-rasx" else ras_traces(a.file)
        o = {"id": a.id, "format": fmt, "independent": True, "traces": traces}
    except Exception as e:  # the independent reader could not read it: an oracle error, recorded
        o = {"id": a.id, "format": fmt, "error": f"{type(e).__name__}: {e}"}
    a.out.mkdir(parents=True, exist_ok=True)
    oracle_json.write_text(a.out / f"{a.id}.json", json.dumps(o, indent=1) + "\n")
    print(f"{a.id}: {len(o['traces'])} trace comparisons" if "traces" in o else f"{a.id}: oracle error {o['error']}")


if __name__ == "__main__":
    main()
