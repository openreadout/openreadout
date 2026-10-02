"""Chromatography: chromConverter (Ethan Bass, GPL-3.0; R, run as a black box with its own
parsers) on the detector files, beside the primary oracles (rainbow-api and Aston for
ChemStation, the LabSolutions exports and msconvert for Shimadzu, Appia for Empower ARW).

- Agilent ChemStation `.ch` (FID, TCD, VWD, MWD, ...) and `.uv` (DAD): each signal's values
  (first samples, whole-trace statistics, point count, first and last time) and the file header
  (sample name, operator, method, signal description, unit, acquisition time, file version).
- Shimadzu LabSolutions `.lcd`/`.gcd`: each chromatogram channel's values and the run metadata.
- Waters Empower `.arw` exports: the trace and its header fields.

`CHROMCONVERTER_LIB` must name an R library holding chromConverter 0.9
(install.packages("chromConverter", lib = ...)); without it this family writes nothing.
"""
from __future__ import annotations

import csv
import json
import os
import re
import subprocess
import tempfile
from datetime import datetime, timezone
from pathlib import Path

import numpy as np

from . import Rec, trace_values

FAMILY = "chromatography"
FORMATS = {"chemstation", "shimadzu", "empower-arw", "andi-chrom"}
HERE = Path(__file__).resolve().parent


def cc(fmt: str, path: Path) -> list[tuple[dict, dict]]:
    """chromConverter's traces: [(metadata, {column: list})]."""
    lib = os.environ.get("CHROMCONVERTER_LIB")
    if not lib:
        raise RuntimeError("set CHROMCONVERTER_LIB to an R library with chromConverter")
    with tempfile.TemporaryDirectory() as out:
        r = subprocess.run(["Rscript", str(HERE / "chromconverter.R"), fmt, str(path), out],
                           env=dict(os.environ, CHROMCONVERTER_LIB=lib), capture_output=True, text=True,
                           timeout=1800)
        if r.returncode != 0:
            raise RuntimeError(f"chromConverter: {r.stderr.strip()[-300:]}")
        items = []
        for i in range(1, 1000):
            m, t = Path(out) / f"meta_{i}.json", Path(out) / f"trace_{i}.csv"
            if not t.exists():
                break
            meta = json.loads(dec(m.read_bytes())) if m.exists() else {}
            rows = list(csv.reader(dec(t.read_bytes()).splitlines()))
            cols = {h: [row[k] for row in rows[1:]] for k, h in enumerate(rows[0])}
            items.append((meta, cols))
        version = r.stdout.strip().split()[-1] if r.stdout.strip() else "?"
    return items, version


def dec(b: bytes) -> str:
    try:
        return b.decode("utf-8")
    except UnicodeDecodeError:
        return b.decode("latin-1")


def num(v):
    try:
        x = float(v)
    except (TypeError, ValueError):
        return float("nan")
    return x


def epoch_wallclock(v) -> str | None:
    """chromConverter reports ChemStation times as seconds since 1970 of the local clock."""
    if isinstance(v, (int, float)) and v > 0:
        return datetime.fromtimestamp(v, timezone.utc).replace(tzinfo=None).isoformat()
    if isinstance(v, str) and re.match(r"\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}", v):
        return v.replace(" ", "T")
    return None


