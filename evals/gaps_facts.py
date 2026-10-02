"""Questions for three routes the scenario tier found missing: integrating a band or region of a
spectrum on its own axis (IR/Raman cm-1, UV-Vis nm), plate layouts whose control wells carry the
vendor's own role names (Gen5 POSCON/NEGCON), and a folder of spectra with the lab's sample sheet
and README in it.

Every answer is computed here with third-party readers or taken from vendor-computed values,
never with OpenReadout:

* spectra are read with brukeropus (MIT, OPUS), SpectroChemPy (CeCILL-B, OMNIC; the band area is
  cross-checked on OMNIC's own CSV export of the same spectrum), renishawWiRE (MIT, WiRE) and
  jcamp (MIT, JCAMP-DX), and integrated with NumPy (`trapezoid` over the samples inside the
  window); each area is computed both on the samples inside the window and with the signal
  interpolated at the window's ends, and the tolerance covers both;
* the Gen5 values come from the export itself: Gen5's own `NormLum` (percent of the
  POSCON-NEGCON window, vendor-computed and stored under each well) and, for Z', the raw LUM
  reads and the Gen5 layout with plain arithmetic (sample and population standard deviations).

The values go to `evals/facts/gaps.json` (committed; `facts-gaps:` sources).

    oracle/.venv/bin/python evals/gaps_facts.py          # recompute evals/facts/gaps.json
    oracle/.venv/bin/python evals/gaps_facts.py --check  # exit 1 if the committed facts differ
"""

from __future__ import annotations

import argparse
import contextlib
import io
import json
import statistics
import sys
import warnings
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
import analysis as a
import generate as g

OUT = Path(__file__).resolve().parent / "facts" / "gaps.json"
DIR = "sample_batch"

CALCITE = "omnic-toffolo-atr-calcite-spar-brazil"
CALCITE_CSV = "omnic-toffolo-atr-calcite-spar-brazil-csv"
CARBONATE = (1300.0, 1550.0)
BITUMEN_AROMATIC = "opus-bitumen-5h-120c-3"
AROMATIC, ALIPHATIC = (1535.0, 1670.0), (1350.0, 1525.0)
RAMAN = "wdf-pywdf-sp"
RAMAN_WINDOW = (1550.0, 1650.0)
UV = "jcamp-lancashire-dupinc1"
UV_WINDOW = (300.0, 350.0)
SCREEN = "gen5-lum-endpoint"
ACTIVE_BELOW = 50.0
CH_STRETCH = (2800.0, 3000.0)
FOLDER_SPECTRA = [
    ("opus-bitumen-unaged-2", "spectrum_01.2"),
    ("opus-bitumen-1h-180c-2", "spectrum_02.2"),
    ("opus-bitumen-5h-120c-3", "spectrum_03.3"),
]
FOLDER_TREATMENT = {"spectrum_01.2": "unaged", "spectrum_02.2": "RTFOT 1 h 180 C", "spectrum_03.3": "oven 5 h 120 C"}


# ---------------------------------------------------------------- readers


def _np():
    import numpy as np

    return np


def opus_ab(cid: str):
    from brukeropus import OPUSFile

    warnings.simplefilter("ignore")
    d = OPUSFile(str(a.path(cid))).a
    return _np().asarray(d.x, float), _np().asarray(d.y, float), a.rd("brukeropus")


def omnic(cid: str):
    import spectrochempy as scp

    warnings.simplefilter("ignore")
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        d = scp.read_omnic(str(a.path(cid)))
    np = _np()
    return np.asarray(d.x.data, float), np.asarray(d.data[0], float), a.rd("spectrochempy")


def export_xy(cid: str):
    np = _np()
    rows = []
    for line in a.path(cid, "oracle-export").read_text(errors="replace").splitlines():
        parts = [p for p in line.replace(",", " ").split() if p]
        if len(parts) >= 2:
            try:
                rows.append((float(parts[0]), float(parts[1])))
            except ValueError:
                continue
    arr = np.asarray(rows)
    return arr[:, 0], arr[:, 1]


