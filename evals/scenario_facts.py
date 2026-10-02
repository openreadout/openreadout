"""Scenario tier: realistic end-to-end lab requests over a staged folder.

A real request is rarely one lookup: "here is our GC-MS reaction series and the sample sheet — when
does the product overtake the internal standard?", "this plate plus the compound list: which
compounds are hits?", "these ICS tubes and the FlowJo workspace: what is the background-subtracted
IFN-γ response?", "export these ND2 z-stacks to OME-Zarr and tell me the z-step". Each scenario
stages a folder `sample_data/` holding development-corpus files under the names a lab would use (with
their companions, as APFS clones like evals/share.py), plus the lab's own sheet where one is natural
(a sample sheet, plate map, compound list or integration-region list, fixed here and stored in the
question), and asks one request whose answer is a single scoreable value, name or list. Where a
method matters the prompt states it ("single-point external standard", "4PL on log10
concentration", "normalise to the mean Cq of ACT1 and TFC1"), and tolerances accept any correct way
of doing it.

The answers are computed here, never with OpenReadout: vendor-computed values stored with the
data (MSD ChemStation `RESULTS.CSV`, ChemStation `Report.TXT`, the LabSolutions peak tables inside
ANDI files, Gen5's own normalisation, TopSpin `integrals.txt`, the vendor Ct values inside `.eds`
files, LinRegPCR Cq values and exclusion flags in RDML) and third-party readers run in the oracle
venv (pyteomics on the depositor's mzML, FlowKit, pyABF, allotropy + SciPy, tifffile, nd2,
brukeropus, Aston, scipy's netCDF reader), with plain NumPy/statistics arithmetic; the computation
is cross-checked a second way where one exists. The values go to `evals/facts/scenario.json`
(committed), so the questions regenerate without the corpus; `--check` recomputes and fails on
drift. The question builders at the bottom use the standard library only (generate.py imports them).

    oracle/.venv/bin/python evals/scenario_facts.py          # recompute evals/facts/scenario.json
    oracle/.venv/bin/python evals/scenario_facts.py --check  # exit 1 if the committed facts differ

(`sh oracle/setup_full_env.sh` builds an oracle venv with every reader used here.)
"""

from __future__ import annotations

import argparse
import json
import math
import re
import statistics
import sys
import tomllib
from importlib.metadata import version
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
sys.path.append(str(ROOT / "oracle"))
import oracle_json  # noqa: E402  (oracle/oracle_json.py: <id>.json, or <id>.json.gz over 1 MiB)

FACTS = HERE / "facts" / "scenario.json"
MANIFEST = ROOT / "corpus" / "manifest.toml"
DIR = "sample_data"


def manifest() -> list[dict]:
    with MANIFEST.open("rb") as fh:
        return tomllib.load(fh)["file"]


def entry(cid: str, role: str = "input") -> dict:
    for e in manifest():
        if e["id"] == cid and e.get("role", "input") == role:
            if e.get("tier") == "heldout" or e.get("role") == "heldout":
                raise SystemExit(f"{cid} is a held-out file: never used for scenarios")
            return e
    raise SystemExit(f"{cid} ({role}) not in the manifest")


def corpus() -> Path:
    sys.path.insert(0, str(HERE))
    import facts

    return facts.corpus_dir()


def cpath(cid: str, role: str = "input", sub: str = "") -> Path:
    p = corpus() / entry(cid, role)["filename"]
    return p / sub if sub else p


def rd(*packages: str) -> str:
    return " + ".join(f"{p} {version(p)}" for p in packages)


# ================================================================ scenario definitions (stdlib)
# Staging plans and generated sheets are pure functions of this module, so the questions
# regenerate without the corpus.

# --- chromatography: GC-MS reaction monitoring with an internal standard
GCMS_RUNS = [("00", 0), ("01", 1), ("03", 3), ("05", 5), ("08", 8), ("12", 12)]
GCMS_IS_RT, GCMS_PRODUCT_RT = 6.30, 10.29


def gcms_cid(nn: str) -> str:
    return "chromhandler-rau-r505-00-data-ms" if nn == "00" else f"chromhandler-rau-r505-{nn}-d-data-ms"


# --- chromatography: HPLC-PDA ANDI exports with the LabSolutions peak tables (MTBLS1892)
HPLC_SAMPLES = [
    ("B01", "MicroTom", "DAF-2"),
    ("B07", "MicroTom", "DAF0"),
    ("B13", "MicroTom", "DAF2P"),
    ("B20", "MicroTom", "DAF4P"),
    ("B25", "MicroTom", "DAF2E"),
    ("B31", "MicroTom", "DAF4E"),
    ("B36", "MicroTom", "DAF4E"),
    ("d07", "della", "DAF0"),
    ("d19", "della", "DAF4P"),
    ("d30", "della", "DAF2E"),
]
HPLC_ANALYTE_S, HPLC_IS_S = 1641.0, 3418.5


def hplc_cid(sample: str, ch: int) -> str:
    return f"mtbls1892-{sample.lower()}-pda-ch{ch}-cdf"


# --- LC-MS carry-over (MTBLS404, negative ESI)
LCMS_SEQUENCE = [
    ("run_01.raw", "mtbls404-QC1_001", "pooled QC"),
    ("run_02.raw", "mtbls404-Blanc04", "blank"),
    ("run_03.raw", "mtbls404-HU_neg_017", "urine sample"),
    ("run_04.raw", "mtbls404-Blanc04_b2", "blank"),
]
HIPPURATE_MZ, LCMS_WINDOW = 178.0510, (5.5, 6.1)

# --- flow: ICS tubes with the lab's FlowJo workspace
ICS_TUBES = [
    ("flowkit-8c-e01", "unstimulated"),
    ("flowkit-8c-e03", "peptide pool A"),
    ("flowkit-8c-e05", "peptide pool B"),
]
ICS_CD8 = "/Time/Singlets/aAmine-/CD3+/CD8+"

# --- ephys: current-clamp step protocols
EPHYS_CELLS = [
    ("cell_01.abf", "pyabf-171116sh-0018", "pyramidal"),
    ("cell_02.abf", "pyabf-2019-07-24-0055-fsi", "fast-spiking interneuron"),
    ("cell_03.abf", "pyabf-190619b-0003", "pyramidal"),
]

# --- plates
IC50_ROWS = {"cmpd-A": "BC", "cmpd-B": "DE", "cmpd-C": "FG"}
IC50_TOP_UM = 10.0  # column 1; 1:3 serial dilution to column 11


def ic50_conc_um(col: int) -> float:
    return IC50_TOP_UM / 3 ** (col - 1)


def ic50_plate_map() -> str:
    rows = ["well,role,compound,concentration_uM"]
    for r in "ABCDEFGH":
        for c in range(1, 13):
            cmpd = next((k for k, v in IC50_ROWS.items() if r in v), None)
            if cmpd is None:
                rows.append(f"{r}{c},empty,,")
            elif c == 12:
                rows.append(f"{r}{c},blank,,")
            else:
                rows.append(f"{r}{c},sample,{cmpd},{ic50_conc_um(c):.6g}")
    return "\n".join(rows) + "\n"


def screen_compound_id(spl: int) -> str:
    return f"KX-{1000 + 7 * spl:04d}"


def screen_list() -> str:
    return "sample_id,compound\n" + "".join(f"SPL{n},{screen_compound_id(n)}\n" for n in range(1, 81))


HCS_WELLS = {"A01": ("DMSO", "0.1 %"), "C07": ("cmpd-17", "10 uM")}

# --- microscopy
BLEACH_RUNS = [
    ("run_a.nd2", "ome-aryeh-MeOh-high-fluo-003"),
    ("run_b.nd2", "ome-aryeh-MeOh-high-fluo-007"),
    ("run_c.nd2", "ome-aryeh-MeOh-high-fluo-011"),
]
ZARR_STACKS = [("stack_a", "aics-ND2-jonas-header-test2"), ("stack_b", "ome-jonas-nd2Test-Exception9-e3")]

