"""Quantitation tier: peak areas, heights, retention times, area-% purity and area ratios of
chromatographic peaks, and extracted-ion / SRM chromatogram peaks: the everyday questions of an
analytical chemist ("what is the area of the main peak?", "what is the purity by area %?", "at
what retention time does m/z X elute and how big is its peak?").

Answers never come from OpenReadout. They are either computed by the vendor's own data system
for the same injection and stored with the raw data (Agilent ChemStation `Report.TXT`
area-percent reports, MSD ChemStation `RESULTS.CSV` integrator results; the agent gets only the
signal file, not the report), or computed here with pyteomics (Apache-2.0, reading the
depositor's own mzML of the vendor run the agent gets) and pyOpenMS (BSD-3:
PeakPickerChromatogram + PeakIntegrator, trapezoid, base-to-base). Tolerances accept the method
differences between integrators (baseline placement, peak boundaries): see book/src/guides/quantitation.md,
where the same vendor comparisons are run over 300 peaks.

    oracle/.venv/bin/python evals/quant.py          # recompute evals/facts/quant.json
    oracle/.venv/bin/python evals/quant.py --check  # exit 1 if the committed facts differ

(pyOpenMS is in the worktree oracle project: `cd oracle && uv sync` then oracle/.venv/bin/python.)
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass
from importlib.metadata import version
from pathlib import Path
from typing import Any, Callable

sys.path.insert(0, str(Path(__file__).resolve().parent))
import facts
import generate as g

ROOT = Path(__file__).resolve().parent.parent
OUT = Path(__file__).resolve().parent / "facts" / "quant.json"


def path(corpus_id: str, role: str = "input") -> Path:
    p = facts.corpus_dir() / facts.manifest_file(corpus_id, role)
    if not p.exists():
        raise SystemExit(f"{corpus_id}: {p} is missing; fetch the corpus first (cargo xtask corpus fetch)")
    return p


def tidy(v: Any) -> Any:
    if isinstance(v, float):
        return float(f"{v:.8g}")
    if isinstance(v, dict):
        return {k: tidy(x) for k, x in v.items()}
    return v


# ---------------------------------------------------------------- vendor reports


def report_peaks(dir_id: str, signal: str) -> list[dict]:
    """Peak rows of one signal of a ChemStation area-percent Report.TXT (UTF-16 text)."""
    b = path(f"{dir_id}-report-txt", "part").read_bytes()
    t = b.decode("utf-16") if b[:2] in (b"\xff\xfe", b"\xfe\xff") else b.decode("latin-1")
    rows: list[dict] = []
    for sec in re.split(r"\n\s*Signal \d+: ", t)[1:]:
        if sec.split("\n", 1)[0].strip().rstrip(",").strip() != signal:
            continue
        for line in sec.split("\n"):
            m = re.match(
                r"\s*(\d+)\s+([\d.]+)\s+([A-Z]{2,4}(?: [A-Z])?)\s+([\d.]+)"
                r"\s+([\d.eE+-]+)\s+([\d.eE+-]+)\s+([\d.eE+-]+)",
                line,
            )
            if m:
                rows.append(
                    {
                        "rt_min": float(m.group(2)),
                        "area": float(m.group(5)),
                        "height": float(m.group(6)),
                        "area_percent": float(m.group(7)),
                    }
                )
        if rows:
            break
    if not rows:
        raise SystemExit(f"{dir_id}: no rows for signal {signal!r} in Report.TXT")
    return rows


def results_csv(cid: str) -> list[dict]:
    rows = []
    for line in path(cid, "companion").read_text(errors="replace").splitlines():
        m = re.match(r"\d+=,\s*\d+,\s*([\d.]+),\s*\d+,\s*\d+,\s*\d+,\"[^\"]*\",\s*(\d+),\s*(\d+),", line)
        if m:
            rows.append({"rt_min": float(m.group(1)), "height": float(m.group(2)), "area": float(m.group(3))})
    return rows


REPORT = "ChemStation Report.TXT (vendor integration of this injection)"


def main_area(dir_id: str, signal: str):
    top = max(report_peaks(dir_id, signal), key=lambda p: p["area"])
    return {"area": top["area"], "rt_min": top["rt_min"]}, REPORT


def main_area_percent(dir_id: str, signal: str):
    top = max(report_peaks(dir_id, signal), key=lambda p: p["area"])
    return {"area_percent": top["area_percent"], "rt_min": top["rt_min"]}, REPORT


def main_height(dir_id: str, signal: str):
    top = max(report_peaks(dir_id, signal), key=lambda p: p["area"])
    return {"height": top["height"], "rt_min": top["rt_min"]}, REPORT


def second_peak_rt(dir_id: str, signal: str):
    rows = sorted(report_peaks(dir_id, signal), key=lambda p: -p["area"])
    return {"rt_min": rows[1]["rt_min"], "area": rows[1]["area"]}, REPORT


def tic_area_ratio(cid: str, rt_a: float, rt_b: float):
    rows = results_csv(f"{cid.removesuffix('-data-ms')}-results-csv")
    pick = lambda rt: min(rows, key=lambda r: abs(r["rt_min"] - rt))  # noqa: E731
    a, b = pick(rt_a), pick(rt_b)
    return {"ratio": a["area"] / b["area"], "rt_a": a["rt_min"], "rt_b": b["rt_min"]}, (
        "MSD ChemStation RESULTS.CSV (vendor integrator on the total-ion chromatogram)"
    )


# ---------------------------------------------------------------- pyteomics + pyOpenMS


def _pick_largest(t_min, y) -> dict:
    """pyOpenMS PeakPickerChromatogram (SG smoothing) on (seconds, intensity); the picked peak
    with the largest PeakIntegrator trapezoid area (base-to-base) between the picker's bounds."""
    import numpy as np
    import pyopenms as oms

    t_s = np.asarray(t_min, dtype=float) * 60.0
    c = oms.MSChromatogram()
    c.set_peaks((t_s, np.asarray(y, dtype=float)))
    pp = oms.PeakPickerChromatogram()
    p = pp.getDefaults()
    p.setValue("use_gauss", "false")
    p.setValue("method", "corrected")
    pp.setParameters(p)
    picked = oms.MSChromatogram()
    pp.pickChromatogram(c, picked)
    fda = {a.getName(): list(a) for a in picked.getFloatDataArrays()}
    pi = oms.PeakIntegrator()
    q = pi.getDefaults()
    q.setValue("integration_type", "trapezoid")
    q.setValue("baseline_type", "base_to_base")
    pi.setParameters(q)
    best = None
    for k in range(picked.size()):
        lw, rw = fda["leftWidth"][k], fda["rightWidth"][k]
        pa = pi.integratePeak(c, lw, rw)
        bg = pi.estimateBackground(c, lw, rw, pa.apex_pos)
        e = {
            "apex_rt_min": pa.apex_pos / 60.0,
            "area_counts_s": pa.area - bg.area,
            "height": pa.height - bg.height,
            "start_min": lw / 60.0,
            "end_min": rw / 60.0,
        }
        if best is None or e["area_counts_s"] > best["area_counts_s"]:
            best = e
    return best


