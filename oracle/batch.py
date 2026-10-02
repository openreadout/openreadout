#!/usr/bin/env python
"""Ground truth for batch tables (`--tidy`), sample-sheet joins and group summaries.

Usage: uv run --group plate python batch.py      (writes ../corpus/oracle/batch/*.json)

Independent readers only, never openreadout:

* gate-medians.json: FlowKit 1.3 (BSD-3) gates the flowkit-8c-ics workspace's three samples
  (`GatingStrategy.gate_sample`); for every population, the median of FSC-A (scale values) and
  of two compensated fluorescence parameters (the workspace sample's matrix,
  `Sample.apply_compensation`) over the population's events (`get_gate_membership`), with NumPy.
* fcs-summaries.json: FlowIO 1.4 (BSD-3) `as_array(preprocess=True)` (scale values), per
  parameter: finite events, median, mean, sample sd, min, max (NumPy).
* image-stats.json: nd2 (BSD-3) per image (position) and channel over every z and t: mean,
  min, max, median (NumPy, float64).
* trace-stats.json: pyABF (MIT) per sweep of channel 0: mean, population std, min, max.
* plate-values.json: allotropy (MIT) ASM documents of single-read plate exports: well → value.
* summarize.json (+ summarize-input.csv): a seeded synthetic table (NumPy) summarized with
  pandas `groupby` (count, mean, std, sem, median, min, max) and SciPy `ttest_ind(equal_var=False)`
  and `mannwhitneyu` (defaults) against the control group, with and without replicate averaging.
* sheets.json (+ samples.xlsx, layout.xlsx, layout.csv): sample sheets and plate layouts written
  here with openpyxl and the csv module; the expected long rows are the values written.
"""
import csv
import json
import math
import os
import sys
import tomllib
from importlib.metadata import version
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
FILES = Path(os.environ.get("OPENREADOUT_CORPUS_DIR", ROOT / "corpus" / "files")).resolve()
OUT = ROOT / "corpus" / "oracle" / "batch"

with open(ROOT / "corpus" / "manifest.toml", "rb") as f:
    MANIFEST = [e for e in tomllib.load(f)["file"]]
BY_ID = {e["id"]: e for e in MANIFEST if e.get("role", "input") == "input"}


def path_of(cid):
    return FILES / BY_ID[cid]["filename"]


def num(v):
    v = float(v)
    return float(f"{v:.12g}") if math.isfinite(v) else None


def write(name, data):
    OUT.mkdir(parents=True, exist_ok=True)
    p = OUT / name
    p.write_text(json.dumps(data, indent=1, sort_keys=False) + "\n")
    print(f"wrote {p}")


# ---------------------------------------------------------------- flow

EIGHT = [
    ("flowkit-8c-e01", "101_DEN084Y5_15_E01_008_clean.fcs"),
    ("flowkit-8c-e03", "101_DEN084Y5_15_E03_009_clean.fcs"),
    ("flowkit-8c-e05", "101_DEN084Y5_15_E05_010_clean.fcs"),
]
MEDIAN_PARAMS = ["FSC-A", "Comp-CD4 PE-Cy7 FLR-A", "Comp-CD8 PerCP-Cy55 FLR-A"]


def gate_medians():
    import flowkit as fk

    wsp_id = "flowkit-8c-ics"
    wsp_path = FILES / next(e["filename"] for e in MANIFEST if e["id"] == wsp_id)
    wsp = fk.parse_wsp(str(wsp_path))
    out = {
        "generator": f"flowkit {fk.__version__} + numpy {np.__version__}",
        "gating": wsp_id,
        "parameters": MEDIAN_PARAMS,
        "samples": [],
    }
    for fcs_id, sample_name in EIGHT:
        s = wsp["samples"][sample_name]
        gs = s["gating_strategy"]
        sample = fk.Sample(str(path_of(fcs_id)), sample_id=sample_name, use_flowjo_labels=True, subsample=0)
        res = gs.gate_sample(sample)
        raw = sample.get_events(source="raw")
        comp_sample = fk.Sample(str(path_of(fcs_id)), sample_id=sample_name, use_flowjo_labels=True, subsample=0)
        comp_sample.apply_compensation(s["compensation"]["matrix"])
        comp = comp_sample.get_events(source="comp")
        labels = sample.pnn_labels
        pops = []
        for _, row in res.report.iterrows():
            gp = tuple(row["gate_path"])
            members = res.get_gate_membership(row["gate_name"], gate_path=gp)
            path = "/" + "/".join([p for p in gp if p != "root"] + [row["gate_name"]])
            if row["quadrant_parent"] is not None and isinstance(row["quadrant_parent"], str):
                continue  # quadrants: paths differ in convention; the 8c-ics workspace has none
            med = {}
            for p in MEDIAN_PARAMS:
                src, name = (comp, p[len("Comp-"):]) if p.startswith("Comp-") else (raw, p)
                vals = src[members, labels.index(name)]
                med[p] = num(np.median(vals)) if vals.size else None
            pops.append({"path": path, "count": int(members.sum()), "medians": med})
        pops.sort(key=lambda e: e["path"])
        out["samples"].append({"fcs": fcs_id, "sample": sample_name, "populations": pops})
    write("gate-medians.json", out)


