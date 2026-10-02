#!/usr/bin/env python
"""Ground truth for MicroCal `.itc` files (`microcal-itc`) from the vendor software's own exports
and the depositors' records — standard library only (no ITC reader is used).

Usage:  python itc_oracle.py [--out DIR] --id ID FILE [--raw EXPORT] [--heats EXPORT] [--fact KEY=VALUE ...] ...

- `--raw`: Origin's raw export of the same run (`<name>RAW.TXT`/`RAW.DAT`: a header line, then
  time (s) and differential power (µcal/s) per row): the row count, and time and power at up to
  256 evenly spaced rows. The export prints five decimals.
- `--heats`: Origin's integrated-heats table (`DH INJV Xt Mt XMt NDH`): the injection volumes
  (INJV, µl) and the cell concentration of the first row (Mt, mM, before dilution by the first
  injection).
- `--fact`: a value the depositor states outside the file (file name, README, lab notebook), e.g.
  `cell_concentration_mM=0.217` from `…_217uMcell_…`; each is recorded with its `--fact-source`.

Writes `<DIR>/<ID>.json`; `crates/openreadout-corpus-tests/tests/itc_oracle/mod.rs` compares.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parent))
import oracle_json  # noqa: E402  (<id>.json, or <id>.json.gz over 1 MiB in corpus/oracle/heldout)

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "corpus" / "oracle" / "itc"


def raw_export(p: Path) -> dict:
    rows = []
    for line in p.read_text(encoding="latin-1").splitlines():
        f = line.split()
        try:
            rows.append((float(f[0]), float(f[1])))
        except (ValueError, IndexError):
            continue
    n = len(rows)
    idx = sorted({round(i * (n - 1) / 255) for i in range(256)}) if n else []
    return {"file": p.name, "n": n, "decimals": 5, "samples": [[i, rows[i][0], rows[i][1]] for i in idx]}


def heats_export(p: Path) -> dict:
    lines = [l.split("\t") for l in p.read_text(encoding="latin-1").splitlines() if l.strip()]
    head = [h.strip() for h in lines[0]]
    vol, mt = [], None
    for row in lines[1:]:
        rec = dict(zip(head, [c.strip() for c in row]))
        try:
            vol.append(float(rec["INJV"]))
        except (KeyError, ValueError):
            continue
        if mt is None:
            try:
                mt = float(rec["Mt"])
            except (KeyError, ValueError):
                pass
    return {"file": p.name, "injection_volumes_ul": vol, "cell_concentration_mM": mt}


def main() -> None:
    args = sys.argv[1:]
    out_dir = OUT
    if args[:1] == ["--out"]:
        out_dir, args = Path(args[1]), args[2:]
    out_dir.mkdir(parents=True, exist_ok=True)
    cur = None
    todo = []
    while args:
        a = args.pop(0)
        if a == "--id":
            cur = {"id": args.pop(0), "file": Path(args.pop(0)), "facts": {}, "fact_source": ""}
            todo.append(cur)
        elif a == "--raw":
            cur["raw"] = Path(args.pop(0))
        elif a == "--heats":
            cur["heats"] = Path(args.pop(0))
        elif a == "--fact":
            k, v = args.pop(0).split("=", 1)
            cur["facts"][k] = float(v)
        elif a == "--fact-source":
            cur["fact_source"] = args.pop(0)
        else:
            sys.exit(__doc__)
    for t in todo:
        data = {"file": t["file"].name}
        if "raw" in t:
            data["raw_export"] = raw_export(t["raw"])
        if "heats" in t:
            data["heats_export"] = heats_export(t["heats"])
        if t["facts"]:
            data["depositor_facts"] = {**t["facts"], "source": t["fact_source"]}
        oracle_json.write_text(out_dir / f"{t['id']}.json", json.dumps(data, indent=1) + "\n")
        print("wrote", t["id"], ", ".join(k for k in data if k != "file"))


if __name__ == "__main__":
    main()
