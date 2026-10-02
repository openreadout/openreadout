"""Plate readers: an independent grid scanner beside the primary oracles (allotropy's converters
and oracle/plate.py's vendor readers).

The scanner knows no vendor layout. It reads the export's cells (text in any of the usual
encodings and delimiters, or the first sheets of an .xlsx/.xls) and finds every plate grid: a
row of consecutive column numbers 1..N (N = 12, 24 or 48) followed by rows lettered A, B, C,
... whose next N cells are numbers. Each numeric grid is one read's values by well. The first
grid is set against OpenReadout's first read (`table` 0, read 1), well by well, and the number of
numeric grids against the reads OpenReadout reports for that plate. It checks the one thing a
vendor-specific reader can silently get wrong in a plate export: which value belongs to which
well (row/column orientation, off-by-one, a label column taken for data).

Kinetic exports (wells as columns, one row per time) and binary documents have no grid and get
no checks here.

The run's start: allotropy's ASM `measurement time` (its converter's reading of the export header)
against the normalized `experiment.acquisition.started_at`. allotropy marks a clock time without a
zone `+00:00`; the wall clock is compared. SoftMax Pro text exports state no read time, only the
footer's `Date Last Saved`, which allotropy turns into its measurement time: there it is set
against `experiment.acquisition.saved_at`, beside the footer itself read as text.
"""
from __future__ import annotations

import csv
import io
import re
from pathlib import Path

from . import Rec

FAMILY = "plate-reader"
FORMATS = {"plate"}
ROWS = "ABCDEFGHIJKLMNOPQRSTUVWXYZ"


def cells(path: Path) -> list[list[str]]:
    suf = path.suffix.lower()
    if suf in (".xlsx", ".xls", ".xlsm"):
        from python_calamine import CalamineWorkbook
        wb = CalamineWorkbook.from_path(str(path))
        out = []
        for name in wb.sheet_names:
            for row in wb.get_sheet_by_name(name).to_python():
                out.append(["" if c is None else (repr(c) if isinstance(c, float) else str(c)) for c in row])
            out.append([])
        return out
    if suf not in (".txt", ".csv", ".tsv", ".asc"):
        return []
    raw = path.read_bytes()
    text = None
    if raw[:2] in (b"\xff\xfe", b"\xfe\xff"):
        text = raw.decode("utf-16")
    else:
        for enc in ("utf-8-sig", "cp1252", "latin-1"):
            try:
                text = raw.decode(enc)
                break
            except UnicodeDecodeError:
                continue
    lines = text.splitlines()
    delim = "\t" if sum(l.count("\t") for l in lines) >= sum(l.count(",") for l in lines) / 2 else ","
    if delim == "," and sum(l.count(";") for l in lines) > sum(l.count(",") for l in lines):
        delim = ";"
    return [row for row in csv.reader(lines, delimiter=delim)]


def number(s: str):
    s = s.strip().replace("−", "-")
    if not s:
        return None
    try:
        return float(s)
    except ValueError:
        return None


def grids(rows: list[list[str]]) -> list[dict]:
    """Every A..-lettered numeric grid under a 1..N header row: {'values': {(r, c): v}, 'n': N}."""
    out = []
    i = 0
    while i < len(rows):
        row = [c.strip() for c in rows[i]]
        found = None
        for start in range(len(row)):
            n = 0
            while start + n < len(row) and row[start + n].isdigit() and int(row[start + n]) == n + 1:
                n += 1
            if n in (12, 24, 48):
                found = (start, n)
                break
        if not found:
            i += 1
            continue
        start, n = found
        if start >= 1 and row[start - 1].lower().startswith("temperature"):
            # SoftMax Pro: no row letters; the rows follow in order A, B, ... (first cell: the
            # temperature on the first row, empty after it)
            g = softmax_grid(rows, i, start, n)
            if g:
                out.append(g)
                i += g["rows"] + 1
                continue
        # reads[j][(row, col)]: a lettered row holds read 1; unlabelled rows right after it
        # (Gen5 writes several reads interleaved this way) hold reads 2, 3, ...
        reads: list[dict] = []
        numeric = True
        k = i + 1
        r = 0
        while k < len(rows) and r < 32:
            line = [c.strip() for c in rows[k]]
            if start < 1 or len(line) < start or line[start - 1] != ROWS[r]:
                break
            j = 0
            while True:
                vals = [number(c) for c in line[start:start + n]]
                if j == 0 and all(v is None for v in vals):
                    numeric = False
                if j >= len(reads):
                    reads.append({})
                for c, v in enumerate(vals):
                    if v is not None:
                        reads[j][(r, c)] = v
                k += 1
                j += 1
                if k >= len(rows):
                    break
                nxt = [c.strip() for c in rows[k]]
                if len(nxt) < start + 1 or nxt[start - 1] != "" or number(nxt[start]) is None:
                    break
                line = nxt
            r += 1
        if r >= 8 and numeric and reads and reads[0]:
            out.append({"reads": reads, "n": n, "rows": r})
        i = k if r else i + 1
    return out


