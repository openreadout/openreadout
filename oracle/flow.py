#!/usr/bin/env python
"""Ground truth for flow analysis: FlowJo workspaces, Gating-ML, compensation, transforms.

Usage: uv run python flow.py            (writes ../corpus/oracle/flow/<id>.json for every job below)
       uv run python flow.py ID [...]   (only these jobs)

Runs FlowKit 1.3 and flowutils 1.2 (both BSD-3, Scott White) as black boxes:

* gating jobs (`<gating id>.json`): FlowKit parses the workspace (`parse_wsp`) or Gating-ML
  document (`parse_gating_xml`), gates each paired FCS file with `GatingStrategy.gate_sample`
  (FlowKit's Sample reads events with FlowIO and applies $PnE/$PnG/$TIMESTEP scaling, the same
  scale values openreadout gates), and every population's count is recorded under
  openreadout's path convention (`/A/B`; a quadrant's path includes its quadrant gate). The
  count FlowJo stored in the workspace (`count` attributes) is read here with lxml, walking the
  XML independently of both FlowKit and openreadout, and recorded as `flowjo_count`.
* compensation jobs (`<fcs id>.comp.json`): FlowKit `Sample.apply_compensation` with the FCS
  file's own spillover keyword ($SPILLOVER, $SPILL or SPILL, parsed with flowutils' get_spill)
  or a workspace sample's matrix, then FlowKit's transform classes applied to the compensated
  values of the recorded rows (logicle, arcsinh, hyperlog, log, linear, FlowJo biex and log) and
  NumPy's arcsinh(x / cofactor). Rows: the first 100 events and evenly spaced events up to 1000.
"""
import json
import os
import sys
import tomllib
from pathlib import Path

import numpy as np
import flowkit as fk
import flowutils
from lxml import etree

sys.path.insert(0, str(Path(__file__).resolve().parent))
import oracle_json  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
FILES = Path(os.environ.get("OPENREADOUT_CORPUS_DIR", ROOT / "corpus" / "files")).resolve()
OUT = ROOT / "corpus" / "oracle" / "flow"

with open(ROOT / "corpus" / "manifest.toml", "rb") as f:
    MANIFEST = {e["id"]: e for e in tomllib.load(f)["file"]}


def path_of(corpus_id):
    return FILES / MANIFEST[corpus_id]["filename"]


# (gating id, [(fcs id, workspace sample name or None)])
GATING_JOBS = [
    ("flowkit-line-gates", [("flowkit-line", "data_set_simple_line_100.fcs")]),
    ("flowkit-line-ellipse", [("flowkit-line", "data_set_simple_line_100.fcs")]),
    ("flowkit-diamond-quad", [("flowkit-diamond", "test_data_diamond_01.fcs")]),
    ("flowkit-diamond-asinh", [("flowkit-diamond", "test_data_diamond_01.fcs")]),
    ("flowkit-diamond-biex", [("flowkit-diamond", "test_data_diamond_01.fcs")]),
    ("flowkit-gml-all", [("flowkit-gml-events", None)]),
]
EIGHT = [
    ("flowkit-8c-e01", "101_DEN084Y5_15_E01_008_clean.fcs"),
    ("flowkit-8c-e03", "101_DEN084Y5_15_E03_009_clean.fcs"),
    ("flowkit-8c-e05", "101_DEN084Y5_15_E05_010_clean.fcs"),
]
for wsp in ("flowkit-8c-ics", "flowkit-8c-ellipse", "flowkit-8c-bool", "flowkit-8c-quad"):
    GATING_JOBS.append((wsp, EIGHT))
GATING_JOBS.append(("zenodo19221995-celeidoscope-wsp", [("zenodo19221995-facsdiscover-zam36", "Zam36 YFP.fcs")]))
# FlowJo 10.0.7 to 10.8.1 workspaces from five further depositors, each with one or two of its samples
GATING_JOBS += [
    ("zenodo14537941-hoxa13-wsp", [("zenodo14537941-tomato-1-003-fcs", "Specimen_001_tomato+ 1_003.fcs")]),
    ("zenodo14780093-20221214-wsp", [("zenodo14780093-msc-kit-90-fcs", "MSC kit_90.fcs"),
                                     ("zenodo14780093-msc-kit-nm-fcs", "MSC kit_NM.fcs")]),
    ("zenodo7941894-icam1-wsp", [("zenodo7941894-ns-dapi-fcs", "Specimen_001_ns dapi.fcs"),
                                 ("zenodo7941894-b1-fcs", "Specimen_001_B1.fcs")]),
    ("zenodo15023230-fig1-wsp", [("zenodo15023230-media-059-fcs", "06042020_media_059.fcs"),
                                 ("zenodo15023230-combo-only-049-fcs", "06042020_COMBO ONLY_049.fcs")]),
]

