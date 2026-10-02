"""Flow cytometry (FCS): the TEXT segment read by fcsparser (MIT; the primary oracle is FlowIO),
mapped to OpenReadout's normalized fields, and compensated events from FlowKit (BSD-3), whose
compensation (inverse of the file's spillover matrix) is independent of ours.

fcsparser reads the first data set only, so every check is about data set 0 (`tables[0]`).
"""
from __future__ import annotations

import re
from datetime import datetime
from importlib.metadata import version
from pathlib import Path

from . import Rec

FAMILY = "flow-cytometry"
FORMATS = {"fcs"}
T = "/tables/0"


def fcs_date(date: str | None, clock: str | None) -> str | None:
    """$DATE (dd-mmm-yyyy per the standard; other orders in practice) + $BTIM/$ETIM
    (hh:mm:ss[.cc] or hh:mm:ss:tt with 1/60 s ticks in FCS 2.0/3.0) as ISO 8601 without zone."""
    if not date:
        return None
    date = date.strip()
    day = None
    for f in ("%d-%b-%Y", "%d-%b-%y", "%Y-%m-%d", "%d.%m.%Y", "%m/%d/%Y", "%Y/%m/%d", "%d-%B-%Y", "%Y-%b-%d"):
        try:
            day = datetime.strptime(date.title() if "b" in f.lower() else date, f)
            break
        except ValueError:
            continue
    if day is None:
        return None
    iso = day.strftime("%Y-%m-%d")
    if not clock or not clock.strip():
        return iso
    c = clock.strip()
    m = re.fullmatch(r"(\d{1,2}):(\d{2})(?::(\d{2}))?(?:([.:])(\d+))?", c)
    if not m:
        return iso
    h, mi, s, sep, frac = m.groups()
    sec = float(s or 0)
    if frac and sep == ".":
        sec += float("0." + frac)          # FCS 3.1: hundredths after a point
    elif frac and len(frac) <= 2 and int(frac) < 60:
        sec += int(frac) / 60               # FCS 2.0/3.0: sixtieths after a fourth colon
    # (a fourth field of three digits or >= 60 is not a sixtieth: left out)
    return f"{iso}T{int(h):02d}:{int(mi):02d}:{sec:06.3f}"


def spill(meta: dict):
    for k in ("$SPILLOVER", "SPILL", "$SPILL", "SPILLOVER", "$COMP"):
        v = meta.get(k)
        if not v or not isinstance(v, str):
            continue
        parts = [p.strip() for p in v.split(",")]
        try:
            n = int(float(parts[0]))
        except ValueError:
            continue
        if n <= 0 or len(parts) < 1 + n + n * n:
            continue
        names = parts[1:1 + n]
        vals = [float(x) for x in parts[1 + n:1 + n + n * n]]
        return k, names, [vals[i * n:(i + 1) * n] for i in range(n)]
    return None


