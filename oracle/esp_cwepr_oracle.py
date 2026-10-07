#!/usr/bin/env python
"""Ground truth for Bruker ESP/EMX (WinEPR) `.par`/`.spc` pairs read with cwepr, compared by
`crates/openreadout-corpus-tests/tests/corpus/series_oracle/` like the other series oracles.

Source, never our reader: cwepr 0.5.1 (BSD-2-Clause, https://github.com/tillbiskup/cwepr), its
`ESPWinEPRImporter` run as a black box: the `.spc` values it imports and, for a single spectrum,
the magnetic-field axis it builds from the `.par` (mT, converted here to G, the unit of the
`.par`). Two things are worked around, not reimplemented:

- cwepr refuses a `.par` whose `JDA`/`JTM` date it cannot parse; its date step is skipped (the
  date is not compared here);
- cwepr imports a 2D file (`SSX`/`SSY` in the `.par`) as one flat vector and builds a 1D axis
  for it. The vector is cut into `SSY` sweeps of `SSX` values, the numbers cwepr read from the
  `.par`, and no abscissa is compared for 2D files.

Usage (a venv with cwepr; `setuptools<81` for aspecd's pkg_resources import):
    python oracle/esp_cwepr_oracle.py --id ID corpus/files/ID.par [--out DIR]

Writes `<DIR>/<ID>.json` (default `corpus/oracle/series/`), the same shape `series_oracle.py`
writes: rows `[i, x, y]` sampled at up to 256 evenly spaced indices per compared sweep.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import oracle_json  # noqa: E402
from series_oracle import pick  # noqa: E402

OUT = HERE.parent / "corpus" / "oracle" / "series"


def cwepr_traces(par: Path) -> list[dict]:
    import numpy as np
    import cwepr
    import cwepr.dataset
    import cwepr.io.esp_winepr as esp

    esp.ESPWinEPRImporter._extract_datetime = lambda self: None  # see the module docstring
    imp = esp.ESPWinEPRImporter(source=str(par))
    ds = cwepr.dataset.ExperimentalDataset()
    ds.import_from(imp)
    y = np.asarray(ds.data.data, dtype=float).ravel()
    pars = imp._par_dict
    src = f"cwepr {getattr(cwepr, '__version__', '0.5.1')} ESPWinEPRImporter (BSD-2-Clause, black box; {imp.parameters['format']})"
    nx, ny = int(pars.get("SSX", 0) or 0), int(pars.get("SSY", 0) or 0)
    out = []
    if nx and ny > 1:
        if nx * ny != len(y):
            raise RuntimeError(f"cwepr read {len(y)} values, the .par says SSX {nx} x SSY {ny}")
        sweeps = y.reshape(ny, nx)
        for s in sorted({0, ny // 2, ny - 1}):
            out.append({"trace": 0, "sweep": s, "sweeps": ny, "channel": "intensity", "n": nx,
                        "samples": [[i, None, float(sweeps[s, i])] for i in pick(nx)],
                        "y_tol_rel": 1e-9, "y_tol_abs": 0.0, "source": src})
        return out
    x = np.asarray(ds.data.axes[0].values, dtype=float)
    if ds.data.axes[0].unit == "mT":
        x = x * 10.0  # G, as the .par states the field
    out.append({"trace": 0, "sweep": 0, "channel": "intensity", "n": len(y),
                "samples": [[i, float(x[i]), float(y[i])] for i in pick(len(y))],
                "x_tol": 1e-6, "y_tol_rel": 1e-9, "y_tol_abs": 0.0, "source": src})
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--id", required=True)
    ap.add_argument("--out", type=Path, default=OUT)
    ap.add_argument("par", type=Path)
    a = ap.parse_args()
    try:
        o = {"id": a.id, "format": "bruker-esp", "independent": True, "traces": cwepr_traces(a.par)}
    except Exception as e:  # the independent reader could not read it: an oracle error, recorded
        o = {"id": a.id, "format": "bruker-esp", "error": f"{type(e).__name__}: {e}"}
    a.out.mkdir(parents=True, exist_ok=True)
    oracle_json.write_text(a.out / f"{a.id}.json", json.dumps(o, indent=1) + "\n")
    print(f"{a.id}: {len(o.get('traces', []))} trace comparisons" if "traces" in o else f"{a.id}: oracle error {o['error']}")


if __name__ == "__main__":
    main()