FCS_SUMMARY = [
    "fcsparser-fortessa-a01",
    "fcsparser-hts-lsr-ii-d06",
    "fcsparser-facs-diva",
    "flowio-g11",
    "flowio-data1",
    "fcsparser-miltenyi-fcs31",
]


def fcs_summaries():
    import flowio

    out = {"generator": f"flowio {flowio.__version__} + numpy {np.__version__}", "files": []}
    for cid in FCS_SUMMARY:
        fd = flowio.FlowData(str(path_of(cid)), ignore_offset_error=True)
        a = fd.as_array(preprocess=True).astype(np.float64)
        names = list(fd.pnn_labels)
        params = []
        for i, n in enumerate(names):
            v = a[:, i]
            v = v[np.isfinite(v)]
            params.append({
                "parameter": n,
                "events": int(v.size),
                "median": num(np.median(v)),
                "mean": num(v.mean()),
                "sd": num(v.std(ddof=1)) if v.size > 1 else None,
                "min": num(v.min()),
                "max": num(v.max()),
            })
        out["files"].append({"id": cid, "event_count": int(a.shape[0]), "parameters": params})
    write("fcs-summaries.json", out)


# ---------------------------------------------------------------- images, traces

ND2 = ["aics-ND2-dims-p4z5t3c2y32x32", "aics-ND2-dims-t3c2y32x32", "aics-ND2-dims-p2z5t3-2c4y32x32", "aics-ND2-maxime-BF007"]


def image_stats():
    import nd2

    out = {"generator": f"nd2 {nd2.__version__} + numpy {np.__version__}", "files": []}
    for cid in ND2:
        with nd2.ND2File(str(path_of(cid))) as f:
            a = f.asarray()
            dims = list(f.sizes)
        # move P (positions) first and C last, flatten the rest
        p_ax = dims.index("P") if "P" in dims else None
        c_ax = dims.index("C") if "C" in dims else None
        if p_ax is None:
            a = a[np.newaxis]
            dims = ["P"] + dims
            c_ax = None if c_ax is None else c_ax + 1
        else:
            a = np.moveaxis(a, p_ax, 0)
            dims = ["P"] + [d for d in dims if d != "P"]
            c_ax = dims.index("C") if "C" in dims else None
        rows = []
        for p in range(a.shape[0]):
            ap = a[p]
            if c_ax is None:
                chans = [ap]
            else:
                ap = np.moveaxis(ap, c_ax - 1, 0)
                chans = list(ap)
            for c, v in enumerate(chans):
                v = v.astype(np.float64).ravel()
                rows.append({
                    "image": p,
                    "channel": c,
                    "count": int(v.size),
                    "mean": num(v.mean()),
                    "min": num(v.min()),
                    "max": num(v.max()),
                    "median": num(np.median(v)),
                })
        out["files"].append({"id": cid, "dims": dims, "rows": rows})
    write("image-stats.json", out)


ABF = ["pyabf-171116sh-0011", "pyabf-2018-12-09-pclamp11-0001", "pyabf-file-axon-7", "pyabf-2020-06-16-0000"]