def run(rec: Rec, entry: dict, path: Path, ctx) -> None:
    import fcsparser
    r = rec.reader(f"fcsparser {version('fcsparser')} (MIT), TEXT segment of data set 0")
    try:
        meta = fcsparser.parse(str(path), meta_data_only=True, reformat_meta=True)
    except Exception as e:
        raise RuntimeError(f"fcsparser: {e}")
    ch = meta["_channels_"]
    n = int(meta.get("$PAR") or len(ch))
    idx = list(ch.index)
    names = [str(ch.loc[i, "$PnN"]).strip() for i in idx]
    rec.check("event count ($TOT)", f"{T}/row_count", int(meta["$TOT"]), reader=r, required=True)
    rec.check("parameter count ($PAR)", f"{T}/columns#len", n, reader=r)
    rec.check("parameter names ($PnN)", f"{T}/columns/*/name", names, reader=r)
    kw = dict(by="/name", reader=r)
    col = f"{T}/columns"
    labels = {}
    for i, nm in zip(idx, names):
        s = meta.get(f"$P{i}S")
        if isinstance(s, str) and s.strip():
            labels[nm] = s.strip()
    rec.check("parameter labels ($PnS)", col, labels, each="/label", cmp="text", **kw)
    rng, bits, gain, volt, dec, off = {}, {}, {}, {}, {}, {}
    for i, nm in zip(idx, names):
        try:
            rng[nm] = float(str(ch.loc[i, "$PnR"]).strip())
        except (ValueError, KeyError):
            pass
        b = ch.loc[i, "$PnB"] if "$PnB" in ch.columns else None
        if b is not None and str(b).strip() not in ("*", ""):
            try:
                bits[nm] = int(b)
            except ValueError:
                pass
        g = meta.get(f"$P{i}G")
        if g not in (None, ""):
            try:
                gain[nm] = float(g)
            except ValueError:
                pass
        v = meta.get(f"$P{i}V")
        if v not in (None, ""):
            try:
                volt[nm] = float(v)
            except ValueError:
                pass
        e = meta.get(f"$P{i}E")
        if isinstance(e, str) and "," in e:
            try:
                a, b2 = (float(x) for x in e.split(",")[:2])
                dec[nm], off[nm] = a, b2
            except ValueError:
                pass
    rec.check("parameter range ($PnR)", col, rng, each="/extra/range_keyword", cmp="num", rel=1e-12, **kw)
    rec.check("parameter bits ($PnB)", col, bits, each="/extra/bits", **kw)
    rec.check("parameter gain ($PnG)", col, gain, each="/extra/gain", cmp="num", rel=1e-12, **kw)
    rec.check("detector voltage ($PnV)", col, volt, each="/extra/detector_voltage", cmp="num", rel=1e-12, **kw)
    rec.check("amplification decades ($PnE)", col, dec, each="/extra/amplification/decades", cmp="num", **kw)
    rec.check("amplification offset ($PnE)", col, off, each="/extra/amplification/offset", cmp="num", rel=1e-12, **kw)
    ex = f"{T}/extra"
    rec.check("data type ($DATATYPE)", f"{ex}/datatype", meta.get("$DATATYPE"), cmp="text", reader=r)
    rec.check("mode ($MODE)", f"{ex}/mode", meta.get("$MODE"), cmp="text", reader=r)
    bo = str(meta.get("$BYTEORD", "")).replace(" ", "")
    order = {"1,2,3,4": "little-endian", "1,2": "little-endian", "4,3,2,1": "big-endian", "2,1": "big-endian"}.get(bo)
    if meta.get("$DATATYPE") != "A":
        rec.check("byte order ($BYTEORD)", f"{ex}/byte_order", order, cmp="text", reader=r)
    rec.check("FCS version", f"{ex}/fcs_version", meta["__header__"]["FCS format"], cmp="text", reader=r)
    ts = meta.get("$TIMESTEP")
    if ts:
        try:
            rec.check("time step ($TIMESTEP)", f"{ex}/timestep_s", float(ts), cmp="num", rel=1e-12, reader=r)
        except ValueError:
            pass
    rec.check("file name ($FIL)", f"{ex}/file_name", meta.get("$FIL"), cmp="text", reader=r)
    rec.check("source ($SRC)", f"{ex}/source", meta.get("$SRC"), cmp="text", reader=r)
    rec.check("system ($SYS)", f"{ex}/system", meta.get("$SYS"), cmp="text", reader=r)
    rec.check("operator ($OP)", "/experiment/acquisition/operator", meta.get("$OP"), cmp="text", reader=r)
    rec.check("cytometer ($CYT)", "/experiment/instrument/model", meta.get("$CYT"), cmp="text", reader=r)
    rec.check("cytometer serial ($CYTSN)", "/experiment/instrument/serial", meta.get("$CYTSN"), cmp="text", reader=r)
    # FCS 3.2: ISO 8601 $BEGINDATETIME/$ENDDATETIME (with a zone) supersede $DATE/$BTIM/$ETIM
    for key, field, where in (("$BEGINDATETIME", "acquisition start ($BEGINDATETIME)", "started_at"),
                             ("$ENDDATETIME", "acquisition end ($ENDDATETIME)", "ended_at")):
        v = meta.get(key)
        if isinstance(v, str) and v.strip():
            rec.check(field, f"/experiment/acquisition/{where}", v.strip(), cmp="time", abs=0.02, reader=r)
    if meta.get("$BEGINDATETIME"):
        return tail(rec, meta, path, r, ex)
    start = fcs_date(meta.get("$DATE"), meta.get("$BTIM"))
    if start and "T" in start:
        rec.check("acquisition start ($DATE $BTIM)", "/experiment/acquisition/started_at", start, cmp="time", abs=0.02, reader=r)
    end = fcs_date(meta.get("$DATE"), meta.get("$ETIM"))
    if end and start and "T" in end and "T" in start and end < start:  # past midnight
        from datetime import timedelta
        d0 = datetime.strptime(end[:10], "%Y-%m-%d") + timedelta(days=1)
        end = d0.strftime("%Y-%m-%d") + end[10:]
    if end and "T" in end:
        rec.check("acquisition end ($DATE $ETIM)", "/experiment/acquisition/ended_at", end, cmp="time", abs=0.02, reader=r)
    tail(rec, meta, path, r, ex)


def tail(rec, meta, path, r, ex):
    sp = spill(meta)
    if sp:
        key, pnames, matrix = sp
        rec.check(f"spillover parameters ({key})", f"{ex}/spillover/parameters", pnames, reader=r)
        rec.check(f"spillover matrix ({key})", f"{ex}/spillover/matrix", matrix, cmp="num", rel=1e-12, reader=r)
        try:
            compensated(rec, path, pnames, matrix)
        except Exception as e:  # FlowKit refuses some files; the TEXT checks stand
            rec.note(f"FlowKit could not compensate: {type(e).__name__}: {e}")


def compensated(rec: Rec, path: Path, pnames: list[str], matrix: list[list[float]]) -> None:
    """The first 16 events compensated by FlowKit with the file's own spillover matrix, against
    `openreadout table --compensate` (both invert the matrix and apply it to the raw events)."""
    import flowkit as fk
    import numpy as np
    import pandas as pd
    try:
        s = fk.Sample(str(path), ignore_offset_error=True, ignore_offset_discrepancy=True)
    except TypeError:
        s = fk.Sample(str(path))
    comp = fk.Matrix(np.array(matrix), pnames) if hasattr(fk, "Matrix") else None
    if comp is None:
        return
    s.apply_compensation(comp)
    ev = s.get_events(source="comp")
    labels = list(s.pnn_labels)
    rows = min(16, ev.shape[0])
    r = rec.reader(f"FlowKit {version('flowkit')} (BSD-3) compensation with the file's spillover matrix")
    cmd = ("table", "--compensate", "fcs", "--max-rows", str(rows))
    for j, name in enumerate(labels):
        if name not in pnames:
            continue
        rec.check(f"compensated events ({name})", f"/rows/*/{j}", [float(x) for x in ev[:rows, j]],
                  cmd=cmd, cmp="num", rel=1e-6, abs=1e-6, scope="tables", reader=r)
