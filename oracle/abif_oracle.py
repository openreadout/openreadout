#!/usr/bin/env python
"""Ground truth for Applied Biosystems ABIF files (`abif`: `.ab1`, `.fsa`, `.hid`) from Biopython's
AbiIO (BSD-3 / Biopython licence), an independent reader of the container.

Usage:  python abif_oracle.py [--out DIR] [--peakscanner CSV] --id ID FILE

Recorded: the sample id and sequence and quality values Biopython reports, the run start and
finish, and every tag Biopython decodes (`abif_raw`): text and scalar tags as values, numeric
arrays as their length, sum and values at up to 64 evenly spaced positions. With `--peakscanner`,
the rows of that PeakScanner export whose sample file is FILE's original name (`--name`, default
FILE's name): dye colour, size, height, data point.

Writes `<DIR>/<ID>.json`; `crates/openreadout-corpus-tests/tests/corpus/abif_oracle/mod.rs`
compares.
"""
from __future__ import annotations

import csv
import json
import math
import sys
from importlib.metadata import version
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import oracle_json  # noqa: E402

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "corpus" / "oracle" / "abif"


def text(v: bytes) -> str:
    try:
        s = v.decode()
    except UnicodeDecodeError:
        s = v.decode("latin-1")
    return s.rstrip(" \x00")


def value(v):
    """A tag value as JSON: text, a number, or an array summary."""
    if isinstance(v, bytes):
        return {"text": text(v)}
    if isinstance(v, bool):
        return {"number": int(v)}
    if isinstance(v, (int, float)):
        return {"number": float(v)} if math.isfinite(float(v)) else None
    if isinstance(v, str):
        return {"text": v}
    if isinstance(v, tuple) and v and all(isinstance(x, (int, float)) for x in v):
        n = len(v)
        idx = sorted({round(j * (n - 1) / 63) for j in range(64)}) if n > 1 else [0]
        return {"count": n, "sum": float(sum(v)), "at": [[i, float(v[i])] for i in idx]}
    return None


def main(argv: list[str]) -> int:
    from Bio import SeqIO

    out_dir, ident, files, scanner, name = OUT, None, [], None, None
    it = iter(argv)
    for a in it:
        if a == "--out":
            out_dir = Path(next(it))
        elif a == "--id":
            ident = next(it)
        elif a == "--peakscanner":
            scanner = Path(next(it))
        elif a == "--name":
            name = next(it)
        else:
            files.append(Path(a))
    if len(files) != 1 or not ident:
        print(__doc__, file=sys.stderr)
        return 2
    rec = SeqIO.read(str(files[0]), "abi")
    raw = rec.annotations.get("abif_raw", {})
    tags = {}
    for k, v in raw.items():
        j = value(v)
        if j is not None:
            tags[k] = j
    qual = rec.letter_annotations.get("phred_quality")
    out = {
        "id": ident,
        "format": "abif",
        "reader": f"Biopython {version('biopython')} (BSD-3)",
        "sample_id": rec.id,
        "sequence": str(rec.seq),
        "quality": list(qual) if qual else None,
        "run_start": rec.annotations.get("run_start"),
        "run_finish": rec.annotations.get("run_finish"),
        "tags": tags,
    }
    if scanner:
        want = name or files[0].name
        rows = []
        with open(scanner, newline="") as f:
            for r in csv.DictReader(f):
                if r["Sample Filename"] != want:
                    continue
                rows.append([
                    r["Dye Color"],
                    float(r["Size"].lstrip("*")),
                    float(r["Height"]),
                    float(r["Data Point"]),
                ])
        out["peakscanner"] = rows
    out_dir.mkdir(parents=True, exist_ok=True)
    oracle_json.write_text(out_dir / f"{ident}.json", json.dumps(out, indent=1, ensure_ascii=False) + "\n")
    print(ident, len(tags), "tags", len(out["sequence"]), "bases", len(out.get("peakscanner", [])), "peakscanner rows")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
