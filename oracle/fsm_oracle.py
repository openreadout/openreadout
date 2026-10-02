#!/usr/bin/env python
"""Ground truth for PerkinElmer Spotlight `.fsm` images, compared by
`crates/openreadout-corpus-tests/tests/hyperspec_oracle/`.

Source, never our reader: specio 0.1.0 (BSD-3-Clause), run as a black box (`specread`): the
spectra (one per pixel, in file order) and the axis (`wavelength`, from the geometry block);
the instrument texts it reads by position (as facts: model, serial).

Usage (the shared oracle venv, which has specio):
    oracle/.venv/bin/python oracle/fsm_oracle.py --id ID corpus/files/pe-fsm/FILE.fsm

Writes `corpus/oracle/hyperspec/<ID>.json`.
"""
from __future__ import annotations

import argparse
import collections
import collections.abc
import hashlib
import json
import math
from pathlib import Path

import numpy as np
import xxhash

# specio 0.1 imports `collections.Iterable`, removed in Python 3.10
collections.Iterable = collections.abc.Iterable  # type: ignore[attr-defined]

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


def main() -> None:
    from specio import specread

    ap = argparse.ArgumentParser()
    ap.add_argument("file")
    ap.add_argument("--id", required=True)
    ap.add_argument("--out", default=str(OUT))
    a = ap.parse_args()
    p = Path(a.file)
    s = specread(str(p))
    amp = np.asarray(s.amplitudes, dtype="f4").astype("f8")
    x = np.asarray(s.wavelength, dtype="f8")
    count, n = amp.shape
    m = s.meta
    nx, ny = int(m["n_x"]), int(m["n_y"])
    tr = {"trace": 0, "sweeps": int(count), "n": int(n), "sweep_hashes": {}, "rows": {}}
    for k in pick(count, 6):
        tr["sweep_hashes"][str(k)] = h128(amp[k])
        tr["rows"][str(k)] = [[int(i), float(x[i]), fnum(amp[k, i])] for i in pick(n, 24)]
    mid = n // 2
    plane = amp[:, mid].reshape(ny, nx)
    img = {"image": 0, "width": nx, "height": ny, "channel": int(mid),
           "xxh3_f64": h128(plane.ravel()),
           "samples": [[int(c), int(r), fnum(plane[r, c])] for r, c in zip(pick(ny, 8), pick(nx, 8))]}
    facts = []
    if m.get("instrument_model"):
        facts.append({"path": "instrument.model", "value": m["instrument_model"]})
    if m.get("instrument_serial_number"):
        facts.append({"path": "instrument.serial", "value": m["instrument_serial_number"].lstrip("/").strip()})
    o = {"id": a.id, "format": "perkinelmer-fsm", "file": p.name, "size": p.stat().st_size,
         "sha256": hashlib.sha256(p.read_bytes()).hexdigest(),
         "reader": "specio 0.1.0 (BSD-3-Clause), specread", "independent": True,
         "trace_count": 1, "traces": [tr], "images": [img], "facts": facts}
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    (out / f"{a.id}.json").write_text(json.dumps(o, indent=1) + "\n")
    print(f"wrote {out / (a.id + '.json')}: {count} spectra x {n} points")


if __name__ == "__main__":
    main()