def softmax_grid(rows, i, start, n):
    values = {}
    r = 0
    for k in range(i + 1, min(i + 33, len(rows))):
        line = [c.strip() for c in rows[k]]
        vals = [number(c.replace(",", ".")) if c.count(",") == 1 and "." not in c else number(c)
                for c in line[start:start + n]]
        if len(vals) < n or all(v is None for v in vals):
            break
        for c, v in enumerate(vals):
            if v is not None:
                values[(r, c)] = v
        r += 1
    if r in (8, 16, 32) and values:
        return {"reads": [values], "n": n, "rows": r}
    return None


WELL = re.compile(r"^([A-Z])(\d{1,2})$")


def well_list(rows) -> dict | None:
    """(value, well) or (well, value) pairs in adjacent cells, as Magellan's list sheets write."""
    values = {}
    for row in rows:
        c = [x.strip() for x in row]
        for a, b in zip(c, c[1:]):
            m, v = WELL.match(b), number(a)
            if not m:
                m, v = WELL.match(a), number(b)
            if m and v is not None:
                key = (ROWS.index(m.group(1)), int(m.group(2)) - 1)
                values.setdefault(key, v)
                break
    if len(values) < 2:
        return None
    ncol = 12 if max(c for _, c in values) < 12 and max(r for r, _ in values) < 8 else 24
    return {"reads": [values], "n": ncol, "rows": None}


def asm_first(v, key: str):
    if isinstance(v, dict):
        for k, x in v.items():
            if k == key and isinstance(x, str):
                return x
            r = asm_first(x, key)
            if r:
                return r
    elif isinstance(v, list):
        for x in v:
            r = asm_first(x, key)
            if r:
                return r
    return None


def asm_time(rec: Rec, entry: dict, path: Path) -> None:
    import sys
    import warnings
    from importlib.metadata import version
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
    import plate as oracle_plate  # the primary oracle's vendor mapping
    vendor = oracle_plate.vendor_for(entry["id"])
    if not vendor or vendor in ("BMG_CSV", "ENVISION_CSV"):  # no allotropy parser for these
        return
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        try:
            asm = oracle_plate.allotropy_asm(path, vendor)
        except Exception as e:  # allotropy refuses the export: no second opinion on the time
            rec.note(f"allotropy: {type(e).__name__}")
            return
    t = asm_first(asm, "measurement time")
    saved_only = vendor == "MOLDEV_SOFTMAX_PRO" and path.suffix.lower() == ".txt"
    if t:
        r = rec.reader(f"allotropy {version('allotropy')} (MIT) ASM `measurement time`")
        if saved_only:
            rec.check("ASM measurement time (wall clock) against the save time", "/experiment/acquisition/saved_at",
                      t, cmp="wallclock", abs=1.0, reader=r)
        else:
            rec.check("ASM measurement time (wall clock)", "/experiment/acquisition/started_at", t, cmp="wallclock",
                      abs=1.0, reader=r)
    if saved_only:
        softmax_saved(rec, path)


def softmax_saved(rec: Rec, path: Path) -> None:
    """The SoftMax Pro text export's last line, `Original Filename: ...; Date Last Saved: M/D/YYYY
    h:mm:ss AM`, read as text (month first, as the US-locale exports in the corpus write it)."""
    from datetime import datetime
    raw = path.read_bytes()
    text = raw.decode("utf-16") if raw[:2] in (b"\xff\xfe", b"\xfe\xff") else raw.decode("latin-1")
    m = re.search(r"Date Last Saved:\s*(\d{1,2}/\d{1,2}/\d{4}\s+\d{1,2}:\d{2}:\d{2}\s*[AP]M)", text)
    if not m:
        return
    t = datetime.strptime(" ".join(m.group(1).split()), "%m/%d/%Y %I:%M:%S %p")
    r = rec.reader("the SoftMax Pro text export's footer (Date Last Saved), read as text")
    rec.check("Date Last Saved (footer)", "/experiment/acquisition/saved_at", t.isoformat(), cmp="wallclock",
              abs=1.0, reader=r)


def run(rec: Rec, entry: dict, path: Path, ctx) -> None:
    asm_time(rec, entry, path)
    try:
        rows = cells(path)
    except Exception as e:  # an export this scanner cannot open: nothing to say
        rec.note(f"could not read the cells: {e}")
        return
    text = " ".join(" ".join(r) for r in rows[:400]).lower()
    if any(w in text for w in ("kinetic", "spectrum", "spectral scan", "cycle nr")):
        return  # kinetic and spectral exports: their grids are derived results, not read 1
    gs = grids(rows)
    if not gs:
        wl = well_list(rows)
        gs = [wl] if wl else []
    if not gs:
        return
    r = rec.reader("an independent plate-grid scanner (oracle/second_fields/plate.py; no vendor layout)")
    g = gs[0]
    ncol = g["n"]
    cmd = ("table", "--table", "0", "--max-rows", "1000000")
    # our rows are [well, row, col, read, wavelength_nm, time_s, value]
    for j, read in enumerate(g["reads"][:4]):
        wells = {str(row * ncol + col): v for (row, col), v in sorted(read.items())}
        rec.check(f"first grid: values by well (read {j + 1})", f"/rows/{{3={j + 1}}}", wells, cmd=cmd, by="/0",
                  each="/6", cmp="num", rel=1e-9, abs=1e-12, scope="tables", reader=r)

