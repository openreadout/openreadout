#!/usr/bin/env python
"""Ground truth for Cytiva Biacore result files (`cytiva-biacore-blr`) and T200 evaluation files
(`cytiva-biacore-bme`, which hold a result file under `_DataManager 1/`).

Usage:  python biacore_oracle.py [--out DIR] --id ID FILE ...

Two independent sources:

- **The control software's report points**: the `RPoint Table` stream (tab-separated text the
  Biacore software writes: per cycle and flow cell, the mean response `AbsResp` over a window
  around `Time`, with `Min`, `Max`, `RelResp`). Read with olefile (BSD-2-Clause, a generic
  compound-file library). The corpus test recomputes each mean from the reader's sensorgrams.
- **allotropy** (MIT) as a black box: its ASM output for the same file (sensorgram data cubes per
  cycle and flow cell, device, software, chip): curve labels, lengths, and times and responses at
  up to 64 evenly spaced samples per curve (all curves of the first 12 cycles).

Also recorded: the `Environment` and `Chip` streams' run facts (key=value text). For evaluation
files, the evaluation software's fits, parsed here from the item XML with ElementTree (our own
reading, independent of the Rust reader): per fit its sample, curve names, model, Chi² and the
first value and standard error of each parameter, in item order; and for steady-state affinity
fits a refit of `Conc*Rmax/(Conc+KD)+offset` to the stored concentrations and responses
(scipy's least squares, BSD) as a check that concentrations, responses and KD belong together
(`refit`: the fits whose KD the refit reproduces within 1 %).

Writes `<DIR>/<ID>.json`; `crates/openreadout-corpus-tests/tests/biacore_oracle/mod.rs` compares.
"""
from __future__ import annotations

import csv
import io
import json
import math
import sys
from importlib.metadata import version
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "corpus" / "oracle" / "biacore"
MAX_CYCLES = 12
SAMPLES = 64


def _num(v):
    try:
        x = float(v)
    except (TypeError, ValueError):
        return None
    return x if math.isfinite(x) else None


def kv(text: str) -> dict:
    out = {}
    for line in text.splitlines():
        if "=" in line:
            k, v = line.split("=", 1)
            out[k.strip()] = v.strip()
    return out


def report_points(ole, pre: str = "") -> list:
    if not ole.exists(pre + "RPoint Table"):
        return []
    text = ole.openstream(pre + "RPoint Table").read().decode("latin-1")
    rows = []
    for r in csv.DictReader(io.StringIO(text), delimiter="\t"):
        rows.append({
            "cycle": int(r["Cycle"]),
            "fc": r["Fc"].strip(),
            "time": _num(r["Time"]),
            "window": _num(r["Window"]),
            "abs_resp": _num(r["AbsResp"]),
            "rel_resp": _num(r["RelResp"]),
            "min": _num(r["Min"]),
            "max": _num(r["Max"]),
            "id": r["Id"],
        })
    return rows


def allotropy_curves(path: Path, evaluation: bool = False) -> dict:
    from allotropy.parser_factory import Vendor
    from allotropy.to_allotrope import allotrope_from_file

    vendor = Vendor.CYTIVA_BIACORE_T200_EVALUATION if evaluation else Vendor.CYTIVA_BIACORE_T200_CONTROL
    d = allotrope_from_file(str(path), vendor)
    agg = d["binding affinity analyzer aggregate document"]
    dev = agg.get("device system document", {})
    sysd = agg.get("data system document", {})
    curves = []
    chip = {}
    for doc in agg["binding affinity analyzer document"]:
        for m in doc["measurement aggregate document"]["measurement document"]:
            cube = m.get("sensorgram data cube")
            if not cube:
                continue
            label = cube["label"]  # Cycle<N>_FlowCell<fc>
            cyc_s, fc = label.split("_FlowCell", 1)
            cycle = int(cyc_s.removeprefix("Cycle"))
            if cycle > MAX_CYCLES:
                continue
            t = cube["data"]["dimensions"][0]
            y = cube["data"]["measures"][0]
            n = len(y)
            idx = sorted({round(i * (n - 1) / (SAMPLES - 1)) for i in range(SAMPLES)}) if n else []
            curves.append({
                "label": label, "cycle": cycle, "flow_cell": fc, "n": n,
                "samples": [[i, t[i], y[i]] for i in idx],
            })
            sc = m.get("sensor chip document", {})
            if sc and not chip:
                chip = {"identifier": sc.get("sensor chip identifier"), "type": sc.get("sensor chip type")}
    return {
        "device_identifier": dev.get("device identifier"),
        "model_number": dev.get("model number"),
        "software_name": sysd.get("software name"),
        "software_version": sysd.get("software version"),
        "chip": chip,
        "max_cycles": MAX_CYCLES,
        "curves": curves,
    }


