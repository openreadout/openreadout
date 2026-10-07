#!/usr/bin/env python
"""Ground truth for LightCycler 480 `.ixo` files from the LightCycler 480 software's own exports of
the same run, written by the depositor:

- the Cp table (`Experiment: ...  Selected Filter: ...`, then Include / Color / Pos / Name / Cp /
  Concentration / Standard / Status, tab-separated, decimal comma or point): per well the sample
  name and the Cp, or none;
- the raw-data export (`Raw Data ... (Run on LCS480 <version>)`, then SamplePos / SampleName /
  Prog# / Seg# / Cycle# / Time / Temp / <channel>...): per well and cycle the fluorescence as the
  software prints it, to two decimals.

The software's raw-data values are not the stored acquisition values: in the depositor's files
each reading equals the stored value times the acquisition's `ScalingFactor` divided by its
`RefValue` and `IntgrTime` (checked on 6 x 17,280 readings, all within the export's rounding).
Whether a reader returns stored or scaled values is its choice; the comparison has to say which.

Written to corpus/oracle/qpcr-roche-export/<id>.json:
  {"software": "...", "channels": [...], "wells": [{"pos": "A1", "name": "...", "cp": 33.59 | null,
   "raw": {"465-510": [cycle 1, cycle 2, ...]}}], "tol_cp": 0.005, "tol_raw": 0.005}

Usage:  python lc480_export_oracle.py --id ID [--cp CP.txt] RAW.txt
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "corpus" / "oracle" / "qpcr-roche-export"


def num(s: str) -> float | None:
    s = s.strip().replace(",", ".")
    try:
        return float(s)
    except ValueError:
        return None


def lines(path: Path) -> list[list[str]]:
    data = path.read_bytes()
    for enc in ("utf-8", "cp1252"):
        try:
            text = data.decode(enc)
            break
        except UnicodeDecodeError:
            continue
    return [l.rstrip("\r\n").split("\t") for l in text.splitlines()]


def main(argv: list[str]) -> int:
    ident, cp_path, files = None, None, []
    it = iter(argv)
    for a in it:
        if a == "--id":
            ident = next(it)
        elif a == "--cp":
            cp_path = Path(next(it))
        else:
            files.append(Path(a))
    if not ident or len(files) != 1:
        print(__doc__, file=sys.stderr)
        return 2
    raw = lines(files[0])
    m = re.search(r"Run on LCS480 ([\d.]+)", "\t".join(raw[0]))
    head = [h.strip() for h in raw[1]]
    chans = [h for h in head[7:] if h]
    wells: dict[str, dict] = {}
    for r in raw[2:]:
        if len(r) < 8 or not r[0].strip():
            continue
        w = wells.setdefault(r[0].strip(), {"pos": r[0].strip(), "name": r[1].strip(), "cp": None,
                                            "raw": {c: [] for c in chans}})
        cyc = int(r[4])
        for i, c in enumerate(chans):
            v = num(r[7 + i])
            arr = w["raw"][c]
            while len(arr) < cyc:
                arr.append(None)
            arr[cyc - 1] = v
    if cp_path:
        cp = lines(cp_path)
        h = [x.strip() for x in cp[1]]
        for r in cp[2:]:
            if len(r) <= h.index("Cp"):
                continue
            pos = r[h.index("Pos")].strip()
            if pos in wells:
                wells[pos]["cp"] = num(r[h.index("Cp")])
                if wells[pos]["name"] != r[h.index("Name")].strip():
                    print("name differs between exports at", pos, file=sys.stderr)
    out = {"id": ident, "format": "roche-lightcycler-ixo", "independent": True,
           "reader": "the LightCycler 480 software's exports of the same run (vendor software)",
           "software": m.group(1) if m else None, "channels": chans,
           "raw_scale": "export = stored x ScalingFactor / (RefValue x IntgrTime)",
           "cp_source": cp_path.name if cp_path else None, "raw_source": files[0].name,
           "tol_cp": 0.005, "tol_raw": 0.005, "wells": list(wells.values())}
    OUT.mkdir(parents=True, exist_ok=True)
    dst = OUT / f"{ident}.json"
    dst.write_text(json.dumps(out, ensure_ascii=False) + "\n")
    print(dst, len(wells), "wells", sum(1 for w in wells.values() if w["cp"] is not None), "with Cp")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