def xic_peak(cid: str, mz: float, ppm: float):
    import numpy as np
    from pyteomics import mzml

    lo, hi = mz * (1 - ppm * 1e-6), mz * (1 + ppm * 1e-6)
    t, y = [], []
    with mzml.MzML(str(path(cid, "oracle-export"))) as r:
        for s in r:
            if s["ms level"] != 1:
                continue
            t.append(float(s["scanList"]["scan"][0]["scan start time"]))
            m, i = np.asarray(s["m/z array"]), np.asarray(s["intensity array"], dtype=float)
            y.append(float(i[(m >= lo) & (m <= hi)].sum()))
    return _pick_largest(t, y), (
        f"pyteomics {version('pyteomics')} XIC on the depositor's mzML + pyOpenMS {version('pyopenms')} "
        "PeakPickerChromatogram/PeakIntegrator (trapezoid, base-to-base)"
    )


def srm_peak(cid: str, q1: float, q3: float):
    from pyteomics import mzml

    with mzml.MzML(str(path(cid, "oracle-export"))) as r:
        for c in r.iterfind("chromatogram"):
            if "selected reaction monitoring chromatogram" not in c:
                continue
            one = lambda x: x[0] if isinstance(x, list) else x  # noqa: E731
            p = float(one(c["precursor"])["isolationWindow"]["isolation window target m/z"])
            d = float(one(c["product"])["isolationWindow"]["isolation window target m/z"])
            if abs(p - q1) < 1e-3 and abs(d - q3) < 1e-3:
                return _pick_largest(c["time array"], c["intensity array"]), (
                    f"pyteomics {version('pyteomics')} SRM chromatogram of the depositor's mzML + pyOpenMS "
                    f"{version('pyopenms')} PeakPickerChromatogram/PeakIntegrator (trapezoid, base-to-base)"
                )
    raise SystemExit(f"{cid}: no SRM chromatogram {q1}>{q3}")