def _fit_records(ole) -> list:
    """The fits of every evaluation item, in item order (modelFit elements, then Fits/FitN)."""
    import re
    import xml.etree.ElementTree as ET

    names = ["/".join(e) for e in ole.listdir(streams=True, storages=False)]
    items = sorted((int(m.group(1)), n) for n in names if (m := re.fullmatch(r"Evaluation/EvaluationItem(\d+)", n)))
    out = []

    def curves(cs):
        pts, subsets = [], []
        if cs is None:
            return pts, subsets
        for sub in cs:
            if not sub.tag.startswith("Subset"):
                continue
            subsets.append((sub.findtext("CurveName") or "").strip())
            for cv in sub:
                if not re.fullmatch(r"Curve\d+", cv.tag):
                    continue
                inj = cv.find("Injections/Injection")
                inc = (cv.findtext("Included") or "").strip()
                pts.append({
                    "conc": _num(inj.findtext("MolarConcentration")) if inj is not None else None,
                    "response": _num(inj.findtext("Response")) if inj is not None else None,
                    "included": None if inc == "" else inc == "true",
                })
        return pts, subsets

    def record(model, cs, key):
        pts, subsets = curves(cs)
        get = lambda el, tag: (el.findtext(tag) or "").strip() if el is not None else ""
        sample = get(cs, "SampleName") or get(key, "sampleName")
        curve = ";".join(subsets) if any(subsets) else get(key, "curveName")
        params = {}
        for entry in (model.findtext("Parameters") or "").split(";"):
            if "|" not in entry:
                continue
            head, rest = entry.split("|", 1)
            name = head.rsplit(":", 1)[-1].strip()
            v = rest.split("|")
            if name and name not in params:
                params[name] = [_num(v[0]), _num(v[1]) if len(v) > 1 else None]
        return {"sample": sample, "curve": curve, "model": model.get("ModelName", ""),
                "chi2": _num(model.findtext("Chi2")), "curves": len(pts), "params": params,
                "expression": (model.findtext("Model") or "").strip(), "points": pts}

    for _n, name in items:
        root = ET.fromstring(ole.openstream(name).read().decode("latin-1").split("\x00", 1)[0])
        for mf in root.iter("modelFit"):
            model = mf.find("model")
            if model is not None:
                out.append(record(model, mf.find("curveSet/CurveSet"), mf.find("key")))
        fits = root.find("Fits")
        if fits is not None:
            for f in fits:
                if re.fullmatch(r"Fit\d+", f.tag):
                    out.append(record(f, root.find("CurveSet"), None))
    return out


def _refit(fit) -> float | None:
    """KD of a steady-state affinity refit to the stored points (offset fixed at 0 when the stored
    fit has it 0 with no error), or None."""
    if fit["expression"].replace(" ", "") != "Conc*Rmax/(Conc+KD)+offset":
        return None
    import numpy as np
    from scipy.optimize import least_squares

    pts = [p for p in fit["points"] if p["conc"] and p["response"] is not None and p["included"] is not False]
    if len(pts) < 3:
        return None
    x = np.array([p["conc"] for p in pts])
    y = np.array([p["response"] for p in pts])
    kd0 = fit["params"].get("KD", [None])[0] or float(np.median(x))
    rm0 = fit["params"].get("Rmax", [None])[0] or float(y.max())
    off = fit["params"].get("offset", [0.0, None])
    free_offset = not (off[0] == 0 and (off[1] in (0, None)))
    def res(p):
        kd, rm = p[0], p[1]
        o = p[2] if free_offset else 0.0
        return x * rm / (x + kd) + o - y
    p0 = [kd0, rm0] + ([off[0] or 0.0] if free_offset else [])
    try:
        r = least_squares(res, p0, x_scale="jac", xtol=1e-14, ftol=1e-14, gtol=1e-14, max_nfev=20000)
    except Exception:  # noqa: BLE001
        return None
    return float(r.x[0]) if r.success else None


def main(argv: list[str]) -> int:
    out_dir = OUT
    ident = None
    files = []
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
    import olefile

    path = files[0]
    ole = olefile.OleFileIO(str(path))
    compat = ole.openstream("\x03BIA compability info").read().decode("latin-1")
    evaluation = "FileType=T200 Evaluation File" in compat
    pre = "_DataManager 1/" if evaluation else ""
    env = kv(ole.openstream(pre + "Environment").read().decode("latin-1"))
    chip = kv(ole.openstream(pre + "Chip").read().decode("latin-1"))
    out = {
        "id": ident,
        "format": "cytiva-biacore-bme" if evaluation else "cytiva-biacore-blr",
        "reader": f"olefile {version('olefile')} (BSD-2-Clause) for the report-point text; allotropy {version('allotropy')} (MIT, black box)",
        "environment": {k: env.get(k) for k in ("Application", "Version", "ProcessingUnit", "InstrumentId", "UserName", "RunType", "Timestamp", "EndTime")},
        "chip": {k: chip.get(k) for k in ("Name", "Id", "NoFcs")},
        "cycles": len({e[1] if evaluation else e[0] for e in ole.listdir(streams=True, storages=False)
                       if (e[0] == "_DataManager 1" and len(e) > 1 and e[1].startswith("_Cycle ")) or (not evaluation and e[0].startswith("_Cycle "))}),
        "report_points": report_points(ole, pre),
    }
    try:
        out["allotropy"] = allotropy_curves(path, evaluation)
    except Exception as e:  # noqa: BLE001 - recorded, the test then skips the comparison
        out["allotropy_error"] = f"{type(e).__name__}: {e}"
    if evaluation:
        fits = _fit_records(ole)
        refits, agree = 0, 0
        for f in fits:
            kd = _refit(f)
            stored = f["params"].get("KD", [None])[0]
            if kd is not None and stored:
                refits += 1
                f["refit_kd"] = kd
                if abs(kd - stored) <= 0.01 * abs(stored):
                    agree += 1
            del f["points"]
        out["fits"] = fits
        out["refit"] = {"steady_state_fits": refits, "kd_within_1pct": agree}
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / f"{ident}.json").write_text(json.dumps(out, indent=1, allow_nan=False) + "\n")
    print(ident, len(out["report_points"]), "report points,", len(out.get("allotropy", {}).get("curves", [])), "allotropy curves,",
          len(out.get("fits", [])), "fits", out.get("refit", ""))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
