"""Agilent OpenLab CDS questions: what the data system integrated in a GC-FID injection, and the
sample it came from, as a chemist asks about an OpenLab result ("how many peaks did it find?",
"area % of the main peak?", "retention time of the peak near 5.3 min?", "area of that peak?",
"which vial?").

Answers never come from OpenReadout: they are the vendor's own values, read with the `csv`
module from the peak-table export `<injection>_1.csv` that OpenLab CDS wrote for the same
injection (Zenodo 14316687, CC-BY-4.0). The agent gets the injection container (`sample.dx`) and
its result package (`sample.rx`), never the CSV; the two tail-peak questions (a peak riding the
solvent tail: a sloped baseline) get only the `.dx`, so the agent must integrate. Tolerances
accept the vendor's table and any sound re-integration of the signal (docs/formats/openlab-cds.md
→ Validation: `analyze peaks` with its default `auto` baseline or `--baseline valley` is within 0.5 % of
OpenLab's areas at the median and 4 % at the 95th percentile; a drop line down the tail is not).

    evals/.venv/bin/python evals/openlab_facts.py          # recompute evals/facts/openlab.json
    evals/.venv/bin/python evals/openlab_facts.py --check  # exit 1 if the committed facts differ
"""

from __future__ import annotations

import argparse
import csv
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import facts
import generate as g

ROOT = Path(__file__).resolve().parent.parent
OUT = Path(__file__).resolve().parent / "facts" / "openlab.json"
READER = "OpenLab CDS peak-table export <injection>_1.csv (vendor integration of this injection), read with csv"


def vendor_csv(corpus_id: str) -> dict:
    """Header fields and peak rows of the vendor's CSV export next to the injection's `.dx`."""
    dx = facts.corpus_dir() / facts.manifest_file(corpus_id)
    p = dx.with_name(dx.stem + "_1.csv")
    if not p.exists():
        raise SystemExit(f"{corpus_id}: {p} is missing; fetch the corpus first (cargo xtask corpus fetch)")
    head: dict[str, str] = {}
    peaks: list[dict] = []
    cols = None
    for row in csv.reader(p.read_text(encoding="utf-8-sig").splitlines()):
        if not row:
            continue
        if cols is None and row[0].startswith("RT [min]"):
            cols = row
        elif cols is None:
            head.update({k.rstrip(":").strip(): v.strip() for k, v in zip(row[0::2], row[1::2], strict=False)})
        elif row[0].strip():
            d = dict(zip(cols, row, strict=False))
            peaks.append(
                {
                    "rt_min": float(d["RT [min]"]),
                    "area": float(d["Area"]),
                    "height": float(d["Height"]),
                    "area_percent": float(d["Area%"]),
                }
            )
    return {"header": head, "peaks": peaks}


def peak_count(cid: str) -> dict:
    return {"count": len(vendor_csv(cid)["peaks"])}


def main_area_percent(cid: str) -> dict:
    top = max(vendor_csv(cid)["peaks"], key=lambda p: p["area"])
    return {"area_percent": top["area_percent"], "rt_min": top["rt_min"]}


def peak_near(cid: str, rt: float) -> dict:
    p = min(vendor_csv(cid)["peaks"], key=lambda p: abs(p["rt_min"] - rt))
    return {"rt_min": p["rt_min"], "area_pa_s": p["area"], "height_pa": p["height"]}


def area_ratio(cid: str, rt_a: float, rt_b: float) -> dict:
    """Ratio of the vendor's areas of the peaks nearest `rt_a` and `rt_b`."""
    peaks = vendor_csv(cid)["peaks"]
    a = min(peaks, key=lambda p: abs(p["rt_min"] - rt_a))
    b = min(peaks, key=lambda p: abs(p["rt_min"] - rt_b))
    return {"ratio": a["area"] / b["area"], "rt_a": a["rt_min"], "rt_b": b["rt_min"]}


def sample(cid: str) -> dict:
    h = vendor_csv(cid)["header"]
    return {"sample_name": h["Sample name"], "vial": h["Location"]}


A1, B2, C3, D1, E2 = (f"zenodo14316687-polyarc-{t}" for t in ("a1-f", "b2-f2", "c3-f3", "d1-f", "e2-f2"))
# peaks riding the solvent tail (sloped baseline), asked without the vendor's result package
F3 = "zenodo14316687-polyarc-f3-f3"
FACTS = [
    (D1, "vendor_peak_count", "number of peaks in the vendor's integration results", lambda: peak_count(D1)),
    (C3, "main_peak_area_percent", "area % of the largest peak in the vendor's results", lambda: main_area_percent(C3)),
    (A1, "peak_near_5_3", "the vendor's peak nearest 5.3 min", lambda: peak_near(A1, 5.3)),
    (E2, "peak_near_9_85", "the vendor's peak nearest 9.85 min", lambda: peak_near(E2, 9.85)),
    (B2, "sample", "sample name and autosampler location printed in the vendor's export", lambda: sample(B2)),
    (F3, "peak_near_4_1", "the vendor's peak nearest 4.1 min (on the solvent tail)", lambda: peak_near(F3, 4.1)),
    (
        D1,
        "area_ratio_4_1_to_5_3",
        "ratio of the vendor's areas of the peaks nearest 4.1 min (on the solvent tail) and 5.3 min",
        lambda: area_ratio(D1, 4.1, 5.3),
    ),
]