def trace_stats():
    import pyabf

    out = {"generator": f"pyabf {pyabf.__version__} + numpy {np.__version__}", "files": []}
    for cid in ABF:
        abf = pyabf.ABF(str(path_of(cid)))
        rows = []
        for s in abf.sweepList:
            abf.setSweep(s, channel=0)
            y = np.asarray(abf.sweepY, dtype=np.float64)
            rows.append({
                "sweep": s,
                "samples": int(y.size),
                "mean": num(y.mean()),
                "std": num(y.std()),
                "min": num(y.min()),
                "max": num(y.max()),
            })
        out["files"].append({"id": cid, "channel": 0, "unit": abf.sweepUnitsY, "rows": rows})
    write("trace-stats.json", out)


PLATES = ["envision-abs-a450", "kaleido-abs-endpoint", "gen5-lum-endpoint"]


def plate_values():
    sys.path.insert(0, str(ROOT / "oracle"))
    import plate

    out = {"generator": f"allotropy {version('allotropy')}", "files": []}
    for cid in PLATES:
        p = path_of(cid)
        asm = plate.allotropy_asm(p, plate.vendor_for(p.name))
        wells = {}
        for doc in asm["plate reader aggregate document"]["plate reader document"]:
            for m in doc["measurement aggregate document"]["measurement document"]:
                if "error aggregate document" in m:
                    continue
                for k, v in m.items():
                    if plate.mode_of(k) and isinstance(v, dict) and "value" in v:
                        well = m["sample document"]["location identifier"]
                        wells.setdefault(well, []).append(num(v["value"]))
        out["files"].append({"id": cid, "wells": wells})
    write("plate-values.json", out)


# ---------------------------------------------------------------- summaries


def summarize():
    import pandas as pd
    from scipy import stats

    rng = np.random.default_rng(20260923)
    rows = []
    means = {"ctrl": 100.0, "low": 110.0, "high": 160.0}
    for cond, mu in means.items():
        for rep in (1, 2, 3):
            for tech in (1, 2):
                for ch, scale in (("GFP", 1.0), ("DAPI", 5.0)):
                    rows.append({
                        "path": f"{cond}_r{rep}_t{tech}.czi",
                        "format": "czi",
                        "channel_name": ch,
                        "condition": cond,
                        "replicate": rep,
                        "mean": round(float(rng.normal(mu * scale, 8.0 * scale)), 6),
                    })
    # one failed file: excluded by summaries
    rows.append({"path": "bad.czi", "format": "czi", "channel_name": "", "condition": "ctrl", "replicate": 9, "mean": ""})
    OUT.mkdir(parents=True, exist_ok=True)
    with open(OUT / "summarize-input.csv", "w", newline="") as fh:
        w = csv.DictWriter(fh, fieldnames=["path", "format", "channel_name", "condition", "replicate", "mean", "error"])
        w.writeheader()
        for r in rows:
            r = dict(r)
            r["error"] = "corrupt" if r["path"] == "bad.czi" else ""
            w.writerow(r)
    df = pd.DataFrame([r for r in rows if r["path"] != "bad.czi"])
    df["mean"] = df["mean"].astype(float)

    def describe(g):
        return {
            "n": int(g.count()),
            "mean": num(g.mean()),
            "sd": num(g.std(ddof=1)),
            "sem": num(g.sem()),
            "median": num(g.median()),
            "min": num(g.min()),
            "max": num(g.max()),
            "cv_percent": num(g.std(ddof=1) / abs(g.mean()) * 100),
        }

    out = {"generator": f"pandas {pd.__version__} + scipy {version('scipy')}", "plain": [], "replicate": []}
    for (cond, ch), g in df.groupby(["condition", "channel_name"]):
        entry = {"condition": cond, "channel_name": ch, **describe(g["mean"])}
        if cond != "ctrl":
            c = df[(df.condition == "ctrl") & (df.channel_name == ch)]["mean"]
            t = stats.ttest_ind(g["mean"], c, equal_var=False)
            u = stats.mannwhitneyu(g["mean"], c)
            entry.update({"welch_t": num(t.statistic), "welch_df": num(t.df), "welch_p": num(t.pvalue),
                          "mwu_u": num(u.statistic), "mwu_p": num(u.pvalue)})
        out["plain"].append(entry)
    rep = df.groupby(["condition", "channel_name", "replicate"], as_index=False)["mean"].mean()
    for (cond, ch), g in rep.groupby(["condition", "channel_name"]):
        entry = {"condition": cond, "channel_name": ch, **describe(g["mean"])}
        if cond != "ctrl":
            c = rep[(rep.condition == "ctrl") & (rep.channel_name == ch)]["mean"]
            t = stats.ttest_ind(g["mean"], c, equal_var=False)
            u = stats.mannwhitneyu(g["mean"], c)
            entry.update({"welch_t": num(t.statistic), "welch_df": num(t.df), "welch_p": num(t.pvalue),
                          "mwu_u": num(u.statistic), "mwu_p": num(u.pvalue)})
        out["replicate"].append(entry)
    write("summarize.json", out)


