#!/usr/bin/env python
"""Ground truth for GenePix Results `.gpr` files (`genepix-gpr`) from pandas (BSD-3), an
independent reader of the tab-separated feature table.

Usage:  python gpr_oracle.py [--out DIR] --id ID FILE

Recorded: the row and column counts, the column titles, for every numeric column (all cells numbers
or `Error`) its values at up to 64 evenly spaced rows (`Error` as null), the `Name` column at the
same rows, and the `Creator` and `Scanner` header records.

Writes `<DIR>/<ID>.json`; `crates/openreadout-corpus-tests/tests/gpr_oracle/mod.rs` compares.
"""
from __future__ import annotations

import io
import json
import math
import sys
from importlib.metadata import version
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parent))
import oracle_json  # noqa: E402  (<id>.json, or <id>.json.gz over 1 MiB in corpus/oracle/heldout)

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "corpus" / "oracle" / "gpr"


def main(argv: list[str]) -> int:
    import pandas as pd

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
    text = files[0].read_bytes().decode("latin-1")
    lines = text.splitlines()
    n_header = int(lines[1].split()[0])
    header = {}
    for l in lines[2 : 2 + n_header]:
        k, _, v = l.strip().strip('"').partition("=")
        header[k] = v
    df = pd.read_csv(io.StringIO("\n".join(lines[2 + n_header :])), sep="\t", dtype=str, keep_default_na=False)
    n = len(df)
    idx = sorted({round(j * (n - 1) / 63) for j in range(64)}) if n else []
    numeric = {}
    for c in df.columns:
        vals = pd.to_numeric(df[c].where(df[c] != "Error"), errors="coerce")
        bad = df[c][vals.isna() & (df[c] != "Error") & (df[c] != "")]
        if len(bad) == 0 and vals.notna().any():
            numeric[c] = [[i, None if math.isnan(vals.iloc[i]) else float(vals.iloc[i])] for i in idx]
    out = {
        "id": ident,
        "format": "genepix-gpr",
        "reader": f"pandas {version('pandas')} (BSD-3)",
        "rows": n,
        "columns": list(df.columns),
        "numeric": numeric,
        "names": [[i, df["Name"].iloc[i]] for i in idx] if "Name" in df.columns else [],
        "creator": header.get("Creator"),
        "scanner": header.get("Scanner"),
    }
    out_dir.mkdir(parents=True, exist_ok=True)
    oracle_json.write_text(out_dir / f"{ident}.json", json.dumps(out, indent=1, ensure_ascii=False) + "\n")
    print(ident, n, "rows", len(df.columns), "columns", len(numeric), "numeric")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
