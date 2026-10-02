"""Questions for the agent-surface features: one colour channel of an RGB image, a
maximum-intensity projection of one stage position, and an analysis command run over a folder
with a sample sheet (peaks per GC run, summarized by group).

Every answer is computed here with third-party readers or taken from the vendor's own report,
never with OpenReadout: czifile (BSD-3) and nd2 (BSD-3) with NumPy for the pixel values
(pylibCZIrw, LGPL, run as a black box, cross-checks the CZI value), and the ChemStation
`Report.TXT` each GC injection stores (the vendor's own integration; the agent gets only the
`.ch` signal files) for the peak areas.

The values go to `evals/facts/agent.json` (committed; `facts-agent:` sources), so the questions
regenerate without the corpus.

    oracle/.venv/bin/python evals/agent_facts.py          # recompute evals/facts/agent.json
    oracle/.venv/bin/python evals/agent_facts.py --check  # exit 1 if the committed facts differ
"""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
import analysis as a
import generate as g

OUT = Path(__file__).resolve().parent / "facts" / "agent.json"
DIR = "sample_batch"

# GC runs of one method (the FID signal of each injection) and the lab's sheet: which lot each is.
GC_RUNS = [
    ("chromhandler-001f0101-d", "run_001.ch", "lot A"),
    ("chromhandler-001f0102-d", "run_002.ch", "lot B"),
    ("chromhandler-001f0103-d", "run_003.ch", "lot A"),
]


# ---------------------------------------------------------------- readers


def czi_rgb_mean(cid: str, component: int):
    """Mean of one colour component (0 red, 1 green, 2 blue) over the whole image, czifile
    (which swaps the CZI's stored B, G, R to R, G, B) + NumPy."""
    import czifile
    import numpy as np

    with czifile.CziFile(a.path(cid)) as f:
        s = next(f.scenes[k] for k in f.scenes)
        arr = np.asarray(s.asarray())
        dims = list(s.dims)
    arr = np.moveaxis(arr, dims.index("S"), -1)
    return float(arr[..., component].mean(dtype="float64")), a.rd("czifile")


def czi_rgb_mean_cross(cid: str, component: int):
    """The same with pylibCZIrw (LGPL, black box), which returns B, G, R."""
    from pylibCZIrw import czi as pyczi

    with pyczi.open_czi(str(a.path(cid))) as d:
        arr = d.read(plane={"C": 0, "Z": 0, "T": 0})
    return float(arr[..., 2 - component].mean(dtype="float64")), a.rd("pylibCZIrw") + " (black box)"


def nd2_rgb_mean(cid: str, component: int):
    import nd2
    import numpy as np

    with nd2.ND2File(a.path(cid)) as f:
        arr = np.asarray(f.asarray())
        axis = list(f.sizes).index("S")
    # The nd2 package returns colour-camera samples in stored order, B, G, R (sample 0 is the one a
    # DAPI plane lights; its own pseudo-wavelengths are 420/515/590 nm): reverse to R, G, B.
    arr = np.moveaxis(arr, axis, -1)[..., ::-1]
    return float(arr[..., component].mean(dtype="float64")), a.rd("nd2")


def nd2_mip_mean(cid: str, position: int, channel: int, t: int):
    """Mean of the max-over-z projection of one stage position, channel and time point, nd2 + NumPy."""
    import nd2
    import numpy as np

    with nd2.ND2File(a.path(cid)) as f:
        arr = np.asarray(f.asarray())
        dims = list(f.sizes)
    arr = np.moveaxis(arr, [dims.index(d) for d in ("P", "T", "C", "Z")], [0, 1, 2, 3])
    mip = arr[position, t, channel].max(axis=0)
    return float(mip.mean(dtype="float64")), a.rd("nd2")


def gc_lot_mean_area(lot: str):
    """Per GC run the area of its largest FID peak from the vendor's Report.TXT, averaged over the
    runs the sheet assigns to `lot`."""
    import quant

    areas = [quant.main_area(dir_id, "FID1 A")[0]["area"] for dir_id, _, grp in GC_RUNS if grp == lot]
    return sum(areas) / len(areas), quant.REPORT


# ---------------------------------------------------------------- facts


@dataclass
class Fact:
    corpus_id: str
    name: str
    how: str
    compute: Callable[[], tuple[Any, str]]
    cross: Callable[[], tuple[Any, str]] | None = None
    cross_rel: float = 1e-9


FACTS: list[Fact] = [
    Fact(
        "aics-RGB-8bit",
        "green_mean",
        "RGB (Bgr24) image, the green samples of every pixel, mean",
        lambda: czi_rgb_mean("aics-RGB-8bit", 1),
        lambda: czi_rgb_mean_cross("aics-RGB-8bit", 1),
    ),
    Fact(
        "aics-ND2-dims-rgb",
        "blue_mean",
        "RGB image, the blue samples of every pixel, mean",
        lambda: nd2_rgb_mean("aics-ND2-dims-rgb", 2),
    ),
    Fact(
        "aics-ND2-dims-p4z5t3c2y32x32",
        "mip_mean_p3_c2_t1",
        "stage position 3 (P index 2), second channel (C index 1), first time point: max over the 5 z-slices "
        "per pixel, mean of the projection",
        lambda: nd2_mip_mean("aics-ND2-dims-p4z5t3c2y32x32", 2, 1, 0),
    ),
    Fact(
        "chromhandler-001f0101-d-fid1a-ch",
        "gc_lot_a_mean_main_area",
        "runs 001F0101 and 001F0103 (lot A in the sheet): area of the largest FID1 A peak in each run's "
        "Report.TXT, pA·s, averaged",
        lambda: gc_lot_mean_area("lot A"),
    ),
]