def compute() -> dict:
    out: dict = {}
    for cid, name, how, fn in FACTS:
        entry = out.setdefault(
            cid, {"file": facts.manifest_file(cid), "extractor": "evals/openlab_facts.py", "facts": {}}
        )
        entry["facts"][name] = {"value": fn(), "reader": READER, "how": how}
    return out


def render(data: dict) -> str:
    return json.dumps(data, indent=1, ensure_ascii=False, sort_keys=True) + "\n"


def rx(cid: str) -> list[tuple[str, str]]:
    """The injection's result package, staged beside it."""
    return [(f"{cid}-rx", "sample.rx")]


OPENLAB_SPECS: list[g.Spec] = [
    g.Spec(
        "qnt-openlab-vendor-peak-count",
        D1,
        "quantitation",
        "This is a GC-FID injection from Agilent OpenLab CDS, with the data system's processing results saved "
        "beside it. How many peaks did the data system integrate and report for this injection?",
        lambda f, m: g.integer(f["vendor_peak_count"]["count"]),
        "facts-openlab: vendor_peak_count (the vendor's CSV export of the same results)",
        extra=rx(D1),
        answer_hint="a whole number",
    ),
    g.Spec(
        "qnt-openlab-main-peak-area-percent",
        C3,
        "quantitation",
        "This is a GC-FID injection from Agilent OpenLab CDS, with the data system's processing results saved "
        "beside it. In those results, what is the area percent of the largest peak?",
        lambda f, m: g.number(f["main_peak_area_percent"]["area_percent"], "%", abs_=0.5),
        "facts-openlab: main_peak_area_percent (the vendor's CSV export of the same results)",
        extra=rx(C3),
        answer_hint="a percentage",
    ),
    g.Spec(
        "qnt-openlab-peak-rt",
        A1,
        "quantitation",
        "This is a GC-FID injection from Agilent OpenLab CDS. There is a peak at about 5.3 min: what is its "
        "retention time, to three decimals?",
        lambda f, m: g.number(f["peak_near_5_3"]["rt_min"], "min", abs_=0.01),
        "facts-openlab: peak_near_5_3 (the vendor's CSV export of this injection)",
        extra=rx(A1),
        answer_hint="a retention time with its unit",
    ),
    g.Spec(
        "qnt-openlab-peak-area",
        E2,
        "quantitation",
        "This is a GC-FID injection from Agilent OpenLab CDS. What is the area of the peak at about 9.85 min, "
        "in pA·s (picoampere-seconds)?",
        lambda f, m: g.number(f["peak_near_9_85"]["area_pa_s"], None, rel=0.05),
        "facts-openlab: peak_near_9_85 (the vendor's CSV export of this injection)",
        extra=rx(E2),
        answer_hint="a number (peak area in pA·s)",
    ),
    # Sloped baseline: the peak at 4.1 min rides the tail of the solvent peak. Only the signal is
    # staged (no .rx), so the agent must integrate; OpenLab ends the peak where it meets the tail.
    # A drop line to the far end of the tail gives 1.5-9x the area; a baseline on the tail (valley
    # to valley, or peaks' default auto) is within 4 % of the vendor.
    g.Spec(
        "qnt-openlab-tail-peak-area",
        F3,
        "quantitation",
        "This is a GC-FID injection from Agilent OpenLab CDS (the signal only, no processing results). The peak "
        "at about 4.1 min elutes on the tail of the solvent peak. What is its area in pA·s (picoampere-seconds), "
        "integrated above the solvent tail?",
        lambda f, m: g.number(f["peak_near_4_1"]["area_pa_s"], None, rel=0.05),
        "facts-openlab: peak_near_4_1 (the vendor's CSV export of this injection)",
        answer_hint="a number (peak area in pA·s)",
    ),
    g.Spec(
        "qnt-openlab-tail-peak-area-ratio",
        D1,
        "quantitation",
        "This is a GC-FID injection from Agilent OpenLab CDS (the signal only, no processing results). What is "
        "the ratio of the area of the peak at about 4.1 min, which rides the tail of the solvent peak, to the "
        "area of the peak at about 5.3 min?",
        lambda f, m: g.number(f["area_ratio_4_1_to_5_3"]["ratio"], None, rel=0.06),
        "facts-openlab: area_ratio_4_1_to_5_3 (the vendor's CSV export of this injection)",
        answer_hint="a number (area ratio)",
    ),
    g.Spec(
        "smp-openlab-vial",
        B2,
        "sample",
        "This is an injection from Agilent OpenLab CDS. From which autosampler vial position was it injected?",
        lambda f, m: g.string(f["sample"]["vial"], accept=[f"vial {f['sample']['vial']}"]),
        "facts-openlab: sample (Location in the vendor's CSV export of this injection)",
        extra=rx(B2),
        answer_hint="the vial position",
    ),
]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true", help="exit 1 if evals/facts/openlab.json is out of date")
    args = ap.parse_args()
    text = render(compute())
    if args.check:
        if not OUT.exists() or OUT.read_text(encoding="utf-8") != text:
            print("evals/facts/openlab.json is out of date; run evals/openlab_facts.py", file=sys.stderr)
            return 1
        print("evals/facts/openlab.json is up to date")
        return 0
    OUT.write_text(text, encoding="utf-8")
    print(f"wrote {OUT.relative_to(ROOT)}: {len(FACTS)} facts")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