def wdf(cid: str):
    from renishawWiRE import WDFReader

    warnings.simplefilter("ignore")
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        r = WDFReader(str(a.path(cid)))
    np = _np()
    y = np.asarray(r.spectra, float).reshape(-1, r.point_per_spectrum)[0]
    return np.asarray(r.xdata, float), y, a.rd("renishawWiRE")


def jcamp_xy(cid: str):
    import jcamp

    read = jcamp.jcamp_readfile if hasattr(jcamp, "jcamp_readfile") else jcamp.readfile
    d = read(str(a.path(cid)))
    np = _np()
    return np.asarray(d["x"], float), np.asarray(d["y"], float), a.rd("jcamp")


def gen5(cid: str):
    """Layout (Well ID) and the LUM and NormLum reads of a Gen5 text export."""
    text = a.path(cid).read_text(errors="replace").splitlines()
    i = text.index("Layout")
    layout = {}
    for line in text[i + 2 : i + 10]:
        cells = line.split("\t")
        for c, v in enumerate(cells[1:13], start=1):
            layout[f"{cells[0]}{c}"] = v.strip()
    j = text.index("Results")
    raw, norm = {}, {}
    for k in range(j + 2, j + 18, 2):
        r1, r2 = text[k].split("\t"), text[k + 1].split("\t")
        assert r1[-1].startswith("LUM") and r2[-1].startswith("NormLum"), (r1[-1], r2[-1])
        for c in range(1, 13):
            raw[f"{r1[0]}{c}"] = float(r1[c])
            norm[f"{r1[0]}{c}"] = float(r2[c])
    return layout, raw, norm


# ---------------------------------------------------------------- arithmetic


def band_area(x, y, lo, hi, baseline: bool, interpolate: bool) -> float:
    """Trapezoid area of y over [lo, hi] (x any order), above the straight line between the
    signal at the ends (baseline) or above zero; on the samples inside, or with the signal
    interpolated at lo and hi."""
    np = _np()
    o = np.argsort(x)
    x, y = x[o], y[o]
    m = (x >= lo) & (x <= hi)
    xs, ys = x[m], y[m]
    if interpolate:
        xs = np.concatenate([[lo], xs, [hi]])
        ys = np.concatenate([[np.interp(lo, x, y)], ys, [np.interp(hi, x, y)]])
    if baseline:
        ys = ys - np.interp(xs, [xs[0], xs[-1]], [ys[0], ys[-1]])
    return float(np.trapezoid(ys, xs))


def both(x, y, lo, hi, baseline=True) -> tuple[float, float]:
    return band_area(x, y, lo, hi, baseline, False), band_area(x, y, lo, hi, baseline, True)


def spread(values: list[float]) -> float:
    """Relative spread of the ways to compute one number (the tolerance covers it)."""
    mid = statistics.mean(values)
    return max(abs(v - mid) for v in values) / abs(mid)


def z_prime(pos: list[float], neg: list[float], ddof: int) -> float:
    sd = statistics.stdev if ddof else statistics.pstdev
    return 1 - 3 * (sd(pos) + sd(neg)) / abs(statistics.mean(pos) - statistics.mean(neg))


# ---------------------------------------------------------------- facts


