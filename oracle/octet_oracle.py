#!/usr/bin/env python
"""Ground truth for Sartorius Octet `.frd` files (`sartorius-octet-frd`) from pykingenie (MIT,
https://github.com/osvalB/pykingenie, installed from PyPI and run as a black box:
`OctetExperiment.read_sensor_data`). Needs `pip install pykingenie` (a separate venv; not in the
shared oracle environment).

Recorded, in `oracle/series_oracle.py`'s format (`crates/openreadout-corpus-tests/tests/
series_oracle/mod.rs` compares): the sensorgram (every step's time and response concatenated,
sampled at up to 256 points: our `time` and `response` channels), the step table (per step: the
point count, concentration, molar concentration, molecular weight, temperature, assay, actual and
cycle time and shake speed; -1 as not set), and the instrument model, serial, operator, start time.

Usage:  python octet_oracle.py --id ID FILE [--out DIR]
"""
from __future__ import annotations

import json
import math
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "corpus" / "oracle" / "series"
ROWS = 256


def pick(n: int) -> list[int]:
    return sorted({round(i * (n - 1) / (ROWS - 1)) for i in range(ROWS)}) if n > ROWS else list(range(n))


def num(v) -> float | None:
    try:
        x = float(v)
    except (TypeError, ValueError):
        return None
    return None if (x == -1 or math.isnan(x)) else x


def main(argv: list[str]) -> int:
    out_dir, ident, files = OUT, None, []
    it = iter(argv)
    for a in it:
        if a == "--out":
            out_dir = Path(next(it))
        elif a == "--id":
            ident = next(it)
        else:
            files.append(Path(a))
    if len(files) != 1 or not ident:
        print(__doc__, file=sys.stderr)
        return 2
    from pykingenie.octet import OctetExperiment  # type: ignore

    e = OctetExperiment(ident)
    e.read_sensor_data(str(files[0]))
    xs = [float(v) for step in e.xs[0] for v in step]
    ys = [float(v) for step in e.ys[0] for v in step]
    idx = pick(len(xs))
    src = f"pykingenie {files[0].name}"
    traces = [
        {"trace": 0, "sweep": 0, "channel": "response", "n": len(ys),
         "samples": [[i, xs[i], ys[i]] for i in idx], "x_tol": 1e-6, "y_tol_rel": 1e-7, "y_tol_abs": 1e-9,
         "source": src},
        {"trace": 0, "sweep": 0, "channel": "time", "n": len(xs),
         "samples": [[i, xs[i], xs[i]] for i in idx], "x_tol": 1e-6, "y_tol_rel": 1e-7, "y_tol_abs": 1e-9,
         "source": src},
    ]
    si = e.step_info[0]
    nsteps = len(e.xs[0])
    cells = []
    for s in range(nsteps):
        cells.append([s, "step", s + 1])
        cells.append([s, "points", len(e.xs[0][s])])
        for ours, theirs in (("concentration", "Concentration"), ("molar_concentration", "MolarConcentration"),
                             ("molecular_weight", "MolecularWeight"), ("temperature", "Temperature"),
                             ("assay_time", "AssayTime"), ("actual_time", "ActualTime"),
                             ("cycle_time", "CycleTime"), ("flow_rate", "FlowRate")):
            v = num(si[theirs][s])
            if v is not None:  # unset (-1) values are not compared
                cells.append([s, ours, v])
    info = e.exp_info[0]
    facts = []
    for path, key in (("instrument.model", "InstrumentType"), ("instrument.serial", "InstrumentSerial"),
                      ("acquisition.operator", "UserName"), ("acquisition.started_at", "StartDateTime")):
        if info.get(key):
            facts.append({"path": path, "value": info[key], "source": src})
    o = {"id": ident, "format": "sartorius-octet-frd", "independent": True, "reader": "pykingenie (MIT) OctetExperiment",
         "traces": traces, "tables": [{"table": 0, "rows": nsteps, "cells": cells, "tol_rel": 1e-9, "tol_abs": 1e-9}],
         "facts": facts}
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / f"{ident}.json").write_text(json.dumps(o, indent=1) + "\n")
    print(ident, nsteps, "steps", len(xs), "points")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
