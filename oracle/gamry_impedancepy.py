#!/usr/bin/env python
"""Ground truth for a Gamry impedance file that gamry-parser refuses, from impedance.py (MIT,
https://github.com/ECSHackWeek/impedance.py, installed from PyPI and run as a black box):
`impedance.preprocessing.readGamry` returns the frequencies and the complex impedance of the
file's ZCURVE table. Written in `oracle/series_oracle.py`'s table format, compared by
`crates/openreadout-corpus-tests/tests/corpus/series_oracle/mod.rs` by column label (`Freq`,
`Zreal`, `Zimag`).

The table's trace index follows the file's table order, as `series_oracle.gamry_traces` numbers
them (CURVE, CURVE1, ... share one trace; every other table is its own).

impedance.py is not in the shared oracle venv: install it in a venv of its own
(`uv venv V && VIRTUAL_ENV=V uv pip install impedance`) and run this script with that Python.

Usage:  python gamry_impedancepy.py --id ID FILE.DTA [--out DIR]
"""
from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

ROWS = 256


def pick(n: int) -> list[int]:
    if n <= 0:
        return []
    return sorted({round(i * (n - 1) / (ROWS - 1)) for i in range(ROWS)}) if n > ROWS else list(range(n))


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--id", required=True)
    ap.add_argument("--out", default=str(Path(__file__).resolve().parent.parent / "corpus" / "oracle" / "series"))
    ap.add_argument("file")
    a = ap.parse_args()
    from impedance import preprocessing  # type: ignore

    path = Path(a.file)
    groups: list[str] = []
    for line in path.read_bytes().decode("latin-1").splitlines():
        f = line.split("\t")
        if len(f) >= 2 and f[1].strip() == "TABLE":
            base = "CURVE" if re.fullmatch(r"CURVE\d*", f[0]) else f[0]
            if base not in groups:
                groups.append(base)
    trace = groups.index("ZCURVE")
    freq, z = preprocessing.readGamry(str(path))
    cols = {"Freq": [float(v) for v in freq], "Zreal": [float(v.real) for v in z], "Zimag": [float(v.imag) for v in z]}
    traces = []
    for label, vals in cols.items():
        traces.append({"trace": trace, "sweep": 0, "label": label, "n": len(vals),
                       "samples": [[i, None, vals[i]] for i in pick(len(vals))], "x_tol": 1e-9,
                       "y_tol_rel": 1e-12, "y_tol_abs": 0.0,
                       "source": f"impedance.py readGamry {path.name} (MIT, black box)"})
    out = Path(a.out) / f"{a.id}.json"
    out.write_text(json.dumps({"id": a.id, "format": "gamry-dta", "independent": True, "traces": traces}, indent=1) + "\n")
    print("wrote", out, len(cols["Freq"]), "points")


if __name__ == "__main__":
    main()