# --- qPCR
RDML_CID = "rdml-kphaffii-upr-linregpcr"
RDML_CONTROL = ["28_1", "28_2", "28_3"]
RDML_TREATED = ["D28_1", "D28_2", "D28_3"]
RDML_TARGET, RDML_REFS = "HAC1u", ("ACT1", "TFC1")
EDS_CID = "eds-7500-abhd17c-ddct"
EDS_CONTROL, EDS_TREATED, EDS_TARGET, EDS_REF = "ABHD17C OE", "Lenvatinib/ABHD17C OE", "ABHD17C", "18s"

# --- spectroscopy
BITUMEN = [
    ("opus-bitumen-unaged-2", "Unaged.2"),
    ("opus-bitumen-1h-180c-2", "1h_180C.2"),
    ("opus-bitumen-5h-120c-3", "5h_120C.3"),
]
SULFOXIDE, ALIPHATIC = (980.0, 1060.0), (1350.0, 1525.0)

# --- share health
DAMAGED_INTACT = [
    "flowio-g11",
    "pyabf-171116sh-0011",
    "mzdata-three-test-scans",
    "aics-ND2-dims-t3c2y32x32",
    "cheminfo-agilent-hplc-cdf",
    "eds-7500-abhd17c-ddct",
    "zenodo7015307-T-2-CH-1",
]
DAMAGED_CORRUPT = ["fcsparser-cytek-nl-2000-header", "synthetic-mzml-truncated"]
DAMAGED_TRUNCATE = ["mtbls20-caffeine-pos", "pyabf-2018-11-16-sh-0006"]
TRUNCATE_FRACTION = 0.6
RECONCILE_FCS = [
    "fcsparser-fortessa-a01",
    "fcsparser-hts-lsr-ii-d06",
    "zenodo18439538-cytoflex",
    "zenodo7971252-sony-sa3800",
    "fcsparser-facs-diva",
]
RECONCILE_MISSING = ["D7", "C05"]

# --- GC-FID single-point external standard
GCFID = ["0101", "0102", "0103", "0104"]
GCFID_STD_PERCENT = 5.00

# --- NMR integration list
NMR_CID = "nmrxiv-s846-50"


# ================================================================ facts (oracle venv)


def f_gcms_reaction() -> dict:
    """Product (10.29 min) / internal standard (6.30 min) TIC area ratio per time point: the MSD
    ChemStation integrator results (RESULTS.CSV) where the vendor stored them; Aston's TIC with a
    window integration for the t = 0 run (no results file) and as a cross-check of the others."""
    import numpy as np
    import scipy.io
    import scipy.io.netcdf

    sys.path.insert(0, str(HERE))
    import quant

    scipy.io.netcdf.NetCDFFile = scipy.io.netcdf_file  # Aston imports a name newer SciPy dropped
    from aston.tracefile import TraceFile

    def aston_ratio(p: Path) -> float:
        tr = TraceFile(str(p)).total_trace()
        t = np.asarray(tr.index, dtype="float64")
        y = np.asarray(tr.values, dtype="float64").ravel()

        def area(lo: float, hi: float) -> float:
            m = (t >= lo) & (t <= hi)
            base = np.interp(t[m], [t[m][0], t[m][-1]], [y[m][0], y[m][-1]])
            return float(np.trapezoid(y[m] - base, t[m]))

        return area(10.20, 10.40) / area(6.22, 6.40)

    rows = []
    for nn, hours in GCMS_RUNS:
        p = cpath(gcms_cid(nn))
        cross = aston_ratio(p)
        vendor = None
        if nn != "00":
            peaks = quant.results_csv(f"chromhandler-rau-r505-{nn}-d-results-csv")

            def near(rt: float, peaks: list[dict] = peaks) -> float:
                best = min(peaks, key=lambda r: abs(r["rt_min"] - rt))
                return best["area"] if abs(best["rt_min"] - rt) < 0.05 else 0.0

            vendor = near(GCMS_PRODUCT_RT) / near(GCMS_IS_RT)
            if not math.isclose(vendor, cross, rel_tol=0.25, abs_tol=0.05):
                raise SystemExit(f"GC-MS R505-{nn}: RESULTS.CSV ratio {vendor} vs Aston window {cross}")
        rows.append(
            {
                "run": f"RAU-R505-{nn}",
                "time_h": hours,
                "ratio": vendor if vendor is not None else cross,
                "source": "RESULTS.CSV" if vendor is not None else "Aston TIC window",
                "aston_ratio": cross,
            }
        )
    first = next(r for r in rows if r["ratio"] > 1.0)
    before = [r for r in rows if r["time_h"] < first["time_h"]]
    if first["ratio"] < 1.1 or max(r["ratio"] for r in before) > 0.9:
        raise SystemExit(f"GC-MS ratios too close to 1.0 to rank: {rows}")
    return {
        "value": first["time_h"],
        "runs": rows,
        "reader": "MSD ChemStation RESULTS.CSV + " + rd("aston"),
        "method": "product/IS TIC area ratio per run; first time point above 1.0",
    }


def f_hplc_is_ratio() -> dict:
    """Analyte (27.35 min) / internal standard (56.97 min) peak-area ratio from the LabSolutions
    peak table stored in each PDA channel-2 ANDI file (scipy netCDF); mean per genotype."""
    from scipy.io import netcdf_file

    rows = []
    for sample, genotype, stage in HPLC_SAMPLES:
        with netcdf_file(cpath(hplc_cid(sample, 2)), "r", mmap=False) as f:
            rt = [float(x) for x in f.variables["peak_retention_time"][:]]
            area = [float(x) for x in f.variables["peak_area"][:]]
        ia = min(range(len(rt)), key=lambda i: abs(rt[i] - HPLC_ANALYTE_S))
        ii = min(range(len(rt)), key=lambda i: abs(rt[i] - HPLC_IS_S))
        assert abs(rt[ia] - HPLC_ANALYTE_S) < 15 and abs(rt[ii] - HPLC_IS_S) < 3, (sample, rt)
        merged = sum(a for t, a in zip(rt, area, strict=True) if 3405 <= t <= 3425)
        rows.append(
            {
                "sample": sample,
                "genotype": genotype,
                "stage": stage,
                "analyte_area": area[ia],
                "is_area": area[ii],
                "ratio": area[ia] / area[ii],
                "ratio_merged_is": area[ia] / merged,
            }
        )

    def ratio_of_means(key: str) -> float:
        mt = statistics.mean(r[key] for r in rows if r["genotype"] == "MicroTom")
        de = statistics.mean(r[key] for r in rows if r["genotype"] == "della")
        return mt / de

    v, alt = ratio_of_means("ratio"), ratio_of_means("ratio_merged_is")
    return {
        "value": v,
        "value_if_is_pair_merged": alt,
        "samples": rows,
        "reader": rd("scipy") + " netcdf_file (LabSolutions peak table: peak_retention_time, peak_area)",
        "method": "per sample analyte/IS area ratio; mean per genotype; MicroTom mean / della mean",
    }


