"""Analysis-tier plate-analysis questions (the plate-reader assays of `openreadout analyze`): standard-curve concentrations,
IC50, Z′, kinetic rates and doubling times, the way a scientist asks them.

Answers are computed here with SciPy (BSD-3), NumPy, openpyxl (MIT) and plain-Python parsing of the
exports, through the helpers in `oracle/assay.py` — never with OpenReadout. Each question states
the fitting convention (model, weighting, blank handling, window), and its tolerance accepts the
spread between correct implementations (SciPy vs R drc vs the vendor's own result, recorded in
`docs/provenance/plate-analysis.md`). `analysis.py` appends `facts(Fact)` to its facts and
`specs()` to its question specs. Like `analysis.py`, this module imports only the standard library
and `generate` at the top; the numeric libraries are imported inside the functions.
"""

from __future__ import annotations

import math
import sys
from pathlib import Path
from typing import Any

import generate as g

ROOT = Path(__file__).resolve().parent.parent


def _oa():
    sys.path.insert(0, str(ROOT / "oracle"))
    import assay as oa  # oracle/assay.py (SciPy, openpyxl; no OpenReadout)

    return oa


def _src(oa, what: str) -> str:
    import numpy
    import scipy

    return f"{what} (scipy {scipy.__version__}, numpy {numpy.__version__})"


# ------------------------------------------------------------------ SkanIt ELISA (4PL)
def skanit_conc(well: str):
    oa = _oa()
    s = oa.skanit_fit()
    return oa.inv4(s["raw"][well] - s["blank"], *s["p"]), _src(oa, "scipy curve_fit 4PL")


def skanit_final(sample: str):
    oa = _oa()
    s = oa.skanit_fit()
    wells = [w for w, n in s["names"].items() if n == sample]
    vals = [oa.inv4(s["raw"][w] - s["blank"], *s["p"]) * s["dils"][w] for w in wells]
    return sum(vals) / len(vals), _src(oa, "scipy curve_fit 4PL")


# ------------------------------------------------------------------ Gen5 (linear, 4PL on Mean V)
def gen5_linear_conc(well: str):
    import numpy as np

    oa = _oa()
    layout, results, _ = oa.gen5_text("gen5-abs-stdcurve-linear")
    ids, cd = layout["Well ID"], layout["Conc/Dil"]
    raw = {w: float(v) for w, v in results["630nmAbsRead:630"].items()}
    blank = np.mean([raw[w] for w, i in ids.items() if i == "BLK"])
    std = [w for w, i in ids.items() if i.startswith("STD")]
    slope, icpt = np.polyfit([float(cd[w]) for w in std], [raw[w] - blank for w in std], 1)
    return float((raw[well] - blank - icpt) / slope), f"numpy {np.__version__} polyfit (plain-Python Gen5 text read)"


def gen5_meanv_ec50():
    import numpy as np

    oa = _oa()
    layout, results, _ = oa.gen5_text("gen5-abs-kinetic-meanv-4pl")
    ids, cd = layout["Well ID"], layout["Conc/Dil"]
    mv = {w: float(v) for w, v in results["Mean V [420]"].items()}
    std = [w for w, i in ids.items() if i.startswith("STD")]
    p, *_ = oa.fit(
        oa.four_pl, np.array([float(cd[w]) for w in std]), np.array([mv[w] for w in std]), [1.0, 3.0, 15.0, 35.0]
    )
    return float(p[2]), _src(oa, "scipy curve_fit 4PL on the Mean V results")


def gen5_max_rate(well: str, h: int = 5):
    """Steepest least-squares slope over `h` consecutive reads, mOD/min."""
    import numpy as np

    oa = _oa()
    text = oa.manifest_file("gen5-abs-kinetic-meanv-4pl").read_bytes().decode("latin-1").splitlines()
    i = next(k for k, ln in enumerate(text) if ln.startswith("Time\tT"))
    head = text[i].split("\t")
    col = head.index(well)
    t, y = [], []
    for ln in text[i + 1 :]:
        if not ln.strip():
            break
        cells = ln.split("\t")
        hh, mm, ss = (int(v) for v in cells[0].split(":"))
        t.append(hh * 60 + mm + ss / 60)
        y.append(float(cells[col]))
    t, y = np.array(t), np.array(y)
    best = max(np.polyfit(t[s : s + h], y[s : s + h], 1)[0] for s in range(len(t) - h + 1))
    return float(best * 1000), f"numpy {np.__version__} polyfit over every 5-read window (plain-Python Gen5 text read)"


