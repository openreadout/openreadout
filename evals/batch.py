"""Batch tier: questions about a folder of files plus the lab's sample sheet or plate map.

"Here is a folder of FCS files and a sample sheet; what is the median FSC-A of the treated
group?", "which files belong to donor 3?", "which condition on this plate map has the highest
mean absorbance?", "which file is the same acquisition as run_001.d?". Each question stages a
folder `sample_batch/` holding corpus files (under neutral names, except where the analysis
software needs the original ones) and one generated file (a sample sheet or plate layout whose
content is fixed here and stored in the question), and asks for a number, a name or a list that
needs every file plus the sheet.

The answers are computed here with third-party readers in the oracle venv, never with
OpenReadout: FlowIO (BSD-3) for FCS scale values and TEXT keywords, FlowKit (BSD-3) for
FlowJo-workspace gating, allotropy (MIT) for plate-reader values, and pandas for the group
arithmetic; the pairing of a vendor file with its conversion comes from corpus/manifest.toml.
They go to evals/facts/batch.json (committed), so the questions regenerate without the corpus.

    cd oracle && uv run --group plate python ../evals/batch.py          # recompute the facts
    cd oracle && uv run --group plate python ../evals/batch.py --check  # exit 1 on drift
"""

from __future__ import annotations

import argparse
import json
import sys
import tomllib
from importlib.metadata import version
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
FACTS = Path(__file__).resolve().parent / "facts" / "batch.json"
MANIFEST = ROOT / "corpus" / "manifest.toml"
DIR = "sample_batch"

# Flow cytometry files with FSC-A and a sample name recorded in TEXT ($SMNO or TUBE NAME).
FLOW = [
    "fcsparser-facs-diva",
    "fcsparser-fortessa-a01",
    "fcsparser-hts-lsr-ii-d06",
    "flowio-100715",
    "flowio-b01-kc-a-w-91-us",
    "flowkit-comp-example",
    "zenodo18439538-cytoflex",
    "zenodo19221995-facsdiscover-zam36",
    "zenodo7971252-sony-sa3800",
]
PLATE = "kaleido-abs-endpoint"
GATE_FCS = ["flowkit-8c-e01", "flowkit-8c-e03", "flowkit-8c-e05"]
GATE_WSP = "flowkit-8c-ics"
CD4 = "/Time/Singlets/aAmine-/CD3+/CD4+"
CD3 = "/Time/Singlets/aAmine-/CD3+"
# vendor directory + its depositor mzML conversion, plus unrelated files
LINK_PAIRS = ["mtbls449-13047CHQ_0001_A1", "mtbls243-03_D24062013T1259_1399CBU_01QC_A3"]
LINK_OTHERS = ["pyteomics-tiny-pwiz", "fcsparser-fortessa-a01", "flowio-g11"]


def manifest() -> list[dict]:
    with MANIFEST.open("rb") as fh:
        return tomllib.load(fh)["file"]


def entry(m: list[dict], cid: str, role: str = "input") -> dict:
    for e in m:
        if e["id"] == cid and e.get("role", "input") == role:
            return e
    raise SystemExit(f"{cid} ({role}) not in the manifest")


def suffix(filename: str) -> str:
    base = filename.rstrip("/").rsplit("/", 1)[-1]
    return base[base.rindex(".") :] if "." in base else ""


# ---------------------------------------------------------------- layouts and sheets (fixed)


def flow_plan(m: list[dict]) -> list[tuple[dict, str]]:
    return [(entry(m, c), f"{DIR}/run_{i + 1:03d}.fcs") for i, c in enumerate(FLOW)]


def condition_sheet(plan: list[tuple[dict, str]]) -> str:
    """file (stem, no extension), condition: alternating control / treated."""
    lines = ["sample_file,condition"]
    for i, (_, staged) in enumerate(plan):
        stem = staged.rsplit("/", 1)[-1].rsplit(".", 1)[0]
        lines.append(f"{stem},{'treated' if i % 2 else 'control'}")
    return "\n".join(lines) + "\n"