def compute() -> dict[str, Any]:
    out: dict[str, Any] = {}
    for f in FACTS:
        value, reader = f.compute()
        rec: dict[str, Any] = {"value": value, "reader": reader, "how": f.how}
        if f.cross is not None:
            other, who = f.cross()
            if abs(other - value) > f.cross_rel * max(abs(value), 1.0):
                raise SystemExit(f"{f.corpus_id} {f.name}: {value} disagrees with {who}: {other}")
            rec["cross_check"] = f"{who}: agrees"
        entry = out.setdefault(
            f.corpus_id,
            {
                "extractor": "evals/agent_facts.py",
                "file": g.manifest_entry(g.load_manifest(), f.corpus_id)["filename"],
                "facts": {},
            },
        )
        entry["facts"][f.name] = a.tidy(rec)
    return out


def render(data: dict) -> str:
    return json.dumps(data, indent=1, sort_keys=True, ensure_ascii=False) + "\n"


# ---------------------------------------------------------------- questions

HINT_RAW = "a number (raw values as stored in the file)"

AGENT_SPECS: list[g.Spec] = [
    g.Spec(
        "ana-mic-czi-rgb-green-mean",
        "aics-RGB-8bit",
        "analysis",
        "This is a colour (RGB) microscope image. What is the mean raw value of its green channel over the "
        "whole image?",
        lambda f, m: g.number(f["green_mean"], None, rel=0.001),
        "facts-agent: green_mean (czifile; pylibCZIrw agrees)",
        answer_hint=HINT_RAW,
    ),
    g.Spec(
        "ana-mic-nd2-rgb-blue-mean",
        "aics-ND2-dims-rgb",
        "analysis",
        "This Nikon file holds a colour (RGB) image. What is the mean raw value of its blue channel?",
        lambda f, m: g.number(f["blue_mean"], None, rel=0.001),
        "facts-agent: blue_mean (nd2)",
        answer_hint=HINT_RAW,
    ),
    g.Spec(
        "ana-mic-nd2-position-mip-mean",
        "aics-ND2-dims-p4z5t3c2y32x32",
        "analysis",
        "This experiment imaged several stage positions as z-stacks over time. For the third stage position "
        "(counting from 1), make a maximum-intensity projection of the second channel along z (for every x/y "
        "position, the brightest value over the z-slices) at the first time point. What is the mean raw pixel "
        "value of that projection?",
        lambda f, m: g.number(f["mip_mean_p3_c2_t1"], None, rel=0.001),
        "facts-agent: mip_mean_p3_c2_t1 (nd2)",
        answer_hint=HINT_RAW,
    ),
]


def build_batch(manifest: dict) -> list[dict]:
    """The folder question: three GC FID signals plus a sheet naming each run's lot."""
    data = g.load_facts("facts-agent")
    cid = "chromhandler-001f0101-d-fid1a-ch"
    fact = data[cid]["facts"]["gc_lot_a_mean_main_area"]
    share_ = [
        {
            "corpus_id": f"{d}-fid1a-ch",
            "path": g.manifest_entry(manifest, f"{d}-fid1a-ch")["filename"],
            "stage_as": f"{DIR}/{name}",
        }
        for d, name, _ in GC_RUNS
    ]
    sheet = "run,lot\n" + "".join(f"{name.rsplit('.', 1)[0]},{lot}\n" for _, name, lot in GC_RUNS)
    return [
        {
            "id": "batch-gc-lot-main-peak-area",
            "family": "chromatography",
            "format": "share",
            "category": "batch",
            "file": {"corpus_id": "share", "path": "", "stage_as": DIR},
            "share": share_,
            "generated": [{"stage_as": f"{DIR}/runs.csv", "content": sheet}],
            "question": "The folder holds the FID signals of three GC runs (`sample_batch/run_*.ch`) and a sheet "
            "`sample_batch/runs.csv` giving the lot each run belongs to. Integrate each run's peaks and take the "
            "area of its largest peak, in pA·s (picoampere-seconds); then average those areas over the runs of "
            "lot A. What is that average?",
            "answer_hint": "a number (peak area in pA·s)",
            "answer": {"type": "number", "value": fact["value"], "unit": None, "tolerance": {"rel": 0.05}},
            "source": f"evals/facts/agent.json [{cid}] (evals/agent_facts.py) — facts-agent: "
            f"gc_lot_a_mean_main_area ({fact['reader']})",
        }
    ]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true", help="exit 1 if evals/facts/agent.json is out of date")
    args = ap.parse_args()
    text = render(compute())
    if args.check:
        if not OUT.exists() or OUT.read_text(encoding="utf-8") != text:
            print("evals/facts/agent.json is out of date; run evals/agent_facts.py", file=sys.stderr)
            return 1
        print("evals/facts/agent.json is up to date")
        return 0
    OUT.write_text(text, encoding="utf-8")
    print(f"wrote {OUT}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
