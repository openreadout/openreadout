#!/usr/bin/env python
"""Ground truth for delimited text exports read as one table (`biorad-ddpcr`): Python's own
`csv` module (standard library) reads the file, with no code shared with the Rust reader.

Usage:  python export_tables.py [--out DIR] --id ID --format FORMAT FILE

Per format, the header line is found as the vendor writes it:

- `biorad-ddpcr`: a results export's header is its first line; an amplitude export's header is
  the first line whose first cell ends in `Amplitude` (after QX Manager's `Target Value of`
  lines).

Recorded: the header titles in order, the number of data rows (blank lines and lines of only
commas skipped), and for every titled column the cells of up to 96 rows spread over the file
(all rows when there are fewer): a number when Python's `float` reads the cell, the text when it
does not, `null` when the cell is empty. Writes `<DIR>/<ID>.json`;
`crates/openreadout-corpus-tests/tests/corpus/export_table_oracle/mod.rs` compares.
"""
from __future__ import annotations

import csv
import io
import json
import math
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "corpus" / "oracle" / "export-tables"
SAMPLE_ROWS = 96


def decode(b: bytes) -> str:
    if b.startswith(b"\xff\xfe") or b.startswith(b"\xfe\xff"):
        return b.decode("utf-16")
    if b.startswith(b"\xef\xbb\xbf"):
        b = b[3:]
    try:
        return b.decode("utf-8")
    except UnicodeDecodeError:
        return b.decode("latin-1")


def header_index(fmt: str, lines: list[str]) -> int:
    if fmt == "biorad-ddpcr":
        for i, l in enumerate(lines[:16]):
            first = l.split(",")[0].strip().replace(" ", "").lower()
            if first.endswith("amplitude"):
                return i
        return 0
    raise SystemExit(f"no header rule for format {fmt}")


def cell(c: str):
    t = c.strip()
    if not t:
        return None
    try:
        v = float(t)
    except ValueError:
        return t
    return v if math.isfinite(v) else t


def read(path: Path, fmt: str) -> dict:
    text = decode(path.read_bytes())
    lines = text.splitlines()
    h = header_index(fmt, lines)
    rows = list(csv.reader(io.StringIO("\n".join(lines[h:]))))
    titles = rows[0]
    while titles and not titles[-1].strip():
        titles.pop()
    body = [r for r in rows[1:] if any(c.strip() for c in r)]
    n = len(body)
    pick = range(n) if n <= SAMPLE_ROWS else sorted({round(j * (n - 1) / (SAMPLE_ROWS - 1)) for j in range(SAMPLE_ROWS)})
    cells = {}
    for k, t in enumerate(titles):
        if not t.strip():
            continue
        cells[t.strip()] = [[i, cell(body[i][k]) if k < len(body[i]) else None] for i in pick]
    return {"header_line": h, "columns": [t.strip() for t in titles], "rows": n, "cells": cells}


def main(argv: list[str]) -> int:
    out_dir, ident, fmt, files = OUT, None, None, []
    it = iter(argv)
    for a in it:
        if a == "--out":
            out_dir = Path(next(it))
        elif a == "--id":
            ident = next(it)
        elif a == "--format":
            fmt = next(it)
        else:
            files.append(Path(a))
    if len(files) != 1 or not ident or not fmt:
        print(__doc__, file=sys.stderr)
        return 2
    o = {"id": ident, "format": fmt, "reader": f"Python {sys.version.split()[0]} csv module",
         "independent": True}
    o.update(read(files[0], fmt))
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / f"{ident}.json").write_text(json.dumps(o, indent=1, ensure_ascii=False) + "\n")
    print(f"{ident}: {o['rows']} rows, {len(o['columns'])} columns")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