# (fcs id, matrix source): "fcs" = the file's own spillover keyword; ("wsp", id, sample) = that
# workspace sample's matrix.
COMP_JOBS = [
    ("fcsparser-facs-diva", "fcs"),
    ("fcsparser-fortessa-a01", "fcs"),
    ("fcsparser-hts-lsr-ii-d06", "fcs"),
    ("flowio-100715", "fcs"),
    ("flowkit-comp-example", "fcs"),
    ("zenodo18439538-cytoflex", "fcs"),
    ("zenodo17457137-aurora-beads", "fcs"),
    ("flowkit-8c-e01", ("wsp", "flowkit-8c-ics", "101_DEN084Y5_15_E01_008_clean.fcs")),
    ("zenodo19221995-facsdiscover-zam36", ("wsp", "zenodo19221995-celeidoscope-wsp", "Zam36 YFP.fcs")),
]

# (openreadout spec, FlowKit/NumPy callable)
TRANSFORMS = [
    ("logicle", lambda: fk.transforms.LogicleTransform(262144, 0.5, 4.5, 0)),
    ("logicle:T=262144,W=1,M=4.418539922,A=0", lambda: fk.transforms.LogicleTransform(262144, 1, 4.418539922, 0)),
    ("arcsinh", lambda: fk.transforms.AsinhTransform(262144, 4.5, 0)),
    ("arcsinh:T=262144,M=4,A=1", lambda: fk.transforms.AsinhTransform(262144, 4, 1)),
    ("hyperlog", lambda: fk.transforms.HyperlogTransform(262144, 0.5, 4.5, 0)),
    ("log", lambda: fk.transforms.LogTransform(262144, 4.5)),
    ("linear:T=262144,A=100", lambda: fk.transforms.LinearTransform(262144, 100)),
    ("biex", lambda: fk.transforms.WSPBiexTransform(0, -10, 4.41854, 262144)),
    ("biex:neg=1,width=-100,pos=4.5,maxRange=262144", lambda: fk.transforms.WSPBiexTransform(1, -100, 4.5, 262144)),
    ("flowjo-log:offset=1,decades=4.5", lambda: fk.transforms.WSPLogTransform(1, 4.5)),
    ("arcsinh-cofactor:5", None),
    ("arcsinh-cofactor:150", None),
]


def num(v):
    v = float(v)
    return v if np.isfinite(v) else None


def flowjo_counts(wsp_path):
    """sample name -> {path: stored count}, walked directly from the XML."""
    tree = etree.parse(str(wsp_path))
    out = {}
    for sample in tree.getroot().iter("Sample"):
        node = sample.find("SampleNode")
        if node is None:
            continue
        counts = {}

        def walk(el, prefix):
            sub = el.find("Subpopulations")
            if sub is None:
                return
            for child in sub:
                if not isinstance(child.tag, str) or child.tag not in ("Population", "AndNode", "OrNode", "NotNode"):
                    continue
                path = prefix + "/" + child.get("name")
                c = child.get("count")
                if c is not None and int(c) >= 0:
                    counts[path] = int(c)
                walk(child, path)

        walk(node, "")
        out[node.get("name")] = counts
    return out


def our_path(row):
    gp = [p for p in row["gate_path"] if p != "root"]
    if row["quadrant_parent"] is not None and isinstance(row["quadrant_parent"], str):
        gp.append(row["quadrant_parent"])
    return "/" + "/".join(gp + [row["gate_name"]])


