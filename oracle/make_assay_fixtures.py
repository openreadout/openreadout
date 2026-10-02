#!/usr/bin/env python
"""Write the synthetic plate-analysis fixtures (known truth) used by `openreadout analyze assay` tests and
evals.

Usage:  python oracle/make_assay_fixtures.py [--check]

Plain Python only (no OpenReadout, no NumPy): `random.Random(seed)` is stable across Python
versions, and every value is written with a fixed number of decimals, so the files are
byte-for-byte reproducible. Each file is written to `crates/openreadout-assay/tests/fixtures/`
(committed); the plate matrices and their layouts are also copied into the corpus directory
(`OPENREADOUT_CORPUS_DIR`, default
`corpus/files`) under the `assay-synth-*` names the manifest lists, so evals can stage them.
`--check` exits 1 when a committed fixture differs from what this script writes.

The fixtures (truth in docs/provenance/plate-analysis.md):

* `assay-synth-dose-response.csv` — a 96-well plate matrix (the generic `plate` dialect, one
  luminescence read, RLU) and `assay-synth-dose-response-layout.csv` (long layout: well, role,
  compound, concentration in µM). Columns 1–10: a 10-point 1:3 dilution series from 10 µM; rows
  A–C compound CPD-A, D–F CPD-B, G–H CPD-C; column 11 negative controls (vehicle, 100 % signal);
  column 12 positive controls (full inhibition). Signal = bottom + (top − bottom) / (1 + (x/IC50)^h)
  with 4 % multiplicative Gaussian noise.
* `assay-synth-elisa-5pl.csv` — a 96-well absorbance plate matrix (OD) and
  `assay-synth-elisa-5pl-layout.csv` (plate-map grid: `role`, `sample`, `concentration`,
  `dilution` blocks). Standards (7 levels, 1:2 from 1000 pg/mL, duplicates) in columns 1–2 of
  rows A–G, blanks in H1/H2; 40 unknowns in columns 3–12 (duplicates) at known
  concentrations (every fourth diluted 1:5); OD = 5PL(x) + 0.045 (plate background) with 3 %
  multiplicative noise.
* `assay-synth-kinetics.csv` — a long CSV (`well,time_min,value`): 24 wells, 31 reads 1 min apart,
  enzyme progress curves with a lag, a linear phase and substrate depletion, plus 0.002 OD noise.
* `assay-synth-growth.csv` — a long CSV (`well,time_h,value,role`): 14 wells of OD600 every 15 min
  for 24 h: 12 logistic growth curves (K, r, lag differ) on a 0.09 background and two blank wells
  (role `blank`), with 0.003 OD noise.
"""
from __future__ import annotations

import argparse
import math
import os
import random
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "crates" / "openreadout-assay" / "tests" / "fixtures"
# the plate-matrix fixtures the corpus manifest lists (staged by evals); the long CSVs are test-only
IN_CORPUS = {
    "assay-synth-dose-response.csv",
    "assay-synth-dose-response-layout.csv",
    "assay-synth-elisa-5pl.csv",
    "assay-synth-elisa-5pl-layout.csv",
}
ROWS = "ABCDEFGH"


def matrix_csv(values: dict[str, str], title: str) -> str:
    """A 96-well plate matrix: header `title,1,…,12`, rows A–H."""
    out = [",".join([title] + [str(c) for c in range(1, 13)])]
    for r in ROWS:
        out.append(",".join([r] + [values.get(f"{r}{c}", "") for c in range(1, 13)]))
    return "\n".join(out) + "\n"


def four_pl(x: float, top: float, bottom: float, ic50: float, h: float) -> float:
    return bottom + (top - bottom) / (1.0 + (x / ic50) ** h)


# ------------------------------------------------------------------ dose-response
DOSE_COMPOUNDS = [
    # compound, rows, IC50 (µM), Hill, top, bottom
    ("CPD-A", "ABC", 0.25, 1.0, 100000.0, 2000.0),
    ("CPD-B", "DEF", 1.8, 1.5, 100000.0, 2000.0),
    ("CPD-C", "GH", 3.0, 0.8, 100000.0, 20000.0),
]
DOSES = [10.0 / 3**i for i in range(10)]  # columns 1..10


def dose_response() -> tuple[str, str]:
    rng = random.Random(20260923)
    vals: dict[str, str] = {}
    lay = ["well,role,compound,concentration"]
    for name, rows, ic50, h, top, bottom in DOSE_COMPOUNDS:
        for r in rows:
            for c, x in enumerate(DOSES, start=1):
                y = four_pl(x, top, bottom, ic50, h) * (1.0 + 0.04 * rng.gauss(0.0, 1.0))
                vals[f"{r}{c}"] = f"{y:.0f}"
                lay.append(f"{r}{c},sample,{name},{x:.6g}")
    for r in ROWS:
        vals[f"{r}11"] = f"{100000.0 * (1.0 + 0.04 * rng.gauss(0.0, 1.0)):.0f}"
        lay.append(f"{r}11,negative,,")
        vals[f"{r}12"] = f"{2000.0 * (1.0 + 0.08 * rng.gauss(0.0, 1.0)):.0f}"
        lay.append(f"{r}12,positive,,")
    return matrix_csv(vals, "RLU"), "\n".join(lay) + "\n"