# ---------------------------------------------------------------- facts


@dataclass
class Fact:
    corpus_id: str
    role: str
    name: str
    how: str
    compute: Callable[[], tuple[Any, str]]


FID = "FID1 A"
TCD = "TCD2 B"

FACTS: list[Fact] = [
    Fact(
        "chromhandler-001f0102-d-fid1a-ch",
        "part",
        "main_peak",
        "ChemStation area-percent report, FID1 A: the peak with the largest area (pA*s) and its retention time",
        lambda: main_area("chromhandler-001f0102-d", FID),
    ),
    Fact(
        "chromhandler-001f0101-d-fid1a-ch",
        "part",
        "main_peak_area_percent",
        "ChemStation area-percent report, FID1 A: area % of the largest peak",
        lambda: main_area_percent("chromhandler-001f0101-d", FID),
    ),
    Fact(
        "chromhandler-001f0104-d-tcd2b-ch",
        "part",
        "main_peak_height",
        "ChemStation area-percent report, TCD2 B: height (25 uV) of the largest peak",
        lambda: main_height("chromhandler-001f0104-d", TCD),
    ),
    Fact(
        "chromhandler-001f0103-d-fid1a-ch",
        "part",
        "second_peak",
        "ChemStation area-percent report, FID1 A: retention time of the second-largest peak by area",
        lambda: second_peak_rt("chromhandler-001f0103-d", FID),
    ),
    Fact(
        "chromhandler-rau-r505-05-d-data-ms",
        "input",
        "tic_area_ratio_6p06_6p30",
        "MSD ChemStation integrator, TIC: area of the peak at 6.06 min / area of the peak at 6.30 min",
        lambda: tic_area_ratio("chromhandler-rau-r505-05-d-data-ms", 6.055, 6.301),
    ),
    Fact(
        "mtbls20-caffeine-pos",
        "oracle-export",
        "xic_195_0877_peak",
        "MS1 XIC of m/z 195.0877 ± 10 ppm (sum per scan); the largest peak picked and integrated by pyOpenMS",
        lambda: xic_peak("mtbls20-caffeine-pos", 195.0877, 10.0),
    ),
    Fact(
        "mtbls1822-tsq-74",
        "oracle-export",
        "srm_155_004_111_074_peak",
        "SRM chromatogram 155.004 > 111.074 (its most intense transition) of the depositor's mzML; "
        "the largest peak picked and integrated by pyOpenMS",
        lambda: srm_peak("mtbls1822-tsq-74", 155.004, 111.074),
    ),
]


def compute() -> dict:
    out: dict = {}
    for f in FACTS:
        value, reader = f.compute()
        entry = out.setdefault(
            f.corpus_id,
            {"file": facts.manifest_file(f.corpus_id, f.role), "extractor": "evals/quant.py", "facts": {}},
        )
        entry["facts"][f.name] = {"value": tidy(value), "reader": reader, "how": f.how}
        print(f"{f.corpus_id} {f.name} = {value!r}", file=sys.stderr)
    return out


def render(data: dict) -> str:
    return json.dumps(data, indent=1, ensure_ascii=False, sort_keys=True) + "\n"


# ---------------------------------------------------------------- questions

HOW = " Report the value only (you may state how you integrated)."