def plate_layout() -> tuple[str, dict[str, tuple[str, str]]]:
    """A 96-well plate map (condition and dose grids) and well → (condition, dose)."""
    letters = "ABCDEFGH"
    cond, dose = {}, {}
    for r in range(8):
        for c in range(12):
            w = f"{letters[r]}{c + 1}"
            cond[w] = "ctrl" if c < 3 else ("drugA" if c < 7 else ("drugB" if c < 11 else ""))
            dose[w] = "" if c >= 11 else ["0", "0", "0", "1", "1", "10", "10", "1", "1", "10", "10"][c]
    rows = ["condition," + ",".join(str(i) for i in range(1, 13))]
    for r in range(8):
        rows.append(letters[r] + "," + ",".join(cond[f"{letters[r]}{c}"] for c in range(1, 13)))
    rows += ["", "dose (uM)", "," + ",".join(str(i) for i in range(1, 13))]
    for r in range(8):
        rows.append(letters[r] + "," + ",".join(dose[f"{letters[r]}{c}"] for c in range(1, 13)))
    return "\n".join(rows) + "\n", {w: (cond[w], dose[w]) for w in cond}


# ---------------------------------------------------------------- facts (oracle readers)


def fcs_path(e: dict) -> Path:
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import facts

    return facts.corpus_dir() / e["filename"]


def compute() -> dict[str, Any]:
    import flowio
    import numpy as np
    import pandas as pd

    m = manifest()
    out: dict[str, Any] = {
        "generator": (
            f"flowio {version('flowio')}, flowkit {version('flowkit')}, "
            f"allotropy {version('allotropy')}, pandas {pd.__version__}"
        ),
    }
    # FCS: median FSC-A (scale values) and recorded sample names
    plan = flow_plan(m)
    rows = []
    for e, staged in plan:
        fd = flowio.FlowData(str(fcs_path(e)), ignore_offset_error=True)
        a = fd.as_array(preprocess=True)
        i = list(fd.pnn_labels).index("FSC-A")
        v = np.asarray(a[:, i], dtype=np.float64)
        text = {k.lower().lstrip("$"): v for k, v in fd.text.items()}
        name = text.get("smno") or text.get("tube name")
        rows.append(
            {"id": e["id"], "staged": staged, "median_fsc_a": float(np.median(v[np.isfinite(v)])), "sample_name": name}
        )
    out["flow"] = rows
    # plate values (allotropy)
    sys.path.insert(0, str(ROOT / "oracle"))
    import plate

    pe = entry(m, PLATE)
    p = fcs_path(pe)
    asm = plate.allotropy_asm(p, plate.vendor_for(p.name))
    wells: dict[str, float] = {}
    for doc in asm["plate reader aggregate document"]["plate reader document"]:
        for mm in doc["measurement aggregate document"]["measurement document"]:
            if "error aggregate document" in mm:
                continue
            for k, val in mm.items():
                if plate.mode_of(k) and isinstance(val, dict) and "value" in val:
                    wells[mm["sample document"]["location identifier"]] = float(val["value"])
    out["plate"] = {"id": PLATE, "wells": wells}
    # FlowKit gating of the 8-colour ICS workspace
    import flowkit as fk

    wsp = fk.parse_wsp(str(fcs_path(entry(m, GATE_WSP, "analysis"))))
    gate_rows = []
    for cid in GATE_FCS:
        e = entry(m, cid)
        sample_name = next(n for n in wsp["samples"] if cid[-3:].upper() in n)
        s = fk.Sample(str(fcs_path(e)), sample_id=sample_name, use_flowjo_labels=True, subsample=0)
        res = wsp["samples"][sample_name]["gating_strategy"].gate_sample(s)
        counts = {}
        for _, r in res.report.iterrows():
            path = "/" + "/".join([x for x in r["gate_path"] if x != "root"] + [r["gate_name"]])
            counts[path] = int(r["count"])
        gate_rows.append({"id": cid, "sample": sample_name, "cd3": counts[CD3], "cd4": counts[CD4]})
    out["gate"] = gate_rows
    return out


def tidy(v: Any) -> Any:
    if isinstance(v, float):
        return float(f"{v:.10g}")
    if isinstance(v, dict):
        return {k: tidy(x) for k, x in v.items()}
    if isinstance(v, list):
        return [tidy(x) for x in v]
    return v


# ---------------------------------------------------------------- questions (stdlib only)


def number(value: float, unit: str | None = None, rel: float = 0.005) -> dict:
    return {"type": "number", "value": value, "unit": unit, "tolerance": {"rel": rel}}


def question(
    qid: str, family: str, text: str, hint: str, answer: dict, share_: list[dict], generated: list[dict], source: str
) -> dict:
    return {
        "id": qid,
        "family": family,
        "format": "share",
        "category": "batch",
        "file": {"corpus_id": "share", "path": "", "stage_as": DIR},
        "share": share_,
        "generated": generated,
        "question": text,
        "answer_hint": hint,
        "answer": answer,
        "source": f"evals/facts/batch.json (evals/batch.py) — {source}",
    }


