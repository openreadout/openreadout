#!/usr/bin/env python
"""Ground truth for Malvern Zetasizer `.dts` files (`malvern-zetasizer-dts`) from the Zetasizer
software's own export of the same records, written by the depositor: the tab-separated text
export (`File > Export`, decimal comma or point, any language) or a spreadsheet copy of the
records table (`.xlsx`, a header row, an optional units row).

Recorded, in `oracle/series_oracle.py`'s table format (`crates/openreadout-corpus-tests/tests/
series_oracle/mod.rs` compares): per exported record, in record-number order (the export must
list every record of the file: `rows`), the sample name, the measurement date and time (within a
second: the export rounds to the second), the temperature, and what the export holds of Z-average,
PdI, intensity peaks 1-3 (mean, area; width from %Pd of peak 1), zeta potential, mobility and
conductivity, each within half a unit of the export's last digit. Peaks the export lists as 0
are absent.

Usage:  python zetasizer_oracle.py --id ID EXPORT [--out DIR]
"""
from __future__ import annotations

import datetime as dt
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "corpus" / "oracle" / "series"

MONTHS = {
    # Polish (genitive), Portuguese, English, German, French, Spanish
    "stycznia": 1, "lutego": 2, "marca": 3, "kwietnia": 4, "maja": 5, "czerwca": 6, "lipca": 7,
    "sierpnia": 8, "września": 9, "października": 10, "listopada": 11, "grudnia": 12,
    "janeiro": 1, "fevereiro": 2, "março": 3, "abril": 4, "maio": 5, "junho": 6, "julho": 7,
    "agosto": 8, "setembro": 9, "outubro": 10, "novembro": 11, "dezembro": 12,
    "january": 1, "february": 2, "march": 3, "april": 4, "may": 5, "june": 6, "july": 7,
    "august": 8, "september": 9, "october": 10, "november": 11, "december": 12,
}

# export column (prefix, case-insensitive) -> our column
COLUMNS = [
    ("z-ave", "z_average"), ("pdi", "pdi"),
    ("pk 1 mean int", "peak1_mean"), ("pk 2 mean int", "peak2_mean"), ("pk 3 mean int", "peak3_mean"),
    ("pk 1 area int", "peak1_area"), ("pk 2 area int", "peak2_area"), ("pk 3 area int", "peak3_area"),
    ("zeta potential", "zeta_potential"), ("zp", "zeta_potential"),
    ("mob", "mobility"), ("cond", "conductivity"), ("t (", "temperature"), ("t", "temperature"),
]


def date_of(text: str) -> str | None:
    """A long-form local date ('czwartek, 8 maja 2025 15:59:54', 'segunda-feira, 13 de outubro de
    2025 14:37:53', 'Thursday, May 8, 2025 3:59:54 PM') as ISO without a zone."""
    t = text.lower()
    month = next((m for name, m in MONTHS.items() if re.search(rf"\b{name}\b", t)), None)
    time = re.search(r"(\d{1,2}):(\d{2}):(\d{2})\s*(am|pm)?", t)
    year = re.search(r"\b(19|20)\d{2}\b", t)
    if month is None or time is None or year is None:
        return None
    rest = t[: time.start()].replace(year.group(0), " ")
    day = next((int(d) for d in re.findall(r"\b(\d{1,2})\b", rest) if 1 <= int(d) <= 31), None)
    if day is None:
        return None
    h, mi, s = int(time.group(1)), int(time.group(2)), int(time.group(3))
    if time.group(4) == "pm" and h < 12:
        h += 12
    if time.group(4) == "am" and h == 12:
        h = 0
    return dt.datetime(int(year.group(0)), month, day, h, mi, s).isoformat()


def half_unit(v) -> float:
    """Half the last digit's unit of a value as the export wrote it (its rounding)."""
    s = (repr(v) if isinstance(v, float) else str(v)).strip().replace(",", ".")
    if "e" in s.lower():
        return abs(float(s)) * 5e-4
    dec = len(s.split(".")[1]) if "." in s else 0
    return 0.5 * 10.0 ** -dec * 1.0001


def num(v) -> float | None:
    if v is None:
        return None
    if isinstance(v, (int, float)):
        return float(v)
    s = str(v).strip().replace(" ", "").replace(",", ".")
    try:
        return float(s)
    except ValueError:
        return None


def rows_of(path: Path) -> list[dict]:
    if path.suffix.lower() == ".xlsx":
        import openpyxl  # type: ignore

        ws = openpyxl.load_workbook(path, data_only=True).worksheets[0]
        raw = [list(r) for r in ws.iter_rows(values_only=True)]
    else:
        data = path.read_bytes()
        for enc in ("utf-8", "cp1250", "cp1252"):
            try:
                text = data.decode(enc)
                break
            except UnicodeDecodeError:
                continue
        raw = [l.split("\t") for l in text.splitlines()]
    head = [str(h or "").strip() for h in raw[0]]
    out = []
    for r in raw[1:]:
        if not r or num(r[0]) is None or not float(num(r[0])).is_integer():
            continue  # units row, blank and summary rows
        out.append({head[i]: r[i] for i in range(min(len(head), len(r)))})
    return out


def main(argv: list[str]) -> int:
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
    recs = sorted(rows_of(files[0]), key=lambda r: num(r["Record"]))
    cells = []
    for k, r in enumerate(recs):
        cells.append([k, "record", num(r["Record"])])
        name = r.get("Sample Name")
        if name:
            cells.append([k, "sample_name", str(name).strip()])
        when = date_of(str(r.get("Measurement Date and Time", "")))
        if when:
            cells.append([k, "measured_at", when])
        taken = set()
        for col, v in r.items():
            key = col.lower()
            ours = next((o for p, o in COLUMNS if key == p or (len(p) > 2 and key.startswith(p))), None)
            x = num(v)
            if ours is None or ours in taken or x is None:
                continue
            if ours.startswith("peak") and x == 0.0:
                continue  # the export's 0 for a peak that is not there
            taken.add(ours)
            cells.append([k, ours, x, half_unit(v)])
        pd_raw = next((v for c, v in r.items() if c.lower().startswith("%pd peak 1")), None)
        mean_raw = r.get("Pk 1 Mean Int (d.nm)")
        pd, mean = num(pd_raw), num(mean_raw)
        if pd and mean:
            w = pd * mean / 100.0
            tol = w * (half_unit(pd_raw) / pd + half_unit(mean_raw) / mean) * 1.01
            cells.append([k, "peak1_width", w, tol])
    src = f"Zetasizer export {files[0].name}"
    out = {"id": ident, "format": "malvern-zetasizer-dts", "independent": True,
           "reader": "the Zetasizer software's export of the same records (vendor software)",
           "tables": [{"table": 0, "rows": len(recs), "cells": cells, "tol_rel": 0.005, "tol_abs": 0.0006,
                       "time_tol_s": 1.0, "source": src}]}
    out_dir.mkdir(parents=True, exist_ok=True)
    dst = out_dir / f"{ident}.json"
    dst.write_text(json.dumps(out, indent=1, ensure_ascii=False) + "\n")
    print(dst, len(recs), "records", len(cells), "cells")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
