#!/usr/bin/env python
"""Ground truth for Shimadzu LabSolutions `.lcd` signals from chromConverter (GPL-3.0), run as a
black box through `shimadzu_chromconverter.R`.

Usage:  CHROMCONVERTER_LIB=/path/to/Rlib python shimadzu_oracle.py [--out DIR] --id ID FILE.lcd [...]

Writes `corpus/oracle/shimadzu/<ID>.json`: the chromatogram chromConverter returns (for PDA files
the max plot), as `{name, n, xxh3, first, last_rt_min, unit}` with values hashed as little-endian
float64 exactly as openreadout returns them, and the vendor peak table stored in the file (when
chromConverter finds one), largest peaks first. The corpus test
`crates/openreadout-corpus-tests/tests/shimadzu_signals.rs` compares them.
"""
from __future__ import annotations

import csv
import json
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np
import xxhash
sys.path.insert(0, str(Path(__file__).resolve().parent))
import oracle_json  # noqa: E402  (<id>.json, or <id>.json.gz over 1 MiB in corpus/oracle/heldout)

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "corpus" / "oracle" / "shimadzu"


def run(path: Path, what: str) -> tuple[list[dict], str]:
    with tempfile.TemporaryDirectory() as td:
        out = Path(td) / "x.csv"
        r = subprocess.run(["Rscript", str(HERE / "shimadzu_chromconverter.R"), str(path), what, str(out)],
                           capture_output=True, text=True)
        if not out.exists():
            return [], ""
        return list(csv.DictReader(out.open())), r.stdout.strip()


def main() -> None:
    args = sys.argv[1:]
    out_dir = OUT
    if args[:1] == ["--out"]:  # held-out ground truth goes to corpus/oracle/heldout (gen_heldout.py)
        out_dir = Path(args[1])
        args = args[2:]
    out_dir.mkdir(parents=True, exist_ok=True)
    while args:
        assert args.pop(0) == "--id"
        fid, path = args.pop(0), Path(args.pop(0))
        rows, version = run(path, "chroms")
        v = np.array([float(r["intensity"]) for r in rows], dtype="<f8")
        det = rows[0]["detector"] if rows else None
        name = "PDA max plot" if det == "PDA" else f"LC {det.replace('Chromatogram ', '')}" if det else None
        chrom = {
            "name": name,
            "n": int(v.size),
            "xxh3": xxhash.xxh3_128_hexdigest(v.tobytes()),
            "first": [float(x) for x in v[:8]],
            "last_rt_min": float(rows[-1]["rt"]) if rows else None,
            "unit": (rows[0].get("unit") or None) if rows else None,
        }
        peaks, _ = run(path, "peak_table")
        table = sorted(
            ({"rt_min": float(p["R.time"]), "area": float(p["Area"]), "height": float(p["Height"])} for p in peaks),
            key=lambda p: -p["area"],
        )
        data = {
            "id": fid,
            "file": path.name,
            "reader": f"chromConverter {version} read_chroms(format_in='shimadzu_lcd') via Rscript (GPL-3.0, run as a black box)",
            "chromatogram": chrom if rows else None,
            "peak_table": table[:20],
            "peak_count": len(table),
        }
        oracle_json.write_text(out_dir / f"{fid}.json", json.dumps(data, indent=1) + "\n")
        print("wrote", fid, chrom["n"] if rows else 0, "points,", len(table), "peaks")


if __name__ == "__main__":
    main()
