#!/usr/bin/env python
"""Ground truth for Cytiva ÄKTA / UNICORN results (`cytiva-unicorn-res`, `cytiva-unicorn-zip`).

Usage:  uv run --group fplc python fplc_oracle.py [--out DIR] --id ID FILE [--id ID FILE ...]

Writes `<DIR>/<ID>.json` (default `corpus/oracle/fplc/`) with, per curve, the sample count and
values at up to 128 evenly spaced samples, the sum, maximum and position of the maximum, as the
black-box readers return them:

- `.res` (UNICORN 3-5): PyCORN 0.19 (GPL-2.0, run as a black box: `pc_res3`); PyCORN measures
  volumes from the last injection (as UNICORN displays them), so its volumes are compared as
  differences and its zero with the file's injection mark. Fraction marks (volume, label) too.
- `.zip` (UNICORN 6/7 export): PyCORN 0.19 (`pc_uni6`, black box) for the raw float arrays of
  every curve member, and allotropy 0.1.146 (MIT) for its ASM data cubes (a second opinion on
  the values of the curves it converts). Both read the floats from byte 47 of a member to 48
  bytes before its end, so their arrays start at our sample 5 (`shift`) and stop 11 samples
  before our last; the test compares the overlap. Curve names and point files, event marks and
  UNICORN's own peak table come from `Chrom.1.Xml` parsed with the standard library's
  ElementTree (the vendor's values, used to check our curves against UNICORN's integration).
  Curve members that are not whole .NET float arrays (cut short, or bare floats in a synthetic
  file) are listed as `not_arrays`: both readers return misaligned values for them, and
  openreadout must refuse them.

The corpus test `crates/openreadout-corpus-tests/tests/fplc_oracle/mod.rs` compares them.
"""
from __future__ import annotations

import io
import json
import sys
import zipfile
import xml.etree.ElementTree as ET
from pathlib import Path

import numpy as np
sys.path.insert(0, str(Path(__file__).resolve().parent))
import oracle_json  # noqa: E402  (<id>.json, or <id>.json.gz over 1 MiB in corpus/oracle/heldout)

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "corpus" / "oracle" / "fplc"
SAMPLES = 128


def _curve(name, unit, vol, val, shift=0) -> dict:
    vol = np.asarray(vol, dtype=np.float64)
    val = np.asarray(val, dtype=np.float64)
    n = len(val)
    idx = sorted(set(np.linspace(0, n - 1, min(SAMPLES, n)).round().astype(int).tolist())) if n else []
    k = int(np.nanargmax(val)) if n else 0
    return {
        "name": name,
        "unit": unit,
        "n": n,
        "shift": shift,
        "index": idx,
        "volumes": [float(vol[i]) for i in idx],
        "values": [float(val[i]) for i in idx],
        "sum": float(np.nansum(val)),
        "max": float(val[k]) if n else None,
        "argmax": k,
        "volume_at_max": float(vol[k]) if n else None,
    }


def res(path: Path) -> dict:
    import pycorn
    from importlib.metadata import version

    f = pycorn.pc_res3(str(path))
    f.load()
    curves, fractions, injections = [], [], []
    for k in f.keys():
        v = f[k]
        if not isinstance(v, dict):
            continue
        if v.get("data_type") == "curve":
            d = v["data"]
            curves.append(_curve(k, v.get("unit", ""), [x for x, _ in d], [y for _, y in d]))
        elif k == "Fractions":
            fractions = [[float(x), str(t)] for x, t in v["data"]]
        elif k == "Inject":
            injections = [[float(x), str(t)] for x, t in v["data"]]
    return {
        "reader": f"PyCORN {version('pycorn')} pc_res3 (GPL-2.0, black box)",
        "volumes_from_injection": True,
        "curves": curves,
        "fractions": fractions,
        "injections": injections,
    }


def _fixzip(b: bytes) -> zipfile.ZipFile:
    e = b.rfind(b"PK\x05\x06")
    return zipfile.ZipFile(io.BytesIO(b[: e + 22] if e >= 0 else b))


def _logical(name: str) -> str:
    base = name.replace("\\", "/").rsplit("/", 1)[-1]
    if base.startswith("Copy of "):
        base = base[len("Copy of "):]
    base = base.lower()
    return base[:-4] if base.endswith(".zip") else base