# ------------------------------------------------------------------ synthetic dose-response
def _dose(compound: str):
    import numpy as np

    oa = _oa()
    import csv

    vals = oa.matrix(oa.FIXTURES / "assay-synth-dose-response.csv")
    lay = list(
        csv.DictReader((oa.FIXTURES / "assay-synth-dose-response-layout.csv").read_text(encoding="utf-8").splitlines())
    )
    return oa, vals, lay, np


def dose_ic50(compound: str):
    oa, vals, lay, np = _dose(compound)
    pts = [(float(r["concentration"]), vals[r["well"]]) for r in lay if r["compound"] == compound]
    x = np.array([p[0] for p in pts])
    y = np.array([p[1] for p in pts])
    p, *_ = oa.fit(oa.four_pl, x, y, [y[x == x.min()].mean(), 1.0, 0.5, y[x == x.max()].mean()])
    return float(p[2]), _src(oa, "scipy curve_fit 4PL on raw RLU")


def dose_zprime():
    _oa, vals, lay, np = _dose("")
    pos = np.array([vals[r["well"]] for r in lay if r["role"] == "positive"])
    neg = np.array([vals[r["well"]] for r in lay if r["role"] == "negative"])
    return float(
        1 - 3 * (pos.std(ddof=1) + neg.std(ddof=1)) / abs(pos.mean() - neg.mean())
    ), f"numpy {np.__version__} (Zhang 1999)"


# ------------------------------------------------------------------ synthetic 5PL ELISA
def elisa_final(sample: str):
    import numpy as np

    oa = _oa()
    vals = oa.matrix(oa.FIXTURES / "assay-synth-elisa-5pl.csv")
    lay = oa.grid_layout(oa.FIXTURES / "assay-synth-elisa-5pl-layout.csv")
    role, names = lay["role"], lay["sample"]
    conc = {w: float(v) for w, v in lay["concentration (pg/mL)"].items()}
    dil = {w: float(v.split(":")[1]) for w, v in lay["dilution"].items()}
    blank = np.mean([vals[w] for w, r in role.items() if r == "blank"])
    std = [w for w, r in role.items() if r == "standard"]
    y = np.array([vals[w] - blank for w in std])
    p, *_ = oa.fit(oa.five_pl, np.array([conc[w] for w in std]), y, [0.0, 1.4, 180.0, 3.2, 0.6], sigma=np.abs(y))
    wells = [w for w, n in names.items() if n == sample]
    out = [oa.inv5(vals[w] - blank, *p) * dil.get(w, 1.0) for w in wells]
    return float(np.mean(out)), _src(oa, "scipy curve_fit 5PL, sigma = |y|")


# ------------------------------------------------------------------ Tecan growth
def tecan_doubling_min(well: str):
    import numpy as np
    import openpyxl
    from scipy.optimize import curve_fit

    oa = _oa()
    wb = openpyxl.load_workbook(oa.manifest_file("tecan-icontrol-kinetic-xlsx"), data_only=True, read_only=True)
    rows = [list(r) for r in wb.worksheets[0].iter_rows(values_only=True)]
    i = next(k for k, r in enumerate(rows) if r and r[0] == "Time [s]")
    t = np.array([float(v) for v in rows[i][1:] if v is not None])
    r = next(r for r in rows[i + 2 :] if r and r[0] == well)
    y = np.array([float(v) for v in r[1 : 1 + len(t)]])
    y = y - y.min()
    th = (t - t[0]) / 3600
    p, _ = curve_fit(
        oa.logistic,
        th,
        y,
        p0=[y.max(), max(y[0], y.max() * 1e-3), 4 / th[-1]],
        bounds=([1e-12] * 3, [np.inf] * 3),
        method="trf",
        x_scale="jac",
        xtol=1e-15,
        ftol=1e-15,
        gtol=1e-15,
        max_nfev=200000,
    )
    return float(math.log(2) / p[2] * 60), _src(
        oa, "scipy curve_fit logistic (growthcurver's model, bg = well minimum)"
    )