# ------------------------------------------------------------------ ELISA 5PL
ELISA = {"a": 0.02, "b": 1.4, "c": 180.0, "d": 3.2, "g": 0.6}  # y = d + (a − d)/(1 + (x/c)^b)^g
ELISA_BACKGROUND = 0.045
ELISA_STANDARDS = [1000.0 / 2**i for i in range(7)]  # rows A–G; row H = blank


def five_pl(x: float) -> float:
    p = ELISA
    return p["d"] + (p["a"] - p["d"]) / (1.0 + (x / p["c"]) ** p["b"]) ** p["g"]


def elisa() -> tuple[str, str, list[tuple[str, float]]]:
    rng = random.Random(5150)
    vals: dict[str, str] = {}
    role = {}
    sample = {}
    conc = {}
    dil = {}
    for i, r in enumerate(ROWS):
        for c in (1, 2):
            w = f"{r}{c}"
            if r == "H":
                x = 0.0
                role[w] = "blank"
                sample[w] = "Blank"
            else:
                x = ELISA_STANDARDS[i]
                role[w] = "standard"
                sample[w] = f"STD{i + 1}"
                conc[w] = f"{x:g}"
            vals[w] = f"{(five_pl(x) + ELISA_BACKGROUND) * (1.0 + 0.03 * rng.gauss(0.0, 1.0)):.4f}"
    truth: list[tuple[str, float]] = []
    k = 0
    for r in ROWS:
        for c in range(3, 13, 2):
            k += 1
            x = math.exp(rng.uniform(math.log(8.0), math.log(900.0)))
            name = f"S{k:02d}"
            d = 5 if k % 4 == 0 else 1
            truth.append((name, x * d))
            for cc in (c, c + 1):
                w = f"{r}{cc}"
                role[w] = "sample"
                sample[w] = name
                if d != 1:
                    dil[w] = f"1:{d}"
                vals[w] = f"{(five_pl(x) + ELISA_BACKGROUND) * (1.0 + 0.03 * rng.gauss(0.0, 1.0)):.4f}"
    blocks = []
    for title, m in (("role", role), ("sample", sample), ("concentration (pg/mL)", conc), ("dilution", dil)):
        blocks.append(matrix_csv(m, title))
    return matrix_csv(vals, "OD450"), "\n".join(blocks), truth


# ------------------------------------------------------------------ kinetics
def kinetics() -> str:
    rng = random.Random(777)
    lines = ["well,time_min,value"]
    for i in range(24):
        w = f"{ROWS[i // 6]}{i % 6 + 1}"
        v0 = 0.02 + 0.004 * i  # initial rate, OD/min
        lag = 1.0 + (i % 4)  # min
        cap = 1.2 + 0.05 * (i % 3)  # OD at substrate exhaustion
        base = 0.05
        for t in range(31):
            tt = max(0.0, t - lag)
            # linear phase that saturates exponentially at `cap`
            y = base + cap * (1.0 - math.exp(-v0 * tt / cap))
            y += 0.002 * rng.gauss(0.0, 1.0)
            lines.append(f"{w},{t},{y:.4f}")
    return "\n".join(lines) + "\n"


# ------------------------------------------------------------------ growth
def growth() -> str:
    rng = random.Random(4242)
    lines = ["well,time_h,value,role"]
    wells = [f"{r}{c}" for r in "AB" for c in range(1, 8)]
    for i, w in enumerate(wells):
        role = "blank" if i in (6, 13) else "sample"
        k = 0.8 + 0.05 * (i % 5)
        r = 0.6 + 0.1 * (i % 6)
        n0 = 0.004 + 0.001 * (i % 3)
        for j in range(97):
            t = 0.25 * j
            od = 0.09 if role == "blank" else 0.09 + k / (1.0 + ((k - n0) / n0) * math.exp(-r * t))
            od += 0.003 * rng.gauss(0.0, 1.0)
            lines.append(f"{w},{t:.2f},{od:.4f},{role}")
    return "\n".join(lines) + "\n"


def outputs() -> dict[str, str]:
    dr, dr_lay = dose_response()
    el, el_lay, _ = elisa()
    return {
        "assay-synth-dose-response.csv": dr,
        "assay-synth-dose-response-layout.csv": dr_lay,
        "assay-synth-elisa-5pl.csv": el,
        "assay-synth-elisa-5pl-layout.csv": el_lay,
        "assay-synth-kinetics.csv": kinetics(),
        "assay-synth-growth.csv": growth(),
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true", help="exit 1 if a committed fixture differs")
    args = ap.parse_args()
    corpus = Path(os.environ.get("OPENREADOUT_CORPUS_DIR", ROOT / "corpus" / "files"))
    bad = 0
    for name, text in outputs().items():
        path = FIXTURES / name
        if args.check:
            if not path.exists() or path.read_text(encoding="utf-8") != text:
                print(f"DIFFERS {path.relative_to(ROOT)}")
                bad += 1
            continue
        FIXTURES.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        if corpus.is_dir() and name in IN_CORPUS:
            shutil.copyfile(path, corpus / name)
        print(f"wrote {path.relative_to(ROOT)} ({len(text)} bytes)")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