def f_lcms_carryover() -> dict:
    """Hippuric acid [M-H]- XIC apex in 5.5-6.1 min (pyteomics on the depositor's mzML; summed
    intensity within ±10 ppm per MS1 scan) of each blank as % of the pooled QC's."""
    import numpy as np

    sys.path.insert(0, str(HERE))
    import analysis as a

    lo, hi = HIPPURATE_MZ * (1 - 10e-6), HIPPURATE_MZ * (1 + 10e-6)
    apex, apex_max = {}, {}
    for staged, cid, _role in LCMS_SEQUENCE:
        best = best_max = 0.0
        for s in a.mzml_spectra(cid):
            if s["level"] != 1 or not (LCMS_WINDOW[0] <= s["rt_min"] <= LCMS_WINDOW[1]):
                continue
            m = (s["mz"] >= lo) & (s["mz"] <= hi)
            best = max(best, float(s["i"][m].sum()))
            best_max = max(best_max, float(s["i"][m].max()) if m.any() else 0.0)
        apex[staged], apex_max[staged] = best, best_max
    qc = LCMS_SEQUENCE[0][0]
    blanks = [s for s, _, role in LCMS_SEQUENCE if role == "blank"]
    pct = {b: 100 * apex[b] / apex[qc] for b in blanks}
    pct_max = {b: 100 * apex_max[b] / apex_max[qc] for b in blanks}
    worst = max(pct, key=pct.get)
    assert abs(pct[worst] - pct_max[worst]) / pct[worst] < 0.15, (pct, pct_max)  # within the tolerance
    assert np.isfinite(pct[worst])
    return {
        "value": pct[worst],
        "blank": worst,
        "percent_of_qc": pct,
        "percent_of_qc_single_point": pct_max,
        "apex": apex,
        "reader": rd("pyteomics") + " on the depositor's mzML",
        "method": "XIC (sum within ±10 ppm per MS1 scan) apex in 5.5-6.1 min; blank / QC x 100",
    }


def f_flow_ics() -> dict:
    """FlowKit gating of the lab's FlowJo workspace; % IFNg+ of CD8+ per tube, minus the
    unstimulated tube."""
    import flowkit as fk

    wsp = fk.parse_wsp(str(cpath("flowkit-8c-ics", "analysis")))
    pct = {}
    for cid, stim in ICS_TUBES:
        name = next(n for n in wsp["samples"] if cid[-3:].upper() in n)
        s = fk.Sample(str(cpath(cid)), sample_id=name, use_flowjo_labels=True, subsample=0)
        res = wsp["samples"][name]["gating_strategy"].gate_sample(s)
        counts = {}
        for _, r in res.report.iterrows():
            counts["/" + "/".join([x for x in r["gate_path"] if x != "root"] + [r["gate_name"]])] = int(r["count"])
        pct[stim] = {
            "sample": name,
            "cd8": counts[ICS_CD8],
            "ifng": counts[ICS_CD8 + "/IFNg+"],
            "percent": 100 * counts[ICS_CD8 + "/IFNg+"] / counts[ICS_CD8],
        }
    bg = pct["unstimulated"]["percent"]
    sub = {k: v["percent"] - bg for k, v in pct.items() if k != "unstimulated"}
    best = max(sub, key=sub.get)
    return {
        "value": sub[best],
        "pool": best,
        "background_subtracted": sub,
        "tubes": pct,
        "reader": rd("flowkit"),
        "method": "workspace gates; % IFNg+ of CD8+ minus the unstimulated tube",
    }


def f_ephys_rheobase() -> dict:
    sys.path.insert(0, str(HERE))
    import analysis as a

    cells = {}
    for staged, cid, kind in EPHYS_CELLS:
        rb, how = a.abf_rheobase_numpy(cid)
        cells[staged] = {"type": kind, "rheobase_pa": rb}
    pyr = [c["rheobase_pa"] for c in cells.values() if c["type"] == "pyramidal"]
    return {
        "value": statistics.mean(pyr),
        "cells": cells,
        "reader": how,
        "method": "smallest positive step level whose step window holds an upward 0 mV crossing; mean",
    }


def f_plate_ic50() -> dict:
    """Least potent compound's IC50 (nM): joint 4PL fit on log10 concentration over both replicate
    rows (allotropy values, SciPy), with two other fitting conventions as a tolerance check."""
    import numpy as np
    from scipy.optimize import curve_fit

    sys.path.insert(0, str(HERE))
    import analysis as a

    vals = a.plate_values("skanit-luciferase")
    bg = statistics.mean(vals[f"{r}12"] for rows in IC50_ROWS.values() for r in rows)

    def f4(lx, bot, top, lic, h):
        return bot + (top - bot) / (1 + 10 ** ((lx - lic) * h))

    def fit(x, y) -> float:
        p, _ = curve_fit(f4, x, y, p0=[min(y), max(y), float(np.median(x)), 1.0], maxfev=50000)
        return float(10 ** p[2] * 1000)  # nM

    out = {}
    for cmpd, rows in IC50_ROWS.items():
        lx = np.log10([ic50_conc_um(c) for c in range(1, 12)])
        ys = [np.array([vals[f"{r}{c}"] for c in range(1, 12)]) for r in rows]
        joint = fit(np.concatenate([lx, lx]), np.concatenate(ys))
        means = fit(lx, (ys[0] + ys[1]) / 2)
        norm = fit(np.concatenate([lx, lx]), np.concatenate([100 * (y - bg) / (y.max() - bg) for y in ys]))
        per_row = math.sqrt(fit(lx, ys[0]) * fit(lx, ys[1]))
        out[cmpd] = {
            "ic50_nm": joint,
            "ic50_nm_fit_on_means": means,
            "ic50_nm_normalised": norm,
            "ic50_nm_geomean_of_row_fits": per_row,
        }
    ranked = sorted(out, key=lambda k: out[k]["ic50_nm"], reverse=True)
    if out[ranked[0]]["ic50_nm"] < 2 * out[ranked[1]]["ic50_nm"]:
        raise SystemExit(f"IC50s too close to name the least potent: {out}")
    w = out[ranked[0]]
    spread = max(abs(w[k] - w["ic50_nm"]) / w["ic50_nm"] for k in w)
    return {
        "value": w["ic50_nm"],
        "compound": ranked[0],
        "compounds": out,
        "method_spread": spread,
        "reader": rd("allotropy", "scipy"),
        "method": "4PL (bottom, top, log IC50, Hill) on log10 concentration",
    }


def f_plate_screen() -> dict:
    """Gen5's own normalisation (NormLum, % of the POSCON-NEGCON window) stored under each well,
    and the same number recomputed from the raw reads and the file's control layout."""
    text = cpath("gen5-lum-endpoint").read_text(errors="replace").splitlines()
    i = text.index("Layout")
    layout = {}
    for line in text[i + 2 : i + 10]:
        cells = line.split("\t")
        for c, v in enumerate(cells[1:13], start=1):
            layout[f"{cells[0]}{c}"] = v.strip()
    j = text.index("Results")
    raw, norm = {}, {}
    for k in range(j + 2, j + 18, 2):
        a, b = text[k].split("\t"), text[k + 1].split("\t")
        assert a[-1].startswith("LUM") and b[-1].startswith("NormLum"), (a[-1], b[-1])
        for c in range(1, 13):
            raw[f"{a[0]}{c}"] = float(a[c])
            norm[f"{a[0]}{c}"] = float(b[c])
    pos = statistics.mean(v for w, v in raw.items() if layout[w] == "POSCON")
    neg = statistics.mean(v for w, v in raw.items() if layout[w] == "NEGCON")
    for w in raw:
        mine = 100 * (raw[w] - neg) / (pos - neg)
        assert abs(mine - norm[w]) < 0.01, (w, mine, norm[w])
    spl = {int(v[3:]): norm[w] for w, v in layout.items() if v.startswith("SPL")}
    hits = sorted(screen_compound_id(n) for n, v in spl.items() if v < 10)
    near = sorted(v for v in spl.values() if 5 <= v < 15)
    return {
        "value": hits,
        "activity_percent": {screen_compound_id(n): v for n, v in sorted(spl.items())},
        "closest_to_cutoff": near,
        "reader": "Gen5 NormLum rows (vendor), recomputed from LUM and the layout",
        "method": "activity % = (x - mean NEGCON) / (mean POSCON - mean NEGCON) x 100; hits below 10 %",
    }


def _harmony_images(cid: str) -> list[dict]:
    import xml.etree.ElementTree as ET

    index = cpath(cid) / "Images" / "Index.idx.xml"
    out = []
    for _, el in ET.iterparse(index, events=("end",)):
        tag = el.tag.rsplit("}", 1)[-1]
        if tag == "Image" and any(c.tag.rsplit("}", 1)[-1] == "URL" for c in el):
            d = {c.tag.rsplit("}", 1)[-1]: (c.text or "").strip() for c in el}
            out.append(d)
            el.clear()
    return out