def facts(Fact: Any) -> list:
    """Fact records for analysis.py (`Fact(corpus_id, name, how, compute)`)."""
    return [
        Fact(
            "skanit-elisa-steps",
            "d5_conc",
            "raw 450 nm absorbance minus the mean of the Blank1 wells (H1, H2); unweighted 4PL fitted to the 14 standard wells (Std0001–Std0007, concentrations from the Layout definitions sheet); D5 back-calculated (SkanIt itself reports 0.08038)",
            lambda: skanit_conc("D5"),
        ),
        Fact(
            "skanit-elisa-steps",
            "un0023_final",
            "as d5_conc; the back-calculated concentrations of Un0023's wells (D3, D4) × their dilution 250, averaged (SkanIt: 64.0 and 67.8)",
            lambda: skanit_final("Un0023"),
        ),
        Fact(
            "gen5-abs-stdcurve-linear",
            "e7_conc",
            "630 nm absorbance minus the mean of the BLK wells; least-squares line through the 21 STD wells (Conc/Dil concentrations); E7 back-calculated (Gen5 reports 1628.811)",
            lambda: gen5_linear_conc("E7"),
        ),
        Fact(
            "gen5-abs-kinetic-meanv-4pl",
            "meanv_ec50",
            "unweighted 4PL fitted to the Mean V results of the 24 STD wells; the C (EC50) parameter (Gen5 reports 14.8)",
            gen5_meanv_ec50,
        ),
        Fact(
            "gen5-abs-kinetic-meanv-4pl",
            "b3_max_rate",
            "well B3 of the 420 nm kinetic read: largest least-squares slope over any 5 consecutive exported reads, in mOD/min",
            lambda: gen5_max_rate("B3"),
        ),
        Fact(
            "assay-synth-dose-response",
            "cpd_b_ic50",
            "unweighted 4PL on the raw RLU of CPD-B's 30 wells (rows D–F, columns 1–10); the inflection concentration (µM)",
            lambda: dose_ic50("CPD-B"),
        ),
        Fact(
            "assay-synth-dose-response",
            "z_prime",
            "Z′ = 1 − 3 (SDpos + SDneg)/|mean pos − mean neg| from the 8 positive (column 12) and 8 negative (column 11) control wells, sample SD",
            dose_zprime,
        ),
        Fact(
            "assay-synth-elisa-5pl",
            "s04_final",
            "OD minus the mean of the blank wells; 5PL fitted to the 14 standard wells weighted 1/y²; S04's two wells back-calculated × dilution 5, averaged (pg/mL)",
            lambda: elisa_final("S04"),
        ),
        Fact(
            "tecan-icontrol-kinetic-xlsx",
            "a2_doubling_min",
            "kinetic Abs600 read, well A2 minus its own minimum; least-squares logistic N(t) = K/(1 + ((K − N0)/N0)e^(−rt)) (growthcurver's model); ln 2 / r in minutes (growthcurver's t_gen agrees to 1e-5)",
            lambda: tecan_doubling_min("A2"),
        ),
    ]