def export(path: Path) -> dict:
    import pycorn
    from importlib.metadata import version

    z = _fixzip(path.read_bytes())
    by_logical = {_logical(n): n for n in z.namelist()}
    chrom = z.read(by_logical["chrom.1.xml"]).decode("utf-8-sig")
    root = ET.fromstring(chrom)
    # curve name → (point file, unit, curve number)
    meta = []
    for cv in root.find("Curves"):
        fn = None
        for cp in cv.find("CurvePoints"):
            if cp.findtext("IsFullResolution") == "true" or fn is None:
                fn = cp.findtext("BinaryCurvePointsFileName")
        meta.append((cv.findtext("Name"), fn, (cv.findtext("AmplitudeUnit") or "").strip(),
                     cv.findtext("CurveNumber")))
    # PyCORN's raw arrays by member (pc_uni6 needs the plain export layout)
    pyc = {}
    pyc_error = None
    try:
        f = pycorn.pc_uni6(str(path))
        f.load()
        for k in f.keys():
            v = f[k]
            if isinstance(v, dict) and "CoordinateData.Amplitudes" in v:
                pyc[_logical(k)] = (v["CoordinateData.Volumes"], v["CoordinateData.Amplitudes"])
    except Exception as e:  # a re-packed export (folders, 'Copy of' names) PyCORN cannot walk
        pyc_error = f"{type(e).__name__}: {e}"
    # Members that are not whole .NET float arrays (a publisher cut them short, or wrote bare
    # floats): PyCORN's and allotropy's fixed byte offsets turn them into misaligned values.
    # Recorded as `not_arrays`; openreadout must refuse those curves, not read them.
    not_arrays = []
    for name, fn, unit, num in meta:
        try:
            inner = _fixzip(z.read(by_logical[_logical(fn or "")]))
            b = inner.read("CoordinateData.Amplitudes")
            n = int.from_bytes(b[22:26], "little", signed=True) if len(b) >= 27 else -1
            whole = b[:1] == b"\x00" and b[17:18] == b"\x0f" and n >= 0 and len(b) == 28 + 4 * n
        except Exception:
            whole = False
        if not whole:
            not_arrays.append(name)
    curves = []
    for name, fn, unit, num in meta:
        got = pyc.get(_logical(fn or ""))
        if got is None or name in not_arrays:
            continue
        c = _curve(name, unit, got[0], got[1], shift=5)
        c["curve_number"] = num
        curves.append(c)
    # allotropy's data cubes (a second opinion on the values it converts)
    allo = {}
    allo_error = None
    try:
        from allotropy.parser_factory import Vendor
        from allotropy.to_allotrope import allotrope_from_file
        d = allotrope_from_file(str(path), Vendor.CYTIVA_UNICORN)
        for doc in d["liquid chromatography aggregate document"]["liquid chromatography document"]:
            for m in doc["measurement aggregate document"]["measurement document"]:
                cube = m.get("chromatogram data cube") or {}
                data = cube.get("data") or {}
                if cube.get("label") and data.get("measures") and cube["label"] not in not_arrays:
                    vals = data["measures"][0]
                    vols = (data.get("dimensions") or [[]])[0]
                    allo[cube["label"]] = {"n": len(vals), "sum": float(np.nansum(np.asarray(vals, dtype=float))),
                                           "first": [float(x) for x in vals[:5]],
                                           "volume_first": [float(x) for x in vols[:5]]}
    except Exception as e:
        allo_error = f"{type(e).__name__}: {e}"
    # events and the vendor peak table, from the XML (vendor values)
    events = {}
    ecs = root.find("EventCurves")
    for ec in (list(ecs) if ecs is not None else []):
        kind = ec.get("EventCurveType")
        evs = ec.find("Events")
        if kind == "Logbook":
            events["Logbook_count"] = len(evs) if evs is not None else 0
            continue
        events[kind] = [[float(e.findtext("EventTime") or "nan"), float(e.findtext("EventVolume") or "nan"),
                         e.findtext("EventText") or ""] for e in (evs if evs is not None else [])]
    peak_tables = []
    pts = root.find("PeakTables")
    for pt in (list(pts) if pts is not None else []):
        peaks = []
        pks = pt.find("Peaks")
        for pk in (list(pks) if pks is not None else []):
            g = lambda t: float(pk.findtext(t)) if pk.findtext(t) else None
            peaks.append({"retention": g("MaxPeakRetention"), "start": g("StartPeakRetention"),
                          "end": g("EndPeakRetention"), "height": g("Height"), "area": g("Area"),
                          "width": g("Width")})
        dc = pt.find("DataCurve")
        bl = pt.find("BaseLine")
        peak_tables.append({"name": pt.findtext("Name"),
                            "curve_number": dc.findtext("CurveNumber") if dc is not None else None,
                            "baseline_curve_number": bl.findtext("CurveNumber") if bl is not None else None,
                            "basis": pt.findtext("CalculationRetention"),
                            "zero_at_injection": pt.findtext("ZeroAdjustedToInjectionNumber"),
                            "peaks": peaks})
    out = {
        "reader": f"PyCORN {version('pycorn')} pc_uni6 (GPL-2.0, black box); allotropy {version('allotropy')} (MIT)",
        "curves": curves,
        "allotropy": allo,
        "not_arrays": not_arrays,
        "events": events,
        "peak_tables": peak_tables,
    }
    if pyc_error:
        out["pycorn_error"] = pyc_error
    if allo_error:
        out["allotropy_error"] = allo_error
    return out


def _clean(x):
    """NaN and infinities as null (JSON has none)."""
    if isinstance(x, float):
        return x if np.isfinite(x) else None
    if isinstance(x, dict):
        return {k: _clean(v) for k, v in x.items()}
    if isinstance(x, list):
        return [_clean(v) for v in x]
    return x


def main() -> None:
    args = sys.argv[1:]
    out_dir = OUT
    if args[:1] == ["--out"]:
        out_dir = Path(args[1])
        args = args[2:]
    out_dir.mkdir(parents=True, exist_ok=True)
    while args:
        if args[0] != "--id" or len(args) < 3:
            sys.exit(__doc__)
        ident, path = args[1], Path(args[2])
        args = args[3:]
        try:
            data = res(path) if path.suffix.lower() == ".res" else export(path)
        except Exception as e:
            data = {"error": f"{type(e).__name__}: {e}"}
        oracle_json.write_text(out_dir / f"{ident}.json", json.dumps(_clean(data), indent=1, allow_nan=False) + "\n")
        print(ident, "curves", len(data.get("curves", [])), data.get("error", ""))


if __name__ == "__main__":
    main()