def f_hcs_dapi() -> dict:
    """Mean raw DAPI intensity over every imaged field of each well (tifffile on the planes the
    Harmony index names); % change of the compound well against the DMSO well."""
    import numpy as np
    import tifffile

    cid = "hcs-harmony-idr0034-folder"
    folder = cpath(cid) / "Images"
    per: dict[str, list[float]] = {}
    for d in _harmony_images(cid):
        if d.get("ChannelName") != "DAPI" or not d.get("URL") or not (folder / d["URL"]).is_file():
            continue
        well = f"{chr(64 + int(d['Row']))}{int(d['Col']):02d}"
        per.setdefault(well, []).append(float(tifffile.imread(folder / d["URL"]).astype(np.float64).mean()))
    assert set(per) == set(HCS_WELLS), per.keys()
    means = {w: statistics.mean(v) for w, v in per.items()}
    change = 100 * (means["C07"] - means["A01"]) / means["A01"]
    return {
        "value": change,
        "well_means": means,
        "fields": {w: len(v) for w, v in per.items()},
        "reader": rd("tifffile"),
        "method": "mean of all DAPI field planes per well; (C07 - A01) / A01 x 100",
    }


def f_hcs_incomplete() -> dict:
    """Wells of the ImageXpress copy that have some but not all wavelengths the HTD lists."""
    folder = cpath("hcs-imagexpress-idr0081-folder")
    htd = (folder / "BSF018292-1A.HTD").read_text(errors="replace")
    waves = int(re.search(r'"NWavelengths",\s*(\d+)', htd).group(1))
    have: dict[str, set[int]] = {}
    for f in folder.iterdir():
        m = re.fullmatch(r".+_([A-P]\d{2})_w(\d)\.TIF", f.name, flags=re.I)
        if m:
            have.setdefault(m.group(1), set()).add(int(m.group(2)))
    missing = sorted(w for w, s in have.items() if len(s) < waves)
    return {
        "value": missing,
        "wells_with_images": {w: sorted(s) for w, s in sorted(have.items())},
        "wavelengths": waves,
        "reader": "HTD text + directory listing (Python standard library)",
        "method": "wells with at least one image but fewer than NWavelengths",
    }


def f_bleaching() -> dict:
    import nd2
    import numpy as np

    drops = {}
    for staged, cid in BLEACH_RUNS:
        with nd2.ND2File(str(cpath(cid))) as f:
            arr = f.asarray()
        m = arr.reshape(arr.shape[0], -1).astype(np.float64).mean(axis=1)
        drops[staged] = 100 * (m[0] - m[-1]) / m[0]
    ranked = sorted(drops, key=drops.get, reverse=True)
    if drops[ranked[0]] - drops[ranked[1]] < 1.5:
        raise SystemExit(f"bleaching too close to rank: {drops}")
    return {
        "value": ranked[0],
        "drop_percent": drops,
        "reader": rd("nd2"),
        "method": "mean raw intensity of the first and last time point; (first - last) / first x 100",
    }


def f_zarr_expect() -> dict:
    """Sizes and calibration of the two z-stacks from the committed nd2 oracle (nd2 reader)."""
    out = {}
    for staged, cid in ZARR_STACKS:
        o = oracle_json.load(ROOT / "corpus" / "oracle" / f"{cid}.json")
        im = o["images"][0]
        ps = im["physical_size_um"]
        out[staged] = {
            "SizeX": im["size_x"],
            "SizeY": im["size_y"],
            "SizeZ": im["size_z"],
            "SizeC": im["size_c"],
            "SizeT": im["size_t"],
            "PhysicalSizeX": ps["x"],
            "PhysicalSizeY": ps["y"],
            "PhysicalSizeZ": ps["z"],
        }
    return {
        "value": out["stack_b"]["PhysicalSizeZ"],
        "stacks": out,
        "reader": "corpus/oracle (nd2)",
        "method": "physical pixel sizes and dimensions recorded in the ND2 metadata",
    }


def _rdml_reactions() -> dict:
    import xml.etree.ElementTree as ET
    import zipfile

    p = cpath(RDML_CID)
    raw = p.read_bytes()
    if raw[:2] == b"PK":
        z = zipfile.ZipFile(p)
        raw = z.read(next(n for n in z.namelist() if n.endswith(".xml")))
    root = ET.fromstring(raw)
    ns = root.tag.split("}")[0] + "}"
    per: dict[tuple[str, str], list[tuple[float, bool]]] = {}
    for run in root.iter(ns + "run"):
        for r in run.findall(ns + "react"):
            s = r.find(ns + "sample").get("id")
            for d in r.findall(ns + "data"):
                cq = d.findtext(ns + "cq")
                per.setdefault((s, d.find(ns + "tar").get("id")), []).append(
                    (float(cq) if cq else float("nan"), bool(d.findtext(ns + "excl")))
                )
    return per


def f_qpcr_rdml() -> dict:
    per = _rdml_reactions()

    def fold(keep_excluded: bool, pooled: bool) -> float:
        def cqs(s, t):
            return [c for c, e in per[(s, t)] if math.isfinite(c) and (keep_excluded or not e)]

        def dcq(samples):
            if pooled:
                tg = statistics.mean(c for s in samples for c in cqs(s, RDML_TARGET))
                ref = statistics.mean(statistics.mean(c for s in samples for c in cqs(s, r)) for r in RDML_REFS)
                return tg - ref
            return statistics.mean(
                statistics.mean(cqs(s, RDML_TARGET)) - statistics.mean(statistics.mean(cqs(s, r)) for r in RDML_REFS)
                for s in samples
            )

        return 2 ** -(dcq(RDML_TREATED) - dcq(RDML_CONTROL))

    v = fold(False, False)
    variants = {"pooled_reactions": fold(False, True), "excluded_kept": fold(True, False)}
    # The prompt fixes the averaging order (technical means first): pooling every reaction of a
    # group is a different, unbalanced estimate once a reaction is excluded, recorded for reference.
    if abs(variants["excluded_kept"] - v) / v < 0.06:
        raise SystemExit(f"RDML fold change variants do not separate: {v} {variants}")
    n_excl = sum(e for rs in per.values() for _, e in rs)
    return {
        "value": v,
        "variants": variants,
        "excluded_reactions": n_excl,
        "reader": "xml.etree on the RDML (LinRegPCR Cq values and <excl> flags)",
        "method": "ΔCq = Cq(HAC1u) - mean(Cq ACT1, Cq TFC1) per biological sample (technical means); "
        "ΔΔCq = mean ΔCq treated - mean ΔCq control; fold = 2^-ΔΔCq; excluded reactions dropped",
    }


def f_qpcr_eds() -> dict:
    sys.path.insert(0, str(HERE))
    import qpcr_facts as q

    rows = q._result_lines(q._zip_text(cpath(EDS_CID), "apldbio/sds/analysis_result.txt"))

    def mean_cq(sample: str, target: str) -> float:
        vals = [
            float(r["ct"])
            for r in rows
            if r["sample name"] == sample and r["detector"] == target and re.fullmatch(r"[\d.]+", r["ct"])
        ]
        return statistics.mean(vals)

    d = {s: mean_cq(s, EDS_TARGET) - mean_cq(s, EDS_REF) for s in (EDS_CONTROL, EDS_TREATED)}
    ddcq = d[EDS_TREATED] - d[EDS_CONTROL]
    return {
        "value": 2**-ddcq,
        "delta_cq": d,
        "ddcq": ddcq,
        "reader": "zipfile + analysis_result.txt (vendor Ct)",
        "method": "ΔCq = mean Ct(ABHD17C) - mean Ct(18s) per sample; fold = 2^-(ΔCq treated - ΔCq control)",
    }