def chemstation(rec: Rec, path: Path) -> None:
    files = [path] if path.is_file() else sorted(
        (p for p in path.iterdir() if p.suffix.lower() in (".ch", ".uv")), key=lambda p: p.name.lower())
    for f in files:
        if f.suffix.lower() == ".ms":
            continue
        fmt = "chemstation_uv" if f.suffix.lower() == ".uv" else "chemstation_ch"
        try:
            items, ver = cc(fmt, f)
        except RuntimeError as e:
            rec.note(f"{f.name}: {e}")
            continue
        if not items:
            continue
        r = rec.reader(f"chromConverter {ver} (GPL-3.0, R, run as a black box with its own parsers)")
        meta, cols = items[0]
        sel = f"/traces/[extra.file~={f.name}]"
        ex = f"{sel}/extra"
        label = f.name
        text = dict(cmp="text", reader=r)
        rec.check(f"{label}: sample name", f"{ex}/sample_name", meta.get("sample_name"), **text)
        rec.check(f"{label}: operator", f"{ex}/operator", meta.get("operator"), **text)
        rec.check(f"{label}: method", f"{ex}/method", meta.get("method"), **text)
        rec.check(f"{label}: file version", f"{ex}/format_version", meta.get("file_version"), **text)
        rec.check(f"{label}: file type", f"{ex}/file_type", meta.get("file_type"), **text)
        t = epoch_wallclock(meta.get("run_datetime"))
        if t:
            rec.check(f"{label}: acquisition time (local clock)", f"{ex}/acquired_at", t, cmp="wallclock", reader=r)
            if f is files[0]:
                # the run's start as the normalized field (the first signal file's header)
                rec.check("acquisition start (first signal file's header)", "/experiment/acquisition/started_at", t,
                          cmp="time_or_utc", abs=1.0, reader=r)
        # `GCI` is the same generic module tag in every version-179/181 file, GC and LC
        # (docs/provenance/chemstation.md): not a model, so not compared
        if f is files[0] and meta.get("detector_id") and meta["detector_id"].strip() != "GCI":
            rec.check("instrument model (the detector module's id)", "/experiment/instrument/model",
                      meta["detector_id"], cmp="text", reader=r)
        trace = f"@{sel}/index"
        if fmt == "chemstation_ch":
            rec.check(f"{label}: signal", f"{ex}/signal", meta.get("detector_range"), **text)
            rec.check(f"{label}: unit", f"{sel}/channels/0/unit", meta.get("detector_y_unit"), **text)
            rt = np.array([num(x) for x in cols["rt"]])
            y = np.array([num(x) for x in cols["intensity"]])
            rec.check(f"{label}: first time (min)", f"{ex}/axis/first", float(rt[0]), cmp="num", rel=1e-9, reader=r)
            rec.check(f"{label}: last time (min)", f"{ex}/x_end_min", float(rt[-1]), cmp="num", rel=1e-9, reader=r)
            trace_values(rec, trace, 0, 0, y, r, rel=1e-9, field_prefix=f"{label}: signal")
        else:
            rt = np.array([num(x) for x in cols["rt"]])
            lam = np.array([num(x) for x in cols["lambda"]])
            y = np.array([num(x) for x in cols["intensity"]])
            wl = np.unique(lam[np.isfinite(lam)])
            rec.check(f"{label}: wavelengths", f"{sel}/channels#len", int(wl.size), reader=r)
            rec.check(f"{label}: unit", f"{sel}/channels/0/unit", meta.get("detector_y_unit"), **text)
            for k in sorted({0, wl.size // 2, wl.size - 1}):
                w = wl[k]
                v = y[lam == w]
                rec.check(f"{label}: channel {k} wavelength", f"{sel}/channels/{k}/extra/wavelength_nm", float(w),
                          cmp="num", rel=1e-9, reader=r)
                # (chromConverter writes these values to seven significant digits)
                trace_values(rec, trace, 0, k, v, r, rel=1e-6, field_prefix=f"{label}: {w:g} nm")


def shimadzu(rec: Rec, path: Path) -> None:
    fmt = {".lcd": "shimadzu_lcd", ".gcd": "shimadzu_gcd"}.get(path.suffix.lower())
    if fmt is None:
        return
    items, ver = cc(fmt, path)
    if not items:
        return
    r = rec.reader(f"chromConverter {ver} (GPL-3.0, R, run as a black box with its own parsers)")
    meta, cols = items[0]
    text = dict(cmp="text", reader=r)
    rec.check("operator", "/experiment/acquisition/operator", meta.get("operator"), **text)
    rec.check("sample name", "/experiment/sample/name", meta.get("sample_name"), **text)
    rec.check("sample id", "/experiment/sample/id", meta.get("sample_id"), **text)
    mth = meta.get("method")
    if isinstance(mth, str) and mth:
        rec.check("method", "/experiment/method/name", re.split(r"[\\/]", mth)[-1].rsplit(".", 1)[0], **text)
    t = epoch_wallclock(meta.get("run_datetime"))
    if t:
        rec.check("acquisition time", "/experiment/acquisition/started_at", t, cmp="time_mod_zone", abs=1.0, reader=r)
    rt = np.array([num(x) for x in cols["rt"]])
    y = np.array([num(x) for x in cols["intensity"]])
    det = cols.get("detector") or ["Chromatogram Ch1"] * len(rt)
    for name in dict.fromkeys(det):
        m = np.array([d == name for d in det])
        if not m.any():
            continue
        sel = f"/traces/[extra.stream$={name}]" if name.startswith("Chromatogram") else f"/traces/[name~={name}]"
        rec.check(f"{name}: points", f"{sel}/sample_count", int(m.sum()), reader=r)
        rec.check(f"{name}: first time (min)", f"{sel}/extra/axis/first", float(rt[m][0]), cmp="num", abs=1e-9,
                  reader=r)
        trace_values(rec, f"@{sel}/index", 0, 0, y[m], r, rel=1e-9, count=False, field_prefix=f"{name}")


def arw(rec: Rec, path: Path) -> None:
    items, ver = cc("waters_arw", path)
    if not items:
        return
    r = rec.reader(f"chromConverter {ver} (GPL-3.0, R, run as a black box with its own parsers)")
    meta, cols = items[0]
    ex = "/traces/0/extra"
    text = dict(cmp="text", reader=r)
    rec.check("sample name", f"{ex}/sample_name", meta.get("sample_name"), **text)
    rec.check("instrument method", f"{ex}/instrument_method", meta.get("method"), **text)
    rec.check("sample set", f"{ex}/sample_set", meta.get("batch"), **text)
    rec.check("channel", f"{ex}/channel", meta.get("detector_range"), **text)
    rt = np.array([num(x) for x in cols["rt"]])
    y = np.array([num(x) for x in cols["intensity"]])
    rec.check("points", "/traces/0/sample_count", int(rt.size), reader=r)
    rec.check("first time (min)", f"{ex}/x_start_min", float(rt[0]), cmp="num", abs=1e-9, reader=r)
    rec.check("last time (min)", f"{ex}/x_end_min", float(rt[-1]), cmp="num", rel=1e-9, reader=r)
    trace_values(rec, 0, 0, 0, y, r, rel=1e-9)


def andi_stamp(v: str | None) -> str | None:
    """ANDI (ASTM E1947) date-time stamp `YYYYMMDDhhmmss±hhmm` → ISO 8601 with the offset."""
    if not v:
        return None
    s = v.replace(" ", "")
    m = re.fullmatch(r"(\d{4})(\d{2})(\d{2})(\d{2})(\d{2})(\d{2})([+-]\d{4})?", s)
    if not m or m.group(1) == "0000":
        return None
    y, mo, d, h, mi, se, z = m.groups()
    iso = f"{y}-{mo}-{d}T{h}:{mi}:{se}"
    return iso + (f"{z[:3]}:{z[3:]}" if z else "")


def andi(rec: Rec, path: Path) -> None:
    from importlib.metadata import version
    from scipy.io import netcdf_file
    n = netcdf_file(str(path), "r", mmap=False)
    a = {k: (v.decode("latin-1") if isinstance(v, bytes) else v) for k, v in n._attributes.items()}
    n.close()
    r = rec.reader(f"scipy {version('scipy')} netcdf_file: the file's global attributes (ANDI, ASTM E1947)")
    t = andi_stamp(a.get("injection_date_time_stamp")) or andi_stamp(a.get("experiment_date_time_stamp"))
    if t:
        rec.check("injection_date_time_stamp", "/experiment/acquisition/started_at", t, cmp="time", zone=True, reader=r)
    op = a.get("operator_name")
    if isinstance(op, str) and op.strip():
        rec.check("operator_name", "/experiment/acquisition/operator", op.strip(), cmp="text", reader=r)
    for key in ("instrument_model", "instrument_name"):
        v = a.get(key)
        if isinstance(v, str) and v.strip():
            rec.check(key, "/experiment/instrument/model", v.strip(), cmp="text", reader=r)
            break


def run(rec: Rec, entry: dict, path: Path, ctx) -> None:
    fmt = entry["format"]
    if fmt == "andi-chrom":
        return andi(rec, path)
    if fmt == "chemstation":
        if path.suffix.lower() == ".ms":
            return
        chemstation(rec, path)
    elif fmt == "shimadzu":
        shimadzu(rec, path)
    elif fmt == "empower-arw":
        arw(rec, path)
