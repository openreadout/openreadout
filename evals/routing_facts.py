"""Questions for one-call routes: the LC photodiode-array detector of a Thermo run, a compensated
event count with two conditions, maximum-intensity projections compared across a folder, and a
Thermo run paired with its mzXML conversion (which records no start time).

Every answer is computed here with third-party readers or taken from a third party's conversion,
never with OpenReadout:

* the PDA values come from GNPS/MassIVE's ProteoWizard mzML of the MTBLS773 run (its `sourceFile`
  SHA-1 equals the raw file's), read with pyteomics (Apache-2.0): the 280 nm column of the PDA
  spectra (`controllerType=4`), cross-checked with the conversion's own `UV 1` chromatogram
  (channel A, 280 nm), which it times one 0.1 s sample later;
* the event count comes from FlowIO (BSD-3) scale values compensated with the file's
  `$SPILLOVER` in NumPy (`oracle/fcs_filter.py` counts the same way);
* the projections from czifile (BSD-3) + NumPy;
* the pairing from `corpus/manifest.toml` (a raw file and its depositor's mzXML share an id).

The values go to `evals/facts/routing.json` (committed; `facts-routing:` sources).

    oracle/.venv/bin/python evals/routing_facts.py          # recompute evals/facts/routing.json
    oracle/.venv/bin/python evals/routing_facts.py --check  # exit 1 if the committed facts differ
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
import analysis as a
import generate as g

OUT = Path(__file__).resolve().parent / "facts" / "routing.json"
DIR = "sample_batch"

PDA_RUN = "mtbls773-001-blank-start"
FCS_COMP = "flowkit-8c-e01"
CD4, CD8 = "CD4 PE-Cy7 FLR-A", "CD8 PerCP-Cy55 FLR-A"
MIP_FILES = [
    ("zenodo7015307-Z-5-CH-2", "stack_1.czi"),
    ("zenodo7015307-T-2-Z-5-CH-2", "stack_2.czi"),
    ("zenodo7015307-T-3-Z-5-CH-2", "stack_3.czi"),
]
LINK_FILES = [
    ("pxd000001-tmt-erwinia-01", "input", "run_001.raw"),
    ("mtbls13930-neg-sqc-5", "oracle-export", "run_002.mzXML"),
    ("pxd000001-tmt-erwinia-01", "oracle-export", "run_003.mzXML"),
    ("mtbls404-Blanc04", "input", "run_004.raw"),
    ("mtbls14508-mh-b4-c18-pos-dda-r01", "oracle-export", "run_005.mzML"),
]


# ---------------------------------------------------------------- readers


def pda_280():
    """(apex time of the 280 nm PDA column in min, wavelength of the largest value of that spectrum,
    apex time of the export's `UV 1` chromatogram in min), pyteomics on the conversion."""
    import numpy as np
    from pyteomics import mzml

    times, col, spectra = [], [], []
    uv = None
    with mzml.MzML(str(a.path(PDA_RUN, "oracle-export")), decode_binary=True) as f:
        for sp in f:
            if not sp["id"].startswith("controllerType=4"):
                continue
            wl = np.asarray(sp["wavelength array"], dtype=np.float64)
            it = np.asarray(sp["intensity array"], dtype=np.float64)
            times.append(float(sp["scanList"]["scan"][0]["scan start time"]))
            col.append(float(it[np.flatnonzero(wl == 280.0)[0]]))
            spectra.append((wl, it))
        for c in f.iterfind("chromatogram"):
            if c["id"] == "UV 1":
                uv = (np.asarray(c["time array"], dtype=np.float64), np.asarray(c["intensity array"]))
    k = int(np.argmax(col))
    wl, it = spectra[k]
    uv_apex = float(uv[0][int(np.argmax(uv[1]))])
    return times[k], float(wl[int(np.argmax(it))]), uv_apex


def comp_count():
    import flowio
    import numpy as np

    fd = flowio.FlowData(str(a.path(FCS_COMP)))
    names = [fd.channels[k]["pnn"] for k in sorted(fd.channels, key=int)]
    x = np.asarray(fd.as_array(preprocess=True), dtype=np.float64)
    key = next(k for k in fd.text if k.lower() in ("spillover", "$spillover", "spill", "$spill"))
    parts = [p.strip() for p in fd.text[key].split(",")]
    n = int(parts[0])
    det = parts[1 : 1 + n]
    m = np.array([float(v) for v in parts[1 + n : 1 + n + n * n]]).reshape(n, n)
    idx = [names.index(d) for d in det]
    x[:, idx] = x[:, idx] @ np.linalg.inv(m)
    mask = (x[:, names.index(CD4)] > 2000) & (x[:, names.index(CD8)] < 500)
    return int(mask.sum()), int(fd.event_count)


def mip_means():
    """Per file: mean over every time point's max-over-z projection of the second channel."""
    import czifile
    import numpy as np

    out = {}
    for cid, name in MIP_FILES:
        with czifile.CziFile(a.path(cid)) as f:
            s = next(f.scenes[k] for k in f.scenes)
            arr = np.asarray(s.asarray())
            dims = list(s.dims)
        c, z = dims.index("C"), dims.index("Z")
        ch = np.take(arr, 1, axis=c)
        z = z if z < c else z - 1
        out[name] = float(ch.max(axis=z).mean(dtype="float64"))
    return out


def compute() -> dict[str, Any]:
    apex, lam, uv_apex = pda_280()
    matched, total = comp_count()
    mips = mip_means()
    best = max(mips, key=mips.get)
    order = sorted(mips.values(), reverse=True)
    data = {
        PDA_RUN: {
            "pda_280_apex_min": {
                "value": apex,
                "reader": a.rd("pyteomics"),
                "how": "argmax of the 280 nm value over the PDA spectra (controllerType=4) of GNPS's mzML",
            },
            "pda_lambda_max_at_apex_nm": {
                "value": lam,
                "reader": a.rd("pyteomics"),
                "how": "wavelength of the largest value of the spectrum at that apex",
            },
            "uv1_apex_min": {
                "value": uv_apex,
                "reader": a.rd("pyteomics"),
                "how": "argmax of the conversion's UV 1 chromatogram (channel A, 280 nm, 9 nm band)",
            },
        },
        FCS_COMP: {
            "comp_cd4_hi_cd8_lo": {
                "value": matched,
                "reader": a.rd("flowio", "numpy"),
                "how": f"scale values, $SPILLOVER compensation, {CD4} > 2000 and {CD8} < 500; of {total}",
            },
        },
        MIP_FILES[0][0]: {
            "brightest_mip_file": {
                "value": best,
                "reader": a.rd("czifile", "numpy"),
                "how": "per file: mean of every time point's max-over-z projection of channel 2; "
                f"means {', '.join(f'{k} {v:.1f}' for k, v in mips.items())}",
                "margin": order[0] / order[1] - 1,
            },
        },
    }
    out: dict[str, Any] = {}
    for cid, facts_ in data.items():
        out[cid] = {
            "extractor": "evals/routing_facts.py",
            "file": g.manifest_entry(g.load_manifest(), cid)["filename"],
            "facts": {k: a.tidy(v) for k, v in facts_.items()},
        }
    return out


def render(data: dict) -> str:
    return json.dumps(data, indent=1, sort_keys=True, ensure_ascii=False) + "\n"


# ---------------------------------------------------------------- questions


def _apex_tolerance(f: dict) -> float:
    # any correct method: the 280 nm PDA column or the 9 nm channel A, a sample or two either way
    return round(max(0.02, abs(f["pda_280_apex_min"] - f["uv1_apex_min"]) + 0.01), 4)


ROUTING_SPECS: list[g.Spec] = [
    g.Spec(
        "ana-ms-raw-pda-280-apex",
        PDA_RUN,
        "analysis",
        "Besides the mass spectrometer, this LC-MS run recorded a photodiode-array (UV-Vis) detector. At what "
        "retention time does the absorbance at 280 nm reach its maximum?",
        lambda f, m: g.number(round(f["pda_280_apex_min"], 4), "min", abs_=_apex_tolerance(f)),
        "facts-routing: pda_280_apex_min (pyteomics on GNPS's msconvert mzML; its UV 1 chromatogram agrees)",
        answer_hint="the retention time in minutes",
    ),
    g.Spec(
        "ana-ms-raw-pda-lambda-max",
        PDA_RUN,
        "analysis",
        "This LC-MS run also recorded UV-Vis absorbance spectra (200-600 nm) with a photodiode-array detector. "
        "Take the spectrum recorded when the absorbance at 280 nm is highest: at which wavelength does that "
        "spectrum absorb most?",
        lambda f, m: g.number(f["pda_lambda_max_at_apex_nm"], "nm", abs_=3),
        "facts-routing: pda_lambda_max_at_apex_nm (pyteomics on GNPS's msconvert mzML)",
        answer_hint="the wavelength in nm",
    ),
    g.Spec(
        "ana-flow-fcs-comp-two-condition-count",
        FCS_COMP,
        "analysis",
        f"Compensate this file's events with the spillover matrix stored in it (on scale values). How many events "
        f"then have {CD4} above 2000 and {CD8} below 500?",
        lambda f, m: g.number(f["comp_cd4_hi_cd8_lo"], None, abs_=0),
        "facts-routing: comp_cd4_hi_cd8_lo (flowio + NumPy)",
        answer_hint="the number of events",
    ),
]


def build_batch(manifest: dict) -> list[dict]:
    data = g.load_facts("facts-routing")
    out = []
    cid = MIP_FILES[0][0]
    fact = data[cid]["facts"]["brightest_mip_file"]
    out.append(
        {
            "id": "batch-mic-czi-brightest-mip",
            "family": "microscopy",
            "format": "share",
            "category": "batch",
            "file": {"corpus_id": "share", "path": "", "stage_as": DIR},
            "share": [
                {"corpus_id": c, "path": g.manifest_entry(manifest, c)["filename"], "stage_as": f"{DIR}/{n}"}
                for c, n in MIP_FILES
            ],
            "generated": [],
            "question": "The folder holds three z-stacks (`sample_batch/stack_*.czi`, two channels each, some with "
            "several time points). For each file, make a maximum-intensity projection of the second channel along z "
            "(for every x/y position and time point, the brightest value over the z-slices) and average the "
            "projected values over all pixels and time points. Which file has the highest average?",
            "answer_hint": "the file name",
            "answer": {
                "type": "string",
                "value": fact["value"],
                "accept": [fact["value"], f"{DIR}/{fact['value']}"],
                "reject": [n for _, n in MIP_FILES if n != fact["value"]],
            },
            "source": f"evals/facts/routing.json [{cid}] (evals/routing_facts.py) — facts-routing: brightest_mip_file "
            f"({fact['reader']})",
        }
    )
    share_ = []
    for c, role, name in LINK_FILES:
        e = g.manifest_entry(manifest, c, None if role == "input" else role)
        share_.append({"corpus_id": c, "path": e["filename"], "stage_as": f"{DIR}/{name}"})
    out.append(
        {
            "id": "batch-link-raw-mzxml",
            "family": "ms",
            "format": "share",
            "category": "batch",
            "file": {"corpus_id": "share", "path": "", "stage_as": DIR},
            "share": share_,
            "generated": [],
            "question": "One of the mzXML/mzML files in this folder is a conversion of `sample_batch/run_001.raw` "
            "(the same acquisition). Which one? Give its path.",
            "answer_hint": "the path of the file",
            "answer": {
                "type": "string",
                "value": "run_003.mzXML",
                "accept": ["run_003.mzXML", f"{DIR}/run_003.mzXML"],
                "reject": ["run_002.mzXML", "run_005.mzML"],
            },
            "source": "evals/facts/routing.json (evals/routing_facts.py) — corpus/manifest.toml: "
            "pxd000001-tmt-erwinia-01 (thermo-raw, input) and its depositor mzXML "
            "(oracle-export; ProteoWizard 2.2.3017, no start time recorded)",
        }
    )
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true", help="exit 1 if evals/facts/routing.json is out of date")
    args = ap.parse_args()
    text = render(compute())
    if args.check:
        if not OUT.exists() or OUT.read_text(encoding="utf-8") != text:
            print("evals/facts/routing.json is out of date; run evals/routing_facts.py", file=sys.stderr)
            return 1
        print("evals/facts/routing.json is up to date")
        return 0
    OUT.write_text(text, encoding="utf-8")
    print(f"wrote {OUT}")
    print(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