def f_bitumen() -> dict:
    import numpy as np

    sys.path.insert(0, str(HERE))
    import spectroscopy as sp

    def band(x, y, lo, hi, baseline):
        o = np.argsort(x)
        x, y = x[o], y[o]
        m = (x >= lo) & (x <= hi)
        xs, ys = x[m], y[m]
        if baseline:
            ys = ys - np.interp(xs, [xs[0], xs[-1]], [ys[0], ys[-1]])
        return float(np.trapezoid(ys, xs))

    out = {}
    for cid, staged in BITUMEN:
        x, y, _ = sp.opus_block(cid)
        x, y = np.asarray(x, float), np.asarray(y, float)
        out[staged] = {
            "index_baseline": band(x, y, *SULFOXIDE, True) / band(x, y, *ALIPHATIC, True),
            "index_raw": band(x, y, *SULFOXIDE, False) / band(x, y, *ALIPHATIC, False),
        }
    tops = {k: max(out, key=lambda s: out[s][k]) for k in ("index_baseline", "index_raw")}
    if len(set(tops.values())) != 1:
        raise SystemExit(f"the sulfoxide ranking depends on the baseline: {out}")
    return {
        "value": tops["index_raw"].rsplit(".", 1)[0],
        "indices": out,
        "reader": rd("brukeropus"),
        "method": "trapezoid area 980-1060 over 1350-1525 cm-1 (with and without a local linear baseline)",
    }


def f_damaged() -> dict:
    plan = damaged_plan()
    bad = sorted(s for cid, s in plan if cid in DAMAGED_CORRUPT or cid in DAMAGED_TRUNCATE)
    for cid in DAMAGED_CORRUPT:
        assert entry(cid, "corrupt")
    return {
        "value": bad,
        "reader": "corpus/manifest.toml (role = corrupt) + harness truncation",
        "method": f"files the manifest records as corrupt, and the copies truncated to {TRUNCATE_FRACTION:.0%}",
    }


def f_reconcile() -> dict:
    import flowio

    names = {}
    for cid in RECONCILE_FCS:
        fd = flowio.FlowData(str(cpath(cid)), ignore_offset_error=True, only_text=True)
        text = {k.lower().lstrip("$"): v for k, v in fd.text.items()}
        names[cid] = text.get("smno") or text.get("tube name")
    assert all(names.values()) and not set(RECONCILE_MISSING) & set(names.values()), names
    return {
        "value": RECONCILE_MISSING,
        "recorded_names": names,
        "reader": rd("flowio") + " (TEXT $SMNO, TUBE NAME)",
        "method": "sheet names with no FCS file recording that sample name",
    }


def f_gcfid() -> dict:
    sys.path.insert(0, str(HERE))
    import quant

    areas = {}
    for n in GCFID:
        peaks = quant.report_peaks(f"chromhandler-001f{n}-d", "FID1 A")
        near = min(peaks, key=lambda p: abs(p["rt_min"] - 2.82))
        assert abs(near["rt_min"] - 2.82) < 0.05
        areas[n] = near["area"]
    conc = {n: GCFID_STD_PERCENT * areas[n] / areas[GCFID[0]] for n in GCFID[1:]}
    return {
        "value": statistics.mean(conc.values()),
        "areas": areas,
        "concentration_percent": conc,
        "reader": "ChemStation Report.TXT (vendor integration)",
        "method": "single-point external standard: 5.00 % x area / area of the standard; mean of 3 samples",
    }


def nmr_regions() -> list[tuple[int, float, float, float]]:
    rows = []
    for line in cpath(NMR_CID, sub="pdata/1/integrals.txt").read_text(errors="replace").splitlines():
        m = re.match(r"\s*(\d+)\s+([-\d.]+)\s+([-\d.]+)\s+([-\d.eE+]+)\s*$", line)
        if m:
            rows.append((int(m.group(1)), float(m.group(2)), float(m.group(3)), float(m.group(4))))
    return rows


def f_nmr() -> dict:
    rows = nmr_regions()
    ref = rows[0][3]
    total = sum(2 * r[3] / ref for r in rows[1:])
    return {
        "value": total,
        "regions": rows,
        "reader": "TopSpin pdata/1/integrals.txt (vendor-computed integrals)",
        "method": "sum of regions 2-5, scaled so that region 1 = 2 H",
    }


FACT_FUNCS = {
    "gcms_reaction": f_gcms_reaction,
    "hplc_is_ratio": f_hplc_is_ratio,
    "lcms_carryover": f_lcms_carryover,
    "flow_ics": f_flow_ics,
    "ephys_rheobase": f_ephys_rheobase,
    "plate_ic50": f_plate_ic50,
    "plate_screen": f_plate_screen,
    "hcs_dapi": f_hcs_dapi,
    "hcs_incomplete": f_hcs_incomplete,
    "bleaching": f_bleaching,
    "zarr_export": f_zarr_expect,
    "qpcr_rdml": f_qpcr_rdml,
    "qpcr_eds": f_qpcr_eds,
    "bitumen": f_bitumen,
    "damaged": f_damaged,
    "reconcile": f_reconcile,
    "gcfid": f_gcfid,
    "nmr": f_nmr,
}


def tidy(v: Any) -> Any:
    if isinstance(v, bool | str) or v is None:
        return v
    if isinstance(v, int):
        return v
    if isinstance(v, float):
        return float(f"{v:.8g}")
    if isinstance(v, dict):
        return {str(k): tidy(x) for k, x in v.items()}
    if isinstance(v, list | tuple):
        return [tidy(x) for x in v]
    if hasattr(v, "item"):
        return tidy(v.item())
    raise TypeError(f"cannot store {type(v)}")


def compute(only: list[str] | None = None) -> dict:
    out = {}
    for k, fn in FACT_FUNCS.items():
        if only and k not in only:
            continue
        out[k] = tidy(fn())
    return out


# ================================================================ questions (stdlib only)


def share_ref(cid: str, stage_as: str, role: str = "input", sub: str = "") -> dict:
    e = entry(cid, role)
    path = e["filename"].rstrip("/") + (f"/{sub}" if sub else "")
    return {"corpus_id": cid, "path": path, "stage_as": f"{DIR}/{stage_as}"}


def damaged_plan() -> list[tuple[str, str]]:
    ids = sorted(DAMAGED_INTACT + DAMAGED_CORRUPT + DAMAGED_TRUNCATE)
    folders = ["instrument_pc_1", "instrument_pc_2", "instrument_pc_3"]
    out = []
    for i, cid in enumerate(ids):
        e = entry(cid, "corrupt" if cid in DAMAGED_CORRUPT else "input")
        name = e["filename"].rsplit("/", 1)[-1]
        ext = name[name.index(".") :] if "." in name else ""
        out.append((cid, f"{DIR}/{folders[i % 3]}/file_{i + 1:02d}{ext}"))
    return out


def number(value: float, unit: str | None = None, rel: float | None = None, abs_: float | None = None) -> dict:
    tol = {}
    if rel is not None:
        tol["rel"] = rel
    if abs_ is not None:
        tol["abs"] = abs_
    return {"type": "number", "value": value, "unit": unit, "tolerance": tol}


def names(values: list[str], reject: list[str] | None = None, prefix: bool = True) -> dict:
    out = {
        "type": "list",
        "value": [v.split("/", 1)[1] if prefix and v.startswith(DIR + "/") else v for v in values],
        "accept": [sorted({v, v.split("/", 1)[1]}) if v.startswith(DIR + "/") else [v] for v in values],
        "ordered": False,
    }
    if reject:
        out["reject"] = sorted(reject)
    return out


def sheet(rows: list[list[Any]]) -> str:
    return "\n".join(",".join(str(c) for c in r) for r in rows) + "\n"