def compute() -> dict[str, Any]:
    np = _np()
    data: dict[str, dict[str, Any]] = {}
    # calcite carbonate band, SpectroChemPy and OMNIC's CSV export
    x, y, rd = omnic(CALCITE)
    ex, ey = export_xy(CALCITE_CSV)
    ways = [*both(x, y, *CARBONATE), *both(ex, ey, *CARBONATE)]
    data[CALCITE] = {
        "carbonate_band_area": {
            "value": ways[0],
            "reader": rd + " + numpy",
            "how": f"trapezoid of absorbance over {CARBONATE[0]:g}-{CARBONATE[1]:g} cm-1 above the straight line "
            "between the band limits; SpectroChemPy and OMNIC's CSV export, samples inside or ends interpolated",
            "ways": ways,
        },
        "carbonate_band_area_spread": {
            "value": spread(ways),
            "reader": "the ways above",
            "how": "largest relative distance of a way from their mean (the tolerance covers twice it)",
        },
    }
    # bitumen aromaticity index
    x, y, rd = opus_ab(BITUMEN_AROMATIC)
    ways = []
    for interp in (False, True):
        c = band_area(x, y, *AROMATIC, True, interp)
        al = band_area(x, y, *ALIPHATIC, True, interp)
        ways.append(c / al)
    data[BITUMEN_AROMATIC] = {
        "aromaticity_index": {
            "value": ways[0],
            "reader": rd + " + numpy",
            "how": f"area {AROMATIC[0]:g}-{AROMATIC[1]:g} over area {ALIPHATIC[0]:g}-{ALIPHATIC[1]:g} cm-1, each above "
            "its local straight baseline; samples inside or ends interpolated",
            "ways": ways,
        },
        "aromaticity_index_spread": {
            "value": spread(ways),
            "reader": "the ways above",
            "how": "largest relative distance of a way from their mean",
        },
    }
    # Raman band maximum inside a window
    x, y, rd = wdf(RAMAN)
    m = (x >= RAMAN_WINDOW[0]) & (x <= RAMAN_WINDOW[1])
    k = int(np.argmax(y[m]))
    step = float(np.median(np.abs(np.diff(x))))
    data[RAMAN] = {
        "band_max_in_window_cm1": {
            "value": float(x[m][k]),
            "reader": rd,
            "how": f"Raman shift of the highest point between {RAMAN_WINDOW[0]:g} and {RAMAN_WINDOW[1]:g} cm-1",
        },
        "raman_step_cm1": {"value": step, "reader": rd, "how": "median spacing of the Raman-shift list"},
    }
    # UV-Vis region area, no baseline
    x, y, rd = jcamp_xy(UV)
    ways = list(both(x, y, *UV_WINDOW, baseline=False))
    data[UV] = {
        "uv_area_300_350": {
            "value": ways[0],
            "reader": rd + " + numpy",
            "how": f"trapezoid of absorbance over {UV_WINDOW[0]:g}-{UV_WINDOW[1]:g} nm (no baseline)",
            "ways": ways,
        },
        "uv_area_300_350_spread": {
            "value": spread(ways),
            "reader": "the ways above",
            "how": "largest relative distance of a way from their mean",
        },
    }
    # Gen5 screen: vendor normalisation and Z'
    layout, raw, norm = gen5(SCREEN)
    samples = {w: v for w, v in norm.items() if layout[w].startswith("SPL")}
    actives = sorted(w for w, v in samples.items() if v < ACTIVE_BELOW)
    nearest = min(abs(v - ACTIVE_BELOW) for v in samples.values())
    pos = [v for w, v in raw.items() if layout[w] == "POSCON"]
    neg = [v for w, v in raw.items() if layout[w] == "NEGCON"]
    zs = [z_prime(pos, neg, 1), z_prime(pos, neg, 0)]
    data[SCREEN] = {
        "actives_below_50": {
            "value": len(actives),
            "reader": "Gen5 NormLum (vendor-computed, in the export)",
            "how": f"sample (SPL) wells whose NormLum is below {ACTIVE_BELOW:g} %; nearest value to the cut-off "
            f"{nearest:.3f} % away",
            "wells": actives,
            "nearest_to_cutoff": nearest,
        },
        "z_prime": {
            "value": zs[0],
            "reader": "Gen5 LUM reads and layout, plain arithmetic",
            "how": "1 - 3 (SD POSCON + SD NEGCON) / |mean POSCON - mean NEGCON| with sample SDs",
        },
        "z_prime_population_sd": {
            "value": zs[1],
            "reader": "Gen5 LUM reads and layout, plain arithmetic",
            "how": "the same with population SDs",
        },
    }
    # folder: aliphatic C-H stretch area per spectrum
    areas = {}
    rds = set()
    for cid, name in FOLDER_SPECTRA:
        x, y, rd = opus_ab(cid)
        rds.add(rd)
        areas[FOLDER_TREATMENT[name]] = {
            "linear": band_area(x, y, *CH_STRETCH, True, False),
            "linear_interpolated": band_area(x, y, *CH_STRETCH, True, True),
            "none": band_area(x, y, *CH_STRETCH, False, False),
        }
    tops = {k: max(areas, key=lambda t: areas[t][k]) for k in ("linear", "linear_interpolated")}
    if len(set(tops.values())) != 1:
        raise SystemExit(f"the C-H stretch ranking depends on the method: {areas}")
    ranked = sorted(areas, key=lambda t: areas[t]["linear"], reverse=True)
    data[FOLDER_SPECTRA[0][0]] = {
        "largest_ch_stretch_treatment": {
            "value": ranked[0],
            "reader": " + ".join(sorted(rds)) + " + numpy",
            "how": f"trapezoid {CH_STRETCH[0]:g}-{CH_STRETCH[1]:g} cm-1 above the straight line between the band "
            "limits per spectrum, the sheet's treatment of the largest",
            "areas": areas,
            "margin": areas[ranked[0]]["linear"] / areas[ranked[1]]["linear"] - 1,
        }
    }
    out: dict[str, Any] = {}
    manifest = g.load_manifest()
    for cid, facts_ in data.items():
        out[cid] = {
            "extractor": "evals/gaps_facts.py",
            "file": g.manifest_entry(manifest, cid)["filename"],
            "facts": {k: a.tidy(v) for k, v in facts_.items()},
        }
    return out