def specs() -> list[g.Spec]:
    layout_extra = [("assay-synth-dose-response-layout", "sample_layout.csv")]
    return [
        g.Spec(
            "ana-plate-skanit-elisa-conc",
            "skanit-elisa-steps",
            "analysis",
            "This ELISA plate report holds the raw 450 nm absorbances and a plate layout: standards in columns 1–2 "
            "(concentrations in µg/mL in the layout definitions) and blank wells. Subtract the mean of the blank wells, "
            "fit an unweighted four-parameter logistic curve to the individual standard wells, and report the "
            "concentration of the sample in well D5 (µg/mL, before any dilution correction).",
            lambda f, m: g.number(f["d5_conc"], None, rel=0.01),
            "facts-analysis: d5_conc (scipy 4PL; SkanIt's own value within 0.1 %)",
            answer_hint="the concentration in µg/mL",
        ),
        g.Spec(
            "ana-plate-skanit-elisa-dilution",
            "skanit-elisa-steps",
            "analysis",
            "Same ELISA plate: after subtracting the mean of the blank wells and fitting an unweighted four-parameter "
            "logistic curve to the standard wells, what is the concentration of sample Un0023 in the undiluted "
            "sample, i.e. the mean over its wells of the back-calculated concentration times the dilution in the layout "
            "(µg/mL)?",
            lambda f, m: g.number(f["un0023_final"], None, rel=0.01),
            "facts-analysis: un0023_final (scipy 4PL; SkanIt's own Dilution Factor step within 0.1 %)",
            answer_hint="the concentration in µg/mL",
        ),
        g.Spec(
            "ana-plate-gen5-linear-conc",
            "gen5-abs-stdcurve-linear",
            "analysis",
            "This protein-assay plate export defines standards (STD1–STD7 with their concentrations) and blank wells "
            "(BLK) in its layout. Subtract the mean of the blank wells from the 630 nm absorbances, fit a straight line "
            "to the standard wells, and report the concentration of the sample in well E7.",
            lambda f, m: g.number(f["e7_conc"], None, rel=0.01),
            "facts-analysis: e7_conc (numpy polyfit; Gen5's own [Concentration] within 0.1 %)",
            answer_hint="the concentration, in the layout's units",
        ),
        g.Spec(
            "ana-plate-gen5-meanv-ec50",
            "gen5-abs-kinetic-meanv-4pl",
            "analysis",
            "This kinetic enzyme-assay export includes the reader software's Mean V (mean reaction rate) result for "
            "every well, and a layout of standards STD1–STD12 with their concentrations. Fit an unweighted "
            "four-parameter logistic curve to the standards' Mean V values against concentration. What is the "
            "curve's EC50 (the concentration at its inflection point)?",
            lambda f, m: g.number(f["meanv_ec50"], None, rel=0.01),
            "facts-analysis: meanv_ec50 (scipy 4PL; the file's own fit reports 14.8)",
            answer_hint="the concentration, in the layout's units",
        ),
        g.Spec(
            "ana-plate-gen5-max-rate",
            "gen5-abs-kinetic-meanv-4pl",
            "analysis",
            "In this kinetic 420 nm read, what is the maximum rate in well B3, taken as the steepest least-squares "
            "slope over any 5 consecutive exported time points? Give it in mOD per minute.",
            lambda f, m: g.number(f["b3_max_rate"], None, rel=0.01),
            "facts-analysis: b3_max_rate (numpy)",
            answer_hint="the rate in mOD/min",
        ),
        g.Spec(
            "ana-plate-dose-response-ic50",
            "assay-synth-dose-response",
            "analysis",
            "`sample.csv` is a luminescence plate and `sample_layout.csv` its layout (well, role, compound, "
            "concentration in µM). Fit an unweighted four-parameter logistic curve to the raw signal of all of "
            "compound CPD-B's wells against concentration. What is CPD-B's IC50 in µM?",
            lambda f, m: g.number(f["cpd_b_ic50"], None, rel=0.01),
            "facts-analysis: cpd_b_ic50 (scipy 4PL; R drc agrees to 1e-4)",
            extra=layout_extra,
            answer_hint="the IC50 in µM",
        ),
        g.Spec(
            "ana-plate-dose-response-zprime",
            "assay-synth-dose-response",
            "analysis",
            "`sample.csv` is a screening plate and `sample_layout.csv` its layout, with positive and negative control "
            "wells. What is the plate's Z′-factor (Z′ = 1 − 3(SDpos + SDneg)/|mean pos − mean neg|, sample SDs)?",
            lambda f, m: g.number(f["z_prime"], None, abs_=0.005),
            "facts-analysis: z_prime (numpy)",
            extra=layout_extra,
            answer_hint="Z′, a number below 1",
        ),
        g.Spec(
            "ana-plate-elisa-5pl-dilution",
            "assay-synth-elisa-5pl",
            "analysis",
            "`sample.csv` is an ELISA plate (OD450) and `sample_layout.csv` its plate map (role, sample, concentration "
            "in pg/mL, dilution). Subtract the mean of the blank wells, fit a five-parameter logistic curve to the "
            "standard wells with 1/y² weighting, and report sample S04's concentration corrected for its dilution "
            "(mean of its wells, pg/mL).",
            lambda f, m: g.number(f["s04_final"], None, rel=0.01),
            "facts-analysis: s04_final (scipy 5PL; R drc agrees to 3e-6)",
            extra=[("assay-synth-elisa-5pl-layout", "sample_layout.csv")],
            answer_hint="the concentration in pg/mL",
        ),
        g.Spec(
            "ana-plate-tecan-doubling-time",
            "tecan-icontrol-kinetic-xlsx",
            "analysis",
            "This plate reader export has a kinetic OD600 growth read. For well A2, subtract the well's minimum OD "
            "and fit the logistic growth model N(t) = K / (1 + ((K − N0)/N0)·e^(−r·t)) (as the R package growthcurver "
            "does). What is the doubling time ln 2 / r, in minutes?",
            lambda f, m: g.number(f["a2_doubling_min"], "min", rel=0.01),
            "facts-analysis: a2_doubling_min (scipy logistic; growthcurver t_gen agrees)",
            answer_hint="the doubling time in minutes",
        ),
    ]