def scenario(qid, family, text, hint, answer, share_, generated, source, key, **extra) -> dict:
    q = {
        "id": qid,
        "family": family,
        "format": "share",
        "category": "scenario",
        "file": {"corpus_id": "share", "path": "", "stage_as": DIR},
        "share": share_,
        "generated": [{"stage_as": f"{DIR}/{n}", "content": c} for n, c in generated],
        "question": text,
        "answer_hint": hint,
        "answer": answer,
        "source": f"evals/facts/scenario.json [{key}] (evals/scenario_facts.py) — {source}",
    }
    q.update(extra)
    return q


def build_all() -> list[dict]:
    f = json.loads(FACTS.read_text())
    qs = []

    # 1. GC-MS reaction monitoring
    fx = f["gcms_reaction"]
    share_ = [share_ref(gcms_cid(nn), f"RAU-R505-{nn}.D/data.ms") for nn, _ in GCMS_RUNS]
    qs.append(
        scenario(
            "scn-chrom-gcms-reaction-is-ratio",
            "chromatography",
            "We sampled a reaction at several time points and ran each aliquot on our GC-MS (the `.D` folders in "
            "`sample_data/`; `sample_data/samples.csv` gives the time point of each run). An internal standard was "
            f"added to each aliquot and elutes at about {GCMS_IS_RT:.2f} min; the product elutes at about {GCMS_PRODUCT_RT:.2f} min. "
            "Integrate both peaks in each run's total-ion chromatogram and compute the product / internal-standard "
            "peak-area ratio. At which time point does that ratio first exceed 1.0?",
            "the time point in hours",
            number(fx["value"], "h", abs_=0.01),
            share_,
            [("samples.csv", sheet([["data_file", "time_h"]] + [[f"RAU-R505-{nn}.D", h] for nn, h in GCMS_RUNS]))],
            "first time point with product/IS TIC area ratio > 1 (MSD ChemStation RESULTS.CSV of each run; the agent "
            "gets only data.ms; the t = 0 run has no results file: Aston TIC window integration) "
            + json.dumps({r["time_h"]: round(r["ratio"], 3) for r in fx["runs"]}),
            "gcms_reaction",
        )
    )

    # 2. HPLC internal-standard normalisation per genotype
    fx = f["hplc_is_ratio"]
    share_ = []
    for s, _, _ in HPLC_SAMPLES:
        for ch in (1, 2):
            share_.append(share_ref(hplc_cid(s, ch), f"{s}_PDA_ch{ch}.cdf"))
    qs.append(
        scenario(
            "scn-chrom-hplc-is-normalised-genotype",
            "chromatography",
            "These are HPLC-PDA runs of plant extracts exported from LabSolutions as AIA/ANDI files (two PDA "
            "channels per injection); `sample_data/samples.csv` gives each sample's genotype and stage. Use PDA channel 2. "
            "The analyte elutes at about 27.35 min and the internal standard at about 56.97 min (the later, larger peak "
            "of the pair at 56.9-57.0 min). For each sample divide the analyte peak area by the internal-standard peak "
            "area, then average these ratios per genotype. How many times higher is the MicroTom mean than the della "
            "mean?",
            "the ratio of the two genotype means (a number)",
            number(fx["value"], None, rel=0.05),
            share_,
            [("samples.csv", sheet([["sample", "genotype", "stage"]] + [list(r) for r in HPLC_SAMPLES]))],
            "LabSolutions peak tables inside the ANDI files (peak_area; vendor integration): per-sample analyte/IS, "
            f"genotype means, MicroTom / della = {fx['value']:.4g} ({fx['value_if_is_pair_merged']:.4g} if the IS pair "
            "is integrated as one peak)",
            "hplc_is_ratio",
        )
    )

    # 3. LC-MS carry-over
    fx = f["lcms_carryover"]
    share_ = [share_ref(cid, staged) for staged, cid, _ in LCMS_SEQUENCE]
    qs.append(
        scenario(
            "scn-ms-lcms-hippurate-carryover",
            "ms",
            "Our urine metabolomics batch (negative ESI, Thermo .raw files; the injection order and sample types are in "
            "`sample_data/sequence.csv`) needs a carry-over check before we report hippuric acid. Extract the ion "
            f"chromatogram of its [M-H]- ion, m/z {HIPPURATE_MZ:.4f} (± 10 ppm, summing the intensities in that window in "
            "each full scan) from every run and take "
            f"the highest point of the peak between {LCMS_WINDOW[0]} and {LCMS_WINDOW[1]} min. Express each blank's value "
            "as a percentage of the pooled QC's. What is the highest carry-over among the blanks, in percent?",
            "a percentage",
            # profile-point sums (this answer), the single most intense profile point and centroided
            # scans put the same carry-over 11-13 % apart: the tolerance accepts each convention
            number(fx["value"], None, rel=0.2),
            share_,
            [
                (
                    "sequence.csv",
                    sheet(
                        [["injection", "data_file", "sample_type"]]
                        + [[i + 1, s, r] for i, (s, _, r) in enumerate(LCMS_SEQUENCE)]
                    ),
                )
            ],
            f"pyteomics on the depositor's mzML of each run: XIC apex blank/QC x 100 = {json.dumps(fx['percent_of_qc'])}",
            "lcms_carryover",
        )
    )

    # 4. ICS background subtraction
    fx = f["flow_ics"]
    share_ = [share_ref(cid, fx["tubes"][stim]["sample"]) for cid, stim in ICS_TUBES]
    share_.append(share_ref("flowkit-8c-ics", "analysis.wsp", "analysis"))
    qs.append(
        scenario(
            "scn-flow-ics-ifng-background-subtracted",
            "flow",
            "Intracellular cytokine staining of one donor: three FCS tubes, our FlowJo workspace "
            "`sample_data/analysis.wsp` that gates them, and `sample_data/tubes.csv` saying which tube got which stimulation. "
            "Using the workspace's gates, take the percentage of IFNg+ cells among CD8+ T cells in each tube and subtract "
            "the unstimulated tube's percentage. Which peptide pool gives the larger background-subtracted response, "
            "and how large is it? Give the background-subtracted percentage.",
            "the background-subtracted percentage (percentage points)",
            number(fx["value"], None, abs_=0.1),
            share_,
            [
                (
                    "tubes.csv",
                    sheet([["fcs_file", "stimulation"]] + [[fx["tubes"][s]["sample"], s] for _, s in ICS_TUBES]),
                )
            ],
            "FlowKit gating of the workspace: % IFNg+ of CD8+ minus the unstimulated tube "
            + json.dumps({k: round(v, 3) for k, v in fx["background_subtracted"].items()}),
            "flow_ics",
        )
    )

    # 5. rheobase per cell type
    fx = f["ephys_rheobase"]
    share_ = [share_ref(cid, staged) for staged, cid, _ in EPHYS_CELLS]
    qs.append(
        scenario(
            "scn-ephys-pyramidal-rheobase",
            "ephys",
            "Whole-cell current-clamp recordings (ABF), one per cell, each a family of current steps; "
            "`sample_data/cells.csv` gives the cell type. For each cell the rheobase is the smallest positive current step "
            "(relative to the holding current) whose step window contains at least one action potential (an upward "
            "crossing of 0 mV). What is the mean rheobase of the pyramidal cells?",
            "the mean rheobase in pA",
            number(fx["value"], "pA", abs_=1.0),
            share_,
            [("cells.csv", sheet([["file", "cell_type"]] + [[s, k] for s, _, k in EPHYS_CELLS]))],
            "pyABF step epochs and samples, NumPy 0 mV crossings: " + json.dumps(fx["cells"]),
            "ephys_rheobase",
        )
    )

    # 6. IC50 of the least potent compound
    fx = f["plate_ic50"]
    pe = entry("skanit-luciferase")
    qs.append(
        scenario(
            "scn-plate-least-potent-ic50",
            "plates",
            "A cell-viability (luminescence) plate read on our Varioskan, exported from SkanIt, and the plate map "
            "`sample_data/plate_map.csv` (three compounds, each in two replicate rows, 1:3 dilutions from 10 µM; "
            "column 12 has no cells). Fit a four-parameter logistic curve (bottom, top, IC50, Hill slope; "
            "unweighted, on log10 concentration) to each compound's replicate wells. What is the IC50 of the least "
            "potent compound, in nM?",
            "the IC50 in nM",
            number(fx["value"], "nM", rel=0.15),
            [{"corpus_id": "skanit-luciferase", "path": pe["filename"], "stage_as": f"{DIR}/viability_plate.xlsx"}],
            [("plate_map.csv", ic50_plate_map())],
            f"allotropy well values + SciPy 4PL: {fx['compound']} "
            + json.dumps({k: round(v["ic50_nm"], 3) for k, v in fx["compounds"].items()})
            + f" nM; fits on replicate means / normalised values within {fx['method_spread']:.1%}",
            "plate_ic50",
        )
    )

    # 7. screening hits with a compound list
    fx = f["plate_screen"]
    pe = entry("gen5-lum-endpoint")
    non_hits = [c for c in fx["activity_percent"] if c not in fx["value"]]
    qs.append(
        scenario(
            "scn-plate-screen-hits",
            "plates",
            "Primary screen plate (Gen5 luminescence export; its layout marks the POSCON and NEGCON control wells and "
            "the samples SPL1-SPL80) and our compound list `sample_data/compounds.csv` mapping each sample to a compound. "
            "Normalise every sample well as percent activity: (signal - mean NEGCON) / (mean POSCON - mean NEGCON) x "
            "100. Which compounds are hits, i.e. below 10 % activity? Give the compound IDs.",
            "the compound IDs, comma-separated",
            names(fx["value"], reject=non_hits),
            [{"corpus_id": "gen5-lum-endpoint", "path": pe["filename"], "stage_as": f"{DIR}/screen_plate_07.txt"}],
            [("compounds.csv", screen_list())],
            "Gen5's own NormLum values stored under each well (recomputed from the reads and the control layout); "
            f"hits below 10 %; nearest values to the cut-off {fx['closest_to_cutoff']}",
            "plate_screen",
        )
    )

    # 8. HCS nuclear intensity change
    fx = f["hcs_dapi"]
    qs.append(
        scenario(
            "scn-hcs-dapi-change-vs-dmso",
            "hcs",
            "An Operetta plate exported from Harmony (only the wells we need were copied) and our plate map "
            "`sample_data/plate_map.csv`. By how much did the compound change the mean DAPI intensity relative to the "
            "DMSO well? Use the mean raw DAPI pixel value over every imaged field of each well, and give the change "
            "as a percentage of the DMSO value (negative if it went down).",
            "a percentage",
            number(fx["value"], None, abs_=1.0),
            [share_ref("hcs-harmony-idr0034-folder", "plate_2017_03")],
            [
                (
                    "plate_map.csv",
                    sheet([["well", "treatment", "dose"]] + [[w, t, d] for w, (t, d) in HCS_WELLS.items()]),
                )
            ],
            "tifffile means of the DAPI field planes named by Index.idx.xml: "
            + json.dumps({k: round(v, 3) for k, v in fx["well_means"].items()})
            + f", fields {json.dumps(fx['fields'])}",
            "hcs_dapi",
        )
    )

    # 9. HCS copy QA
    fx = f["hcs_incomplete"]
    have = list(fx["wells_with_images"])
    qs.append(
        scenario(
            "scn-hcs-wells-missing-channel",
            "hcs",
            "This is a copy of an ImageXpress plate folder a collaborator sent us. Before we analyse it: among the wells "
            "that have any image in this copy, which ones are missing a wavelength (channel) that the plate was set up "
            "to acquire? Give the well names.",
            "the well names, comma-separated",
            names(fx["value"], reject=[w for w in have if w not in fx["value"]]),
            [share_ref("hcs-imagexpress-idr0081-folder", "BSF018292-1A")],
            [],
            "HTD NWavelengths and the TIFF file names (standard library): " + json.dumps(fx["wells_with_images"]),
            "hcs_incomplete",
        )
    )

    # 10. photobleaching
    fx = f["bleaching"]
    share_ = [share_ref(cid, staged) for staged, cid in BLEACH_RUNS]
    others = [s for s, _ in BLEACH_RUNS if s != fx["value"]]
    qs.append(
        scenario(
            "scn-mic-nd2-strongest-bleaching",
            "microscopy",
            "Three repeats of a photobleaching test on our microscope (time-lapse ND2 files, 13 frames each). "
            "Which acquisition bleached the most, measured as the percentage drop of the mean raw intensity of the "
            "whole frame from the first to the last time point?",
            "the file name",
            {
                "type": "string",
                "value": fx["value"],
                "accept": [fx["value"], fx["value"].rsplit(".", 1)[0]],
                "reject": others + [o.rsplit(".", 1)[0] for o in others],
            },
            share_,
            [],
            "nd2 frame means: drop % " + json.dumps({k: round(v, 3) for k, v in fx["drop_percent"].items()}),
            "bleaching",
        )
    )

    # 11. OME-Zarr export of two z-stacks
    fx = f["zarr_export"]
    share_ = [share_ref(cid, f"{staged}.nd2") for staged, cid in ZARR_STACKS]
    outputs = []
    for staged, _ in ZARR_STACKS:
        e = fx["stacks"][staged]
        outputs.append(
            {
                "output": f"{staged}.ome.zarr",
                "expect": {
                    k: e[k] for k in ("SizeX", "SizeY", "SizeZ", "PhysicalSizeX", "PhysicalSizeY", "PhysicalSizeZ")
                },
            }
        )
    qs.append(
        scenario(
            "scn-mic-nd2-zstacks-to-ome-zarr",
            "microscopy",
            "Our image-analysis pipeline reads OME-Zarr. Convert the two ND2 z-stacks in `sample_data/` to OME-Zarr, "
            "written next to them as `sample_data/stack_a.ome.zarr` and `sample_data/stack_b.ome.zarr`, keeping the physical "
            "calibration (pixel size and z-step) in the OME-Zarr metadata. Then tell me: what is the z-step of stack_b, "
            "in µm?",
            "the z-step in µm",
            number(fx["value"], "µm", rel=0.01),
            share_,
            [],
            "nd2 metadata (corpus/oracle): " + json.dumps(fx["stacks"]),
            "zarr_export",
            task={"kind": "ome-zarr", "outputs": outputs},
        )
    )

    # 12. qPCR ΔΔCq with excluded reactions
    fx = f["qpcr_rdml"]
    groups = [[s, "control"] for s in RDML_CONTROL] + [[s, "treated"] for s in RDML_TREATED]
    qs.append(
        scenario(
            "scn-qpcr-rdml-hac1u-ddcq",
            "qpcr",
            "Our qPCR data for the UPR study, analysed in LinRegPCR and saved as RDML (reactions it flagged are marked "
            "excluded), plus `sample_data/groups.csv` naming the three control and three treated biological samples. "
            "Leave out excluded reactions. Normalise HAC1u to the mean Cq of the reference genes ACT1 and TFC1 (average "
            "the technical replicates of each biological sample first), and compute the fold change of treated versus "
            "control with the ΔΔCq method (efficiency 2). What is the fold change?",
            "the fold change (a number)",
            number(fx["value"], None, rel=0.035),
            [share_ref(RDML_CID, "upr_qpcr.rdml")],
            [("groups.csv", sheet([["sample", "group"], *groups]))],
            f"ElementTree on the RDML: fold {fx['value']:.4g}; pooled reactions {fx['variants']['pooled_reactions']:.4g};"
            f" with the excluded reaction kept {fx['variants']['excluded_kept']:.4g} (rejected by the tolerance)",
            "qpcr_rdml",
        )
    )

    # 13. qPCR ΔΔCq from an .eds
    fx = f["qpcr_eds"]
    qs.append(
        scenario(
            "scn-qpcr-eds-lenvatinib-oe",
            "qpcr",
            "7500 run of our ABHD17C experiment (`sample_data/abhd17c_run.eds`). In the ABHD17C-overexpressing cells, how "
            "did Lenvatinib change ABHD17C expression? Use 18s as the reference gene, the untreated overexpressing "
            "sample (`ABHD17C OE`) as the calibrator and `Lenvatinib/ABHD17C OE` as the treated sample, the Ct values "
            "the instrument software called, and the ΔΔCt method (efficiency 2). Give the fold change.",
            "the fold change (a number)",
            number(fx["value"], None, rel=0.03),
            [share_ref(EDS_CID, "abhd17c_run.eds")],
            [],
            f"vendor Ct values in analysis_result.txt: ΔΔCt {fx['ddcq']:.4g}",
            "qpcr_eds",
        )
    )

    # 14. FT-IR ageing index
    fx = f["bitumen"]
    share_ = [share_ref(cid, staged) for cid, staged in BITUMEN]
    qs.append(
        scenario(
            "scn-spec-bitumen-sulfoxide-index",
            "spectroscopy",
            "ATR-FTIR spectra (Bruker OPUS) of one bitumen binder unaged and after two laboratory ageing treatments; "
            "the file names say the treatment. Compute a sulfoxide index for each: the integrated absorbance of the "
            "S=O band (980-1060 cm-1) divided by the integrated absorbance of the aliphatic band (1350-1525 cm-1). "
            "Which sample has the highest sulfoxide index?",
            "the sample (file) name",
            {
                "type": "string",
                "value": fx["value"],
                "accept": [fx["value"], fx["value"] + ".2", "1 h at 180"],
                "reject": [s.rsplit(".", 1)[0] for _, s in BITUMEN if s.rsplit(".", 1)[0] != fx["value"]],
            },
            share_,
            [],
            "brukeropus absorbance blocks, NumPy trapezoid areas: " + json.dumps(fx["indices"]),
            "bitumen",
        )
    )

    # 15. share health
    fx = f["damaged"]
    plan = damaged_plan()
    share_ = []
    for cid, staged in plan:
        e = entry(cid, "corrupt" if cid in DAMAGED_CORRUPT else "input")
        share_.append({"corpus_id": cid, "path": e["filename"], "stage_as": staged})
    intact = [s for c, s in plan if c in DAMAGED_INTACT]
    qs.append(
        scenario(
            "scn-share-damaged-files",
            "share",
            "We copied these files off three instrument PCs onto the lab share, and some copies may be damaged "
            "(cut short or corrupt). Which files in `sample_data/` are truncated or corrupt? Give their paths.",
            "the paths of the damaged files, comma-separated",
            names(fx["value"], reject=intact + [s.split("/", 1)[1] for s in intact]),
            share_,
            [],
            "corpus/manifest.toml (role = corrupt) and the copies the harness truncates: " + json.dumps(fx["value"]),
            "damaged",
            prepare={"truncate": {s: TRUNCATE_FRACTION for c, s in plan if c in DAMAGED_TRUNCATE}},
        )
    )

    # 16. sample-sheet reconciliation
    fx = f["reconcile"]
    share_ = [share_ref(cid, f"acq_{i + 1:03d}.fcs") for i, cid in enumerate(RECONCILE_FCS)]
    present = list(fx["recorded_names"].values())
    expected = sorted(present + RECONCILE_MISSING, key=lambda s: s[::-1])
    qs.append(
        scenario(
            "scn-flow-sheet-missing-samples",
            "flow",
            "`sample_data/expected_samples.csv` is the list of samples that should have been acquired on the cytometers "
            "this week; the FCS files in `sample_data/` are what we got back (file names are just acquisition numbers). "
            "Match them by the sample name recorded inside each FCS file. Which samples on the list have no FCS file?",
            "only the missing sample names, comma-separated",
            names(fx["value"], reject=present, prefix=False),
            share_,
            [("expected_samples.csv", sheet([["sample_name"]] + [[n] for n in expected]))],
            "FlowIO TEXT ($SMNO, else TUBE NAME) of each file: " + json.dumps(fx["recorded_names"]),
            "reconcile",
        )
    )

    # 17. GC-FID single-point calibration
    fx = f["gcfid"]
    share_ = [share_ref(f"chromhandler-001f{n}-d-fid1a-ch", f"001F{n}.D/FID1A.ch", "part") for n in GCFID]
    rows = [["data_file", "injection", "type", "analyte_percent"]]
    rows += [
        [f"001F{n}.D", i + 1, "standard" if i == 0 else "sample", GCFID_STD_PERCENT if i == 0 else ""]
        for i, n in enumerate(GCFID)
    ]
    qs.append(
        scenario(
            "scn-chrom-gcfid-external-standard",
            "chromatography",
            "Four GC-FID injections from one sequence (`sample_data/*.D/FID1A.ch`); `sample_data/sequence.csv` says the first "
            f"is a calibration standard containing {GCFID_STD_PERCENT:.2f} % (v/v) of the analyte and the other three "
            "are samples. The analyte is the peak at about 2.82 min. Quantify it by single-point external standard "
            "(concentration proportional to FID peak area). What is the mean analyte concentration of the three "
            "samples, in % (v/v)?",
            "a percentage",
            number(fx["value"], None, rel=0.05),
            share_,
            [("sequence.csv", sheet(rows))],
            "ChemStation Report.TXT areas of each injection (vendor integration; the agent gets only FID1A.ch): "
            + json.dumps(fx["areas"]),
            "gcfid",
        )
    )

    # 18. NMR integration list
    fx = f["nmr"]
    share_ = [share_ref(NMR_CID, f"NF20190708/50/{n}", sub=n) for n in ("acqu", "acqus", "fid", "pulseprogram")]
    share_ += [share_ref("nmrxiv-s275-1", f"sucrose_std/1/{n}", sub=n) for n in ("acqu", "acqus", "fid", "uxnmr.par")]
    share_ += [share_ref("nmrxiv-s275-13", f"sucrose_std/13/{n}", sub=n) for n in ("acqu", "acqus", "fid", "uxnmr.par")]
    regions = [["region", "from_ppm", "to_ppm"]] + [[r[0], r[1], r[2]] for r in fx["regions"]]
    qs.append(
        scenario(
            "scn-nmr-aromatic-proton-count",
            "nmr",
            "Raw Bruker NMR data from our spectrometer account: two samples (`NF20190708` is our dibrominated product, "
            "`sucrose_std` the instrument's standard tube), FIDs only. Process the product's 1H FID (Fourier transform, "
            "phase, reference) and integrate the regions in `sample_data/regions.csv`. Setting region 1 to 2 H, how many "
            "protons do regions 2-5 add up to?",
            "the number of protons (a number)",
            number(fx["value"], None, rel=0.03),
            share_,
            [("regions.csv", sheet(regions))],
            "TopSpin pdata/1/integrals.txt of the same acquisition (vendor-processed; the agent gets only the FID): "
            + json.dumps([r[3] for r in fx["regions"]]),
            "nmr",
        )
    )
    return qs


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true", help="exit 1 if evals/facts/scenario.json is out of date")
    ap.add_argument("--only", nargs="*", help="recompute only these facts (keys of FACT_FUNCS)")
    a = ap.parse_args()
    if a.check:
        new = compute()
        old = json.loads(FACTS.read_text())
        same = json.dumps(old, sort_keys=True) == json.dumps(new, sort_keys=True)
        print("evals/facts/scenario.json is up to date" if same else "evals/facts/scenario.json differs from the files")
        return 0 if same else 1
    old = json.loads(FACTS.read_text()) if FACTS.exists() else {}
    old.update(compute(a.only))
    FACTS.write_text(json.dumps({k: old[k] for k in FACT_FUNCS if k in old}, indent=1, ensure_ascii=False) + "\n")
    print(f"wrote {FACTS}")
    for q in build_all():
        print(f"{q['id']}: {q['answer']['value']!r}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