def render(data: dict) -> str:
    return json.dumps(data, indent=1, sort_keys=True, ensure_ascii=False) + "\n"


# ---------------------------------------------------------------- questions


def _rel(spread_: float, floor: float) -> float:
    """A tolerance covering every correct way plus a margin."""
    return round(max(floor, 2 * spread_), 4)


GAPS_SPECS: list[g.Spec] = [
    g.Spec(
        "ana-spec-omnic-carbonate-band-area",
        CALCITE,
        "analysis",
        f"This is an ATR-FTIR absorbance spectrum of calcite. Integrate the carbonate band between "
        f"{CARBONATE[0]:g} and {CARBONATE[1]:g} cm-1 above a straight baseline drawn between the absorbance at "
        f"the two band limits. What is the band area (absorbance x cm-1)?",
        lambda f, m: g.number(f["carbonate_band_area"], None, rel=_rel(f["carbonate_band_area_spread"], 0.01)),
        "facts-gaps: carbonate_band_area (SpectroChemPy + NumPy; OMNIC's own CSV export agrees)",
        answer_hint="a number (absorbance units x cm-1)",
    ),
    g.Spec(
        "ana-spec-opus-aromaticity-index",
        BITUMEN_AROMATIC,
        "analysis",
        f"ATR-FTIR spectrum (Bruker OPUS) of an oven-aged bitumen binder. Compute its aromaticity index: the "
        f"integrated absorbance of the aromatic C=C band ({AROMATIC[0]:g}-{AROMATIC[1]:g} cm-1) divided by the "
        f"integrated absorbance of the aliphatic band ({ALIPHATIC[0]:g}-{ALIPHATIC[1]:g} cm-1), each band integrated "
        f"above a straight baseline between its own limits.",
        lambda f, m: g.number(f["aromaticity_index"], None, rel=_rel(f["aromaticity_index_spread"], 0.01)),
        "facts-gaps: aromaticity_index (brukeropus + NumPy)",
        answer_hint="a number (a ratio of two band areas)",
    ),
    g.Spec(
        "ana-spec-wdf-band-max-in-window",
        RAMAN,
        "analysis",
        f"At what Raman shift is the most intense point of this Raman spectrum between {RAMAN_WINDOW[0]:g} and "
        f"{RAMAN_WINDOW[1]:g} cm-1?",
        lambda f, m: g.number(f["band_max_in_window_cm1"], "cm-1", abs_=round(1.5 * f["raman_step_cm1"], 2)),
        "facts-gaps: band_max_in_window_cm1 (renishawWiRE)",
        answer_hint="the Raman shift in cm-1",
    ),
    g.Spec(
        "ana-spec-jcamp-uv-region-area",
        UV,
        "analysis",
        f"This JCAMP-DX file holds a UV-Vis absorbance spectrum. What is the integrated absorbance between "
        f"{UV_WINDOW[0]:g} and {UV_WINDOW[1]:g} nm (area under the spectrum, no baseline subtraction), in "
        f"absorbance x nm?",
        lambda f, m: g.number(f["uv_area_300_350"], None, rel=_rel(f["uv_area_300_350_spread"], 0.01)),
        "facts-gaps: uv_area_300_350 (jcamp + NumPy)",
        answer_hint="a number (absorbance x nm)",
    ),
    g.Spec(
        "ana-plate-gen5-screen-actives",
        SCREEN,
        "analysis",
        "Primary screen plate from a Gen5 luminescence read; its layout marks positive-control (POSCON), "
        "negative-control (NEGCON) and sample (SPL) wells. Normalise every well as percent of the control window "
        "(0 % = mean NEGCON, 100 % = mean POSCON). How many sample wells are below 50 %?",
        lambda f, m: g.integer(f["actives_below_50"]),
        "facts-gaps: actives_below_50 (Gen5's own NormLum values)",
        answer_hint="the number of wells",
    ),
    g.Spec(
        "ana-plate-gen5-screen-zprime",
        SCREEN,
        "analysis",
        "Screening plate from a Gen5 luminescence read; its layout marks the positive (POSCON) and negative "
        "(NEGCON) control wells. What is the plate's Z' factor from those controls?",
        lambda f, m: g.number(
            f["z_prime"], None, abs_=round(abs(f["z_prime"] - f["z_prime_population_sd"]) + 0.003, 4)
        ),
        "facts-gaps: z_prime (LUM reads and the Gen5 layout; sample or population SD)",
        answer_hint="a number (Z')",
    ),
]