def build_all() -> list[dict]:
    m = manifest()
    with FACTS.open() as fh:
        f = json.load(fh)
    qs = []
    # 1. median FSC-A of the treated group, sheet keyed by file stem
    plan = flow_plan(m)
    share_ = [{"corpus_id": e["id"], "path": e["filename"], "stage_as": s} for e, s in plan]
    sheet = condition_sheet(plan)
    treated = {line.split(",")[0] for line in sheet.splitlines()[1:] if line.endswith(",treated")}
    meds = [r["median_fsc_a"] for r in f["flow"] if r["staged"].rsplit("/", 1)[-1].rsplit(".", 1)[0] in treated]
    qs.append(
        question(
            "batch-fcs-treated-median-fsc",
            "flow",
            "The folder holds flow cytometry (FCS) files and a sample sheet `sample_batch/samples.csv` that gives each "
            "file's condition. For each FCS file take the median of its FSC-A values (scale values, i.e. with $PnE and "
            "$PnG applied, as FlowJo shows them), then average those per-file medians over the files in the 'treated' "
            "condition. What is that average?",
            "a number",
            number(sum(meds) / len(meds)),
            share_,
            [{"stage_as": f"{DIR}/samples.csv", "content": sheet}],
            "flow[].median_fsc_a (FlowIO as_array(preprocess=True), NumPy median) "
            "averaged over the sheet's treated files",
        )
    )
    # 2. which files belong to donor 3, sheet keyed by the sample name recorded in each file
    names = [(r["sample_name"], r["staged"]) for r in f["flow"]]
    donor = {n: 1 + (i % 3) for i, (n, _) in enumerate(sorted(names))}
    lines = ["Sample,Donor"] + [f"{n},{donor[n]}" for n, _ in sorted(names, key=lambda x: x[0][::-1])]
    three = sorted(s for n, s in names if donor[n] == 3)
    qs.append(
        question(
            "batch-fcs-donor-files",
            "flow",
            "The folder holds FCS files and a sample sheet `sample_batch/donors.csv` listing each sample by the sample "
            "name recorded in its FCS file (the $SMNO keyword, or TUBE NAME where there is none) and the donor it came "
            "from. Which FCS files belong to donor 3? Give their paths.",
            "the paths of the files, comma-separated",
            {
                "type": "list",
                "value": [s.split("/", 1)[1] for s in three],
                "accept": [sorted({s, s.split("/", 1)[1]}) for s in three],
                "ordered": False,
            },
            share_,
            [{"stage_as": f"{DIR}/donors.csv", "content": "\n".join(lines) + "\n"}],
            "flow[].sample_name (FlowIO TEXT: $SMNO, else TUBE NAME) matched to the sheet's donor",
        )
    )
    # 3, 4. plate map
    pe = entry(m, PLATE)
    layout, wells = plate_layout()
    vals = f["plate"]["wells"]
    plate_share = [
        {"corpus_id": PLATE, "path": pe["filename"], "stage_as": f"{DIR}/plate_read{suffix(pe['filename'])}"}
    ]
    a10 = [vals[w] for w, (c, d) in wells.items() if c == "drugA" and d == "10"]
    qs.append(
        question(
            "batch-plate-drug-a-10um",
            "plates",
            "The folder holds a plate-reader absorbance export and the plate map "
            "`sample_batch/plate_map.csv` (one grid "
            "for the condition and one for the dose of every well). What is the mean absorbance of the wells treated "
            "with drugA at 10 µM?",
            "the mean absorbance",
            number(sum(a10) / len(a10)),
            plate_share,
            [{"stage_as": f"{DIR}/plate_map.csv", "content": layout}],
            "plate.wells (allotropy ASM values) averaged over the map's drugA / 10 wells",
        )
    )
    means = {}
    for cond in ("ctrl", "drugA", "drugB"):
        v = [vals[w] for w, (c, _) in wells.items() if c == cond]
        means[cond] = sum(v) / len(v)
    ranked = sorted(means, key=means.get, reverse=True)
    if means[ranked[0]] < 1.01 * means[ranked[1]]:
        raise SystemExit(f"plate conditions too close to rank: {means}")
    others = [c for c in means if c != ranked[0]]
    qs.append(
        question(
            "batch-plate-highest-condition",
            "plates",
            "The folder holds a plate-reader absorbance export and the plate map `sample_batch/plate_map.csv`. Wells "
            "left empty on the map were not used. Which condition has the highest mean absorbance over its wells?",
            "the condition name as written on the map",
            {"type": "string", "value": ranked[0], "accept": [ranked[0]], "reject": others},
            plate_share,
            [{"stage_as": f"{DIR}/plate_map.csv", "content": layout}],
            "plate.wells (allotropy) averaged per map condition: "
            + json.dumps({k: round(v, 5) for k, v in means.items()}),
        )
    )
    # 5. FlowJo workspace: % CD4+ of CD3+ in the stimulated samples
    gshare = [
        {"corpus_id": c, "path": entry(m, c)["filename"], "stage_as": f"{DIR}/{g['sample']}"}
        for c, g in zip(GATE_FCS, f["gate"], strict=True)
    ]
    wsp = entry(m, GATE_WSP, "analysis")
    gshare.append({"corpus_id": GATE_WSP, "path": wsp["filename"], "stage_as": f"{DIR}/analysis.wsp"})
    groups = {g["sample"]: ("stimulated" if i < 2 else "unstimulated") for i, g in enumerate(f["gate"])}
    sheet = "fcs_file,group\n" + "".join(f"{s},{grp}\n" for s, grp in sorted(groups.items()))
    pct = [100 * g["cd4"] / g["cd3"] for g in f["gate"] if groups[g["sample"]] == "stimulated"]
    qs.append(
        question(
            "batch-gate-cd4-of-cd3",
            "flow",
            "The folder holds three FCS files, the FlowJo workspace `sample_batch/analysis.wsp` that gates them, and a "
            "sample sheet `sample_batch/groups.csv`. Using the workspace's gates, what percentage of CD3+ "
            "cells are CD4+ "
            "(population CD3+/CD4+ as a percentage of CD3+) on average over the samples of the 'stimulated' group?",
            "a percentage",
            {"type": "number", "value": sum(pct) / len(pct), "unit": None, "tolerance": {"abs": 0.15}},
            gshare,
            [{"stage_as": f"{DIR}/groups.csv", "content": sheet}],
            "gate[] (FlowKit gate_sample of flowkit-8c-ics) CD4+/CD3+ counts, averaged over the sheet's stimulated "
            "samples; 0.15 points tolerance covers FlowJo's own stored counts",
        )
    )
    # 6. link: which file is the same acquisition as the first vendor directory
    lshare = []
    n = 0

    def add(e: dict) -> str:
        nonlocal n
        n += 1
        s = f"{DIR}/run_{n:03d}{suffix(e['filename'])}"
        lshare.append({"corpus_id": e["id"], "path": e["filename"], "stage_as": s})
        return s

    first_vendor = add(entry(m, LINK_PAIRS[0]))
    add(entry(m, LINK_OTHERS[0]))
    add(entry(m, LINK_PAIRS[1], "oracle-export"))
    add(entry(m, LINK_OTHERS[1]))
    target = add(entry(m, LINK_PAIRS[0], "oracle-export"))
    add(entry(m, LINK_PAIRS[1]))
    add(entry(m, LINK_OTHERS[2]))
    qs.append(
        question(
            "batch-link-conversion",
            "ms",
            f"Some files in this folder are the same measurement stored twice: an instrument's own data set and a "
            f"conversion of it. Which file holds the same acquisition as `{first_vendor}`? Give its path.",
            "the path of the file",
            {"type": "string", "value": target.split("/", 1)[1], "accept": sorted({target, target.split("/", 1)[1]})},
            lshare,
            [],
            f"corpus/manifest.toml: {LINK_PAIRS[0]} (agilent-masshunter, input) and its depositor mzML (oracle-export)",
        )
    )
    return qs


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true")
    a = ap.parse_args()
    facts = tidy(compute())
    if a.check:
        old = json.loads(FACTS.read_text())
        same = json.dumps(old, sort_keys=True) == json.dumps(facts, sort_keys=True)
        print("evals/facts/batch.json is up to date" if same else "evals/facts/batch.json differs from the files")
        return 0 if same else 1
    FACTS.write_text(json.dumps(facts, indent=1, ensure_ascii=False) + "\n")
    print(f"wrote {FACTS}")
    for q in build_all():
        print(f"{q['id']}: {q['answer']['value']!r}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