def gating_job(gating_id, pairs):
    gpath = path_of(gating_id)
    is_wsp = MANIFEST[gating_id]["format"] == "flowjo-wsp"
    out = {
        "generator": f"flowkit {fk.__version__} (flowutils {flowutils.__version__})",
        "gating": gating_id,
        "format": MANIFEST[gating_id]["format"],
        "samples": [],
    }
    if is_wsp:
        wsp = fk.parse_wsp(str(gpath))
        stored = flowjo_counts(gpath)
    else:
        strategy = fk.parse_gating_xml(str(gpath))
    for fcs_id, sample_name in pairs:
        fpath = path_of(fcs_id)
        if not fpath.exists():
            print(f"skip {gating_id}/{fcs_id}: {fpath} missing")
            continue
        if is_wsp and sample_name not in wsp["samples"]:
            continue
        sid = sample_name or fcs_id
        sample = fk.Sample(str(fpath), sample_id=sid, use_flowjo_labels=is_wsp, subsample=0, ignore_offset_error=True)
        gs = wsp["samples"][sample_name]["gating_strategy"] if is_wsp else strategy
        res = gs.gate_sample(sample)
        pops = []
        for _, row in res.report.iterrows():
            p = our_path(row)
            entry = {"path": p, "count": int(row["count"])}
            if is_wsp and p in stored.get(sample_name, {}):
                entry["flowjo_count"] = stored[sample_name][p]
            pops.append(entry)
        pops.sort(key=lambda e: e["path"])
        out["samples"].append({
            "fcs": fcs_id,
            "sample": sample_name,
            "event_count": int(sample.event_count),
            "populations": pops,
        })
    return out


def sample_rows(n):
    first = list(range(min(n, 100)))
    rest = np.linspace(100, n - 1, num=max(0, min(n - 100, 900)), dtype=int).tolist() if n > 100 else []
    return sorted(set(first + rest))


def comp_job(fcs_id, source):
    fpath = path_of(fcs_id)
    sample = fk.Sample(str(fpath), subsample=0, ignore_offset_error=True)
    if source == "fcs":
        meta = sample.get_metadata()
        key = next(k for k in ("spillover", "spill") if k in meta)
        matrix, markers = flowutils.compensate.get_spill(meta[key])
        m = fk.Matrix(matrix, detectors=markers)
        how = {"source": "fcs", "keyword": key}
    else:
        _, wsp_id, sample_name = source
        sample = fk.Sample(str(fpath), subsample=0, use_flowjo_labels=True, ignore_offset_error=True)
        comp = fk.parse_wsp(str(path_of(wsp_id)))["samples"][sample_name]["compensation"]
        m = comp["matrix"]
        how = {"source": "workspace", "workspace": wsp_id, "sample": sample_name, "matrix": comp["matrix_name"]}
    sample.apply_compensation(m)
    comp_events = sample.get_events(source="comp")
    n = comp_events.shape[0]
    rows = sample_rows(n)
    # A spectral (OLS) matrix writes its unmixed values into the columns of its rows.
    detectors = list(getattr(m, "true_detectors", None) or m.detectors)
    idx = [sample.pnn_labels.index(d) for d in detectors]
    out = {
        "generator": f"flowkit {fk.__version__} (flowutils {flowutils.__version__})",
        "fcs": fcs_id,
        "compensation": how,
        "event_count": int(n),
        "rows": rows,
        "detectors": detectors,
        "compensated": {d: [num(v) for v in comp_events[rows, i]] for d, i in zip(detectors, idx)},
        "column_sums": {d: num(comp_events[:, i].sum()) for d, i in zip(detectors, idx)},
        "transforms": [],
    }
    # Transforms on the compensated values of the first two detectors.
    for spec, make in TRANSFORMS:
        vals = {}
        for d, i in list(zip(detectors, idx))[:2]:
            x = comp_events[rows, i].astype(np.float64)
            if make is None:
                cof = float(spec.split(":")[1])
                y = np.arcsinh(x / cof)
            else:
                y = make().apply(x.reshape(-1, 1)).reshape(-1)
            vals[d] = [num(v) for v in y]
        out["transforms"].append({"spec": spec, "values": vals})
    return out


def write(name, data):
    OUT.mkdir(parents=True, exist_ok=True)
    p = oracle_json.write_text(OUT / f"{name}.json", json.dumps(data, indent=1) + "\n")  # over 1 MiB: .json.gz
    print("wrote", p.relative_to(ROOT))


def main():
    only = set(sys.argv[1:])
    for gid, pairs in GATING_JOBS:
        if only and gid not in only:
            continue
        if not path_of(gid).exists():
            print(f"skip {gid}: not downloaded")
            continue
        write(gid, gating_job(gid, pairs))
    for fid, source in COMP_JOBS:
        if only and fid not in only:
            continue
        if not path_of(fid).exists():
            print(f"skip {fid}: not downloaded")
            continue
        write(f"{fid}.comp", comp_job(fid, source))


if __name__ == "__main__":
    main()