def build_batch(manifest: dict) -> list[dict]:
    """A folder of spectra with the lab's sheet and README in it."""
    data = g.load_facts("facts-gaps")
    cid = FOLDER_SPECTRA[0][0]
    fact = data[cid]["facts"]["largest_ch_stretch_treatment"]
    sheet = "file,treatment\n" + "".join(f"{name},{FOLDER_TREATMENT[name]}\n" for _, name in FOLDER_SPECTRA)
    readme = (
        "Bitumen ageing study\n\nATR-FTIR spectra (Bruker Alpha II), one binder, three conditioning states.\n"
        "samples.csv maps each spectrum file to its treatment.\n"
    )
    return [
        {
            "id": "batch-spec-bitumen-ch-stretch-sheet",
            "family": "spectroscopy",
            "format": "share",
            "category": "batch",
            "file": {"corpus_id": "share", "path": "", "stage_as": DIR},
            "share": [
                {"corpus_id": c, "path": g.manifest_entry(manifest, c)["filename"], "stage_as": f"{DIR}/{n}"}
                for c, n in FOLDER_SPECTRA
            ],
            "generated": [
                {"stage_as": f"{DIR}/samples.csv", "content": sheet},
                {"stage_as": f"{DIR}/README.txt", "content": readme},
            ],
            "question": f"The folder `{DIR}/` holds ATR-FTIR spectra (Bruker OPUS) of one bitumen binder after "
            "different treatments, the sheet `samples.csv` naming each file's treatment, and a README. For each "
            f"spectrum integrate the aliphatic C-H stretching band between {CH_STRETCH[0]:g} and {CH_STRETCH[1]:g} "
            "cm-1 above a straight baseline between the band limits. Which treatment gives the largest band area?",
            "answer_hint": "the treatment as the sheet names it",
            "answer": {
                "type": "string",
                "value": fact["value"],
                "accept": [fact["value"]],
                "reject": [t for t in FOLDER_TREATMENT.values() if t != fact["value"]],
            },
            "source": f"evals/facts/gaps.json [{cid}] (evals/gaps_facts.py) — facts-gaps: "
            f"largest_ch_stretch_treatment ({fact['reader']}; areas {fact['areas']})",
        }
    ]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true", help="exit 1 if evals/facts/gaps.json is out of date")
    args = ap.parse_args()
    text = render(compute())
    if args.check:
        if not OUT.exists() or OUT.read_text(encoding="utf-8") != text:
            print("evals/facts/gaps.json is out of date; run evals/gaps_facts.py", file=sys.stderr)
            return 1
        print("evals/facts/gaps.json is up to date")
        return 0
    OUT.write_text(text, encoding="utf-8")
    print(f"wrote {OUT}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