# ---------------------------------------------------------------- sheets


def sheets():
    import openpyxl

    OUT.mkdir(parents=True, exist_ok=True)
    rows_letters = "ABCDEFGH"
    # a 96-well layout: condition and dose, as two grids on one CSV and two worksheets of one XLSX
    cond = {}
    dose = {}
    for r in range(8):
        for c in range(12):
            well = f"{rows_letters[r]}{c + 1:02d}"
            cond[well] = "ctrl" if c < 3 else ("drugA" if c < 7 else ("drugB" if c < 11 else ""))
            dose[well] = "" if c >= 11 else str([0, 0, 0, 1, 1, 10, 10, 1, 1, 10, 10][c])
    with open(OUT / "layout.csv", "w", newline="") as fh:
        w = csv.writer(fh)
        w.writerow(["condition"] + [str(i) for i in range(1, 13)])
        for r in range(8):
            w.writerow([rows_letters[r]] + [cond[f"{rows_letters[r]}{c:02d}"] for c in range(1, 13)])
        w.writerow([])
        w.writerow(["dose_uM"])
        w.writerow([""] + [str(i) for i in range(1, 13)])
        for r in range(8):
            w.writerow([rows_letters[r]] + [dose[f"{rows_letters[r]}{c:02d}"] for c in range(1, 13)])
    wb = openpyxl.Workbook()
    ws = wb.active
    ws.title = "condition"
    ws.append([None] + list(range(1, 13)))
    for r in range(8):
        ws.append([rows_letters[r]] + [cond[f"{rows_letters[r]}{c:02d}"] or None for c in range(1, 13)])
    ws2 = wb.create_sheet("dose_uM")
    ws2.append([None] + list(range(1, 13)))
    for r in range(8):
        ws2.append([rows_letters[r]] + [float(dose[f"{rows_letters[r]}{c:02d}"]) if dose[f"{rows_letters[r]}{c:02d}"] else None for c in range(1, 13)])
    wb.save(OUT / "layout.xlsx")
    # a sample sheet workbook: a notes sheet first, then the table
    wb = openpyxl.Workbook()
    ws = wb.active
    ws.title = "notes"
    ws.append(["Experiment 42, see ELN"])
    ws2 = wb.create_sheet("samples")
    table = [
        ["File", "Donor", "Condition", "Dose (uM)"],
        ["run_001.fcs", 1, "control", 0],
        ["run_002.fcs", 1, "treated", 2.5],
        ["run_003", 2, "control", 0],
        ["RUN_004.FCS", 2, "treated", 2.5],
    ]
    for r in table:
        ws2.append(r)
    wb.save(OUT / "samples.xlsx")
    expect = {
        "layout": {
            "columns": ["well", "condition", "dose_uM"],
            "rows": [[w, cond[w], dose[w]] for w in sorted(cond) if cond[w] or dose[w]],
        },
        "samples_xlsx": {
            "worksheet": "samples",
            "columns": table[0],
            "rows": [[str(x) if not isinstance(x, float) else f"{x:g}" for x in r] for r in table[1:]],
        },
    }
    write("sheets.json", {"generator": f"openpyxl {openpyxl.__version__} + csv", **expect})


if __name__ == "__main__":
    jobs = {
        "gate": gate_medians,
        "fcs": fcs_summaries,
        "images": image_stats,
        "traces": trace_stats,
        "plates": plate_values,
        "summarize": summarize,
        "sheets": sheets,
    }
    for name in sys.argv[1:] or jobs:
        jobs[name]()
