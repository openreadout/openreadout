#!/usr/bin/env python
"""Ground truth for a QuantaSoft `.qlp` plate (`biorad-qlp`) from QuantaSoft's own results export
of the same plate (the CSV the depositor published next to it), read with Python's `csv` module.
No reader of the `.qlp` is run.

Usage:  python qlp_oracle.py [--out DIR] --id ID --export RESULTS.csv

Recorded per exported row (well and target): the well, sample, target type and target, status,
the concentration as written, accepted droplets, positives and negatives, the four
double-channel counts, and the mean amplitudes of the positive and negative droplets. Writes
`<DIR>/<ID>.json`; `crates/openreadout-corpus-tests/tests/corpus/export_table_oracle/mod.rs`
compares.
"""
from __future__ import annotations

import csv
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "corpus" / "oracle" / "export-tables"


def num(s: str):
    s = (s or "").strip()
    if not s:
        return None
    try:
        return float(s)
    except ValueError:
        return s


def read(export: Path) -> list[dict]:
    rows = []
    with open(export, encoding="utf-8-sig", newline="") as f:
        for r in csv.DictReader(f):
            g = lambda *ks: next((r[k] for k in ks if k in r), "")
            rows.append({
                "well": g("Well").strip(),
                "sample": g("Sample").strip(),
                "target_type": g("TargetType", "TypeAssay").strip(),
                "target": g("Target", "Assay").strip(),
                "status": g("Status").strip(),
                "concentration": num(g("Concentration")),
                "accepted": num(g("AcceptedDroplets")),
                "positives": num(g("Positives")),
                "negatives": num(g("Negatives")),
                "ch1_pos_ch2_pos": num(g("Ch1+Ch2+")),
                "ch1_pos_ch2_neg": num(g("Ch1+Ch2-")),
                "ch1_neg_ch2_pos": num(g("Ch1-Ch2+")),
                "ch1_neg_ch2_neg": num(g("Ch1-Ch2-")),
                "mean_amplitude_positives": num(g("MeanAmplitudeofPositives", "MeanAmplitudeOfPositives")),
                "mean_amplitude_negatives": num(g("MeanAmplitudeofNegatives", "MeanAmplitudeOfNegatives")),
            })
    return rows


def main(argv: list[str]) -> int:
    out_dir, ident, export = OUT, None, None
    it = iter(argv)
    for a in it:
        if a == "--out":
            out_dir = Path(next(it))
        elif a == "--id":
            ident = next(it)
        elif a == "--export":
            export = Path(next(it))
    if not ident or not export:
        print(__doc__, file=sys.stderr)
        return 2
    o = {"id": ident, "format": "biorad-qlp", "independent": True,
         "reader": f"QuantaSoft's results export ({export.name}), read with Python {sys.version.split()[0]} csv",
         "wells": read(export)}
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / f"{ident}.json").write_text(json.dumps(o, indent=1, ensure_ascii=False) + "\n")
    print(f"{ident}: {len(o['wells'])} exported rows")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