QUANT_SPECS: list[g.Spec] = [
    g.Spec(
        "qnt-gc-fid-main-peak-area",
        "chromhandler-001f0102-d-fid1a-ch",
        "quantitation",
        "This is the FID signal of a GC run. Integrate its peaks: what is the area of the largest peak, in pA·s "
        "(picoampere-seconds)?" + HOW,
        lambda f, m: g.number(f["main_peak"]["area"], None, rel=0.05),
        "facts-quant: main_peak (ChemStation Report.TXT of the same injection)",
        answer_hint="a number (peak area in pA·s)",
    ),
    g.Spec(
        "qnt-gc-fid-purity",
        "chromhandler-001f0101-d-fid1a-ch",
        "quantitation",
        "This is the FID signal of a GC run. By area normalization (area % of all integrated peaks), what is "
        "the purity of the main component, in percent?",
        lambda f, m: g.number(f["main_peak_area_percent"]["area_percent"], "%", abs_=1.0),
        "facts-quant: main_peak_area_percent (ChemStation Report.TXT of the same injection)",
        answer_hint="a percentage",
    ),
    g.Spec(
        "qnt-gc-tcd-main-peak-height",
        "chromhandler-001f0104-d-tcd2b-ch",
        "quantitation",
        "This is the thermal-conductivity detector (TCD) signal of a GC run, in units of 25 µV. What is the "
        "height of its largest peak above the baseline, in those signal units?",
        lambda f, m: g.number(f["main_peak_height"]["height"], None, rel=0.05),
        "facts-quant: main_peak_height (ChemStation Report.TXT of the same injection)",
        answer_hint="a number (signal units)",
    ),
    g.Spec(
        "qnt-gc-fid-second-peak-rt",
        "chromhandler-001f0103-d-fid1a-ch",
        "quantitation",
        "This is the FID signal of a GC run. At what retention time does the second-largest peak (by area) elute?",
        lambda f, m: g.number(f["second_peak"]["rt_min"], "min", abs_=0.02),
        "facts-quant: second_peak (ChemStation Report.TXT of the same injection)",
        answer_hint="a retention time with its unit",
    ),
    g.Spec(
        "qnt-gcms-tic-area-ratio",
        "chromhandler-rau-r505-05-d-data-ms",
        "quantitation",
        "This is a GC-MS run. In its total-ion chromatogram, what is the ratio of the peak area of the peak at "
        "about 6.06 min to that of the peak at about 6.30 min?",
        lambda f, m: g.number(f["tic_area_ratio_6p06_6p30"]["ratio"], None, rel=0.06),
        "facts-quant: tic_area_ratio_6p06_6p30 (MSD ChemStation RESULTS.CSV of the same run)",
        answer_hint="a number (a ratio)",
    ),
    g.Spec(
        "qnt-ms-xic-caffeine-apex",
        "mtbls20-caffeine-pos",
        "quantitation",
        "Extract the ion chromatogram of protonated caffeine, m/z 195.0877 (± 10 ppm), from the full scans of "
        "this LC-MS run. At what retention time does its peak apex elute?",
        lambda f, m: g.number(f["xic_195_0877_peak"]["apex_rt_min"], "min", abs_=0.05),
        "facts-quant: xic_195_0877_peak (pyteomics + pyOpenMS on the depositor's mzML)",
        answer_hint="a retention time with its unit",
    ),
    g.Spec(
        "qnt-ms-srm-peak-area",
        "mtbls1822-tsq-74",
        "quantitation",
        "This is a triple-quadrupole SRM run. Integrate the chromatogram of the transition 155.004 > 111.074: "
        "what is the area of its largest peak, in intensity × seconds (counts·s)?",
        lambda f, m: g.number(f["srm_155_004_111_074_peak"]["area_counts_s"], None, rel=0.15),
        "facts-quant: srm_155_004_111_074_peak (pyteomics + pyOpenMS on the depositor's mzML)",
        answer_hint="a number (peak area in counts·s)",
    ),
]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true", help="exit 1 if evals/facts/quant.json is out of date")
    args = ap.parse_args()
    text = render(compute())
    if args.check:
        if not OUT.exists() or OUT.read_text(encoding="utf-8") != text:
            print("evals/facts/quant.json is out of date; run evals/quant.py", file=sys.stderr)
            return 1
        print("evals/facts/quant.json is up to date")
        return 0
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(text, encoding="utf-8")
    print(f"wrote {OUT.relative_to(ROOT)}: {len(FACTS)} facts")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
