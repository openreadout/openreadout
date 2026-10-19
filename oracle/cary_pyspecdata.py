#!/usr/bin/env python
"""Ground truth for Agilent Cary UV-Vis files (`.dsw`, `.bsw`, `.bsk`) without a vendor export:
pySpecData's `load_cary` (BSD-3-Clause, https://github.com/jmfrancklab/pyspecdata), run as a
black box. It returns one spectrum per spectrum store, in file order, and skips the baseline
store, so its spectra are the sweeps of our trace 0. Every sampled point's wavelength and
absorbance is compared (`crates/openreadout-corpus-tests/tests/corpus/series_oracle/mod.rs`).

pySpecData pulls in matplotlib, PyTables and sympy, so it runs in its own environment:

  uv run --no-project --python 3.12 --with pyspecdata==0.9.45.16 python oracle/cary_pyspecdata.py \
      --id ID FILE [--out DIR]

Writes `<DIR>/<ID>.json` (default `corpus/oracle/series/`).
"""
from __future__ import annotations

import argparse
import contextlib
import io
import json
import sys
from importlib.metadata import version
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import oracle_json  # noqa: E402

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "corpus" / "oracle" / "series"
ROWS = 256


def pick(n: int) -> list[int]:
    if n <= 0:
        return []
    return sorted({round(i * (n - 1) / (ROWS - 1)) for i in range(ROWS)}) if n > ROWS else list(range(n))


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--id", required=True)
    ap.add_argument("--out", type=Path, default=OUT)
    ap.add_argument("file", type=Path)
    a = ap.parse_args()
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        from pyspecdata.load_files.load_cary import load_cary

        spectra = load_cary(str(a.file))
    traces = []
    for sweep, (name, d) in enumerate(spectra.items()):
        x = [float(v) for v in d.getaxis(d.dimlabels[0])]
        y = [float(v) for v in d.data]
        traces.append({
            "trace": 0,
            "sweep": sweep,
            "sweeps": len(spectra),
            "channel": "absorbance",
            "n": len(y),
            "samples": [[i, x[i], y[i]] for i in pick(len(y))],
            # both readers return the stored float32 values
            "x_tol": 1e-4,
            "y_tol_rel": 1e-7,
            "y_tol_abs": 1e-9,
            "source": f"pySpecData {version('pyspecdata')} load_cary, spectrum {name!r}",
        })
    o = {"id": a.id, "format": "agilent-cary", "independent": True,
         "reader": f"pySpecData {version('pyspecdata')} (BSD-3-Clause) load_cary",
         "traces": traces}
    a.out.mkdir(parents=True, exist_ok=True)
    oracle_json.write_text(a.out / f"{a.id}.json", json.dumps(o, indent=1) + "\n")
    print(f"{a.id}: {len(traces)} spectra")


if __name__ == "__main__":
    main()
