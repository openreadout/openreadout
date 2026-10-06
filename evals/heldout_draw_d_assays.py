"""Held-out draw D (2026-10-06): plate readers, qPCR and the bench instruments.

Part of evals/heldout_draw_d.py; the questions were written from the held-out oracles, the
depositors' exports and records, and facts computed with third-party readers, before OpenReadout
was run on any of these files.

Some facts use readers the shared oracle venv does not have (geddes for diffraction, pyNGB for
NETZSCH files). ASSAYS_SITE names a site-packages directory that holds them (the assays agent used
`.venv-heldout-d-assays/lib/python3.12/site-packages`, built from oracle/pyproject.toml's `series`
group); it is appended to sys.path, so the oracle venv's own packages still come first.
"""

from __future__ import annotations

import os
import re
import sys
from typing import Any

C: dict[str, str] = {
    # diffraction
    "d_assays_xrdml": "ho-zenodo21263727-xrd-xrdml-e225dhb",
    "d_assays_brml": "ho-zenodo15591123-xrd-brml-gid",
    "d_assays_raw": "ho-zenodo17744373-xrd-raw-hnt",
    "d_assays_ras": "ho-zenodo14997537-xrd-ras-tmfeo3",
    # EPR
    "d_assays_efs": "ho-zenodo4521882-epr-bes3t-efs",
    # electrochemistry
    "d_assays_mpr": "ho-zenodo3631173-echem-mpr-lsv",
    "d_assays_mpt": "ho-zenodo13271360-echem-mpt-0016",
    "d_assays_dta": "ho-zenodo13772144-echem-dta-chronop",
    "d_assays_nda": "ho-gh-fthuld-neware-nda-old",
    "d_assays_ndax": "ho-gh-empaeconversion-fastnda-ndax-nw4",
    # thermal analysis
    "d_assays_ngb": "ho-zenodo17744373-ngb-dsu-hnt",
    "d_assays_ta": "ho-zenodo17294879-ta-dsc-pdlla",
    # calorimetry, SPR
    "d_assays_itc": "ho-zenodo10988523-itc-gb1-p6",
    "d_assays_blr": "ho-dataverse-srwyye-biacore-blr-screen2",
    "d_assays_bme": "ho-dataverse-xhrsgm-biacore-bme-3bnc117",
    # qPCR
    "d_assays_rdml": "ho-borealis-gfmpyu-rdml-plate1",
    "d_assays_eds": "ho-figshare29367848-eds-gastric",
    # flow cytometry
    "d_assays_fcs_ww": "ho-zenodo20429107-fcs-wastewater",
    "d_assays_fcs_ev": "ho-zenodo17153347-fcs-ev-dpbs",
    # plate readers
    "d_assays_bmg": "ho-gh-pilizota-plate-clariostar-growth",
    "d_assays_gen5": "ho-gh-txtltradeoff-plate-gen5-salt",
}


# ---------------------------------------------------------------- facts (oracle venv only)


def _h():
    import heldout

    return heldout


def _site() -> None:
    p = os.environ.get("ASSAYS_SITE")
    if p and p not in sys.path:
        sys.path.append(p)


def _argmax_x(x, y) -> float:
    i = max(range(len(y)), key=y.__getitem__)
    s = sorted(y)
    assert s[-1] > s[-2], "the largest value is not unique"
    return float(x[i])


def geddes_peak(cid: str):
    """2θ (degrees) of the largest intensity, geddes (MIT, black box)."""
    _site()
    import geddes

    d = geddes.read(str(_h().hpath(cid)))
    return _argmax_x(list(d.x), list(d.y)), _h().rd("geddes")


def geddes_step(cid: str):
    """2θ step (degrees) between consecutive points, geddes (MIT, black box)."""
    _site()
    import geddes

    x = list(geddes.read(str(_h().hpath(cid))).x)
    return (x[-1] - x[0]) / (len(x) - 1), _h().rd("geddes")


def xrdml_xy(cid: str):
    """(2θ, counts) from the XRDML XML itself (standard library): positions from start/end, counts."""
    import xml.etree.ElementTree as ET

    root = ET.parse(_h().hpath(cid)).getroot()
    ns = {"x": root.tag.split("}")[0].strip("{")}
    dp = root.find(".//x:dataPoints", ns)
    pos = next(p for p in dp.findall("x:positions", ns) if p.get("axis") == "2Theta")
    start, end = float(pos.find("x:startPosition", ns).text), float(pos.find("x:endPosition", ns).text)
    counts = dp.find("x:counts", ns)
    if counts is None:
        counts = dp.find("x:intensities", ns)
    y = [float(v) for v in counts.text.split()]
    x = [start + i * (end - start) / (len(y) - 1) for i in range(len(y))]
    return x, y


def xrdml_step_stdlib(cid: str):
    x, _ = xrdml_xy(cid)
    return (x[-1] - x[0]) / (len(x) - 1), "Python ElementTree on the XRDML"


def uxd_peak(cid: str):
    """2θ of the largest count in the depositor's UXD conversion of the RAW file."""
    text = _h().hpath(cid, "oracle-export").read_text(errors="replace").splitlines()
    k = next(i for i, line in enumerate(text) if line.strip().startswith("_2THETACOUNTS"))
    x, y = [], []
    for line in text[k + 1 :]:
        f = line.split()
        if len(f) < 2 or line.startswith(("_", ";")):
            continue
        x.append(float(f[0]))
        y.append(float(f[1]))
    return _argmax_x(x, y), "Python on the depositor's UXD export"


def efs_peak_field(cid: str, manual: bool = False):
    """Field (G) where the real part of the echo-detected field sweep is largest."""
    p = _h().hpath(cid)
    if manual:
        import numpy as np

        keys = {}
        for line in p.read_text(errors="replace").splitlines():
            f = line.split(None, 1)
            if len(f) == 2 and f[0] in ("XMIN", "XWID", "XPTS", "BSEQ", "IRFMT", "IKKF"):
                keys[f[0]] = f[1].strip().strip("'")
        n = int(keys["XPTS"])
        assert keys["BSEQ"] == "BIG" and keys["IRFMT"] == "D" and keys["IKKF"] == "CPLX", keys
        raw = np.fromfile(p.with_suffix(".DTA"), dtype=">f8")
        real = raw[0::2][:n]
        x = [float(keys["XMIN"]) + i * float(keys["XWID"]) / (n - 1) for i in range(n)]
        return _argmax_x(x, [float(v) for v in real]), "NumPy on the DSC keys and the DTA (big-endian float64, complex)"
    sys.path.insert(0, str(_h().ROOT / "oracle" / "third_party"))
    import numpy as np
    from deerload import deerload

    ab, data, _ = deerload(str(p), full_output=True)
    x = np.asarray(ab, dtype="float64").ravel() * 1e3  # deerload divides abscissae by 1000
    return _argmax_x([float(v) for v in x], [float(v) for v in np.real(data)]), "DeerLab deerload (vendored, MIT)"


def mpt_max(cid: str, column: str, pandas: bool = False):
    """Largest value of one column of an EC-Lab text export."""
    p = _h().hpath(cid)
    lines = p.read_bytes().decode("latin-1").splitlines()
    nh = int(lines[1].split(":")[1])
    if pandas:
        import io

        import pandas as pd

        df = pd.read_csv(io.StringIO("\n".join(lines[nh - 1 :])), sep="\t")
        return float(df[column].max()), _h().rd("pandas")
    head = lines[nh - 1].split("\t")
    k = head.index(column)
    vals = [float(r.split("\t")[k]) for r in lines[nh:] if r.strip()]
    return max(vals), "Python on the EC-Lab text export"


def bmg_top_final_well(cid: str, pandas: bool = False):
    """Well whose absorbance reaches the highest value over all reads of a BMG table export."""
    import csv

    p = _h().hpath(cid)
    rows = list(csv.reader(p.read_text(errors="replace").splitlines()))
    k = next(i for i, r in enumerate(rows) if r and r[0] == "Well")
    data = [r for r in rows[k + 2 :] if r and re.fullmatch(r"[A-P]\d{2}", r[0])]
    if pandas:
        import pandas as pd

        df = pd.DataFrame([[float(v) for v in r[2:] if v.strip()] for r in data], index=[r[0] for r in data])
        s = df.max(axis=1).sort_values()
        assert s.iloc[-1] > s.iloc[-2]
        return str(s.index[-1]), _h().rd("pandas")
    last = {r[0]: max(float(v) for v in r[2:] if v.strip()) for r in data}
    s = sorted(last, key=last.get)
    assert last[s[-1]] > last[s[-2]]
    return s[-1], "Python csv on the export text"


def gen5_block(cid: str) -> tuple[list[str], list[list[str]]]:
    """Header and rows of the first kinetic block (first gain) of the depositor's Gen5 CSV export."""
    import csv

    rows = list(csv.reader(_h().hpath(cid, "oracle-export").read_text(errors="replace").splitlines()))
    k = next(i for i, r in enumerate(rows) if r and r[0] == "Time" and len(r) > 2 and "deGFP" in r[1])
    head = rows[k]
    body = []
    for r in rows[k + 1 :]:
        if not r or not r[0].strip():
            break
        body.append(r)
    return head, body


def gen5_top_final_well(cid: str, pandas: bool = False):
    """Well with the highest deGFP fluorescence at the last read (first gain), depositor's Gen5 export."""
    head, body = gen5_block(cid)
    wells = [h for h in head[2:] if h.strip()]
    last = body[-1]
    if pandas:
        import pandas as pd

        s = pd.Series([float(v) for v in last[2 : 2 + len(wells)]], index=wells).sort_values()
        assert s.iloc[-1] > s.iloc[-2]
        return str(s.index[-1]), _h().rd("pandas")
    vals = {w: float(v) for w, v in zip(wells, last[2:], strict=False)}
    s = sorted(vals, key=vals.get)
    assert vals[s[-1]] > vals[s[-2]]
    return s[-1], "Python csv on the depositor's Gen5 export"


def gen5_reads(cid: str):
    """Kinetic read count from the protocol line of the depositor's Gen5 export."""
    text = _h().hpath(cid, "oracle-export").read_text(errors="replace")
    (n,) = re.findall(r"Start Kinetic,\"[^\"]*?(\d+) Reads", text)
    _, body = gen5_block(cid)
    assert len(body) == int(n), (len(body), n)
    return int(n), "Python on the depositor's Gen5 export (protocol line and kinetic block)"


def ngb_duration_min(cid: str, export: bool = False):
    """Duration of the sample run (min): pyNGB's last time, or the Proteus export's last time."""
    if export:
        rows = [
            line.split(",")
            for line in _h().hpath(cid, "oracle-export").read_text(errors="replace").splitlines()
            if line and not line.startswith("#") and line.strip()
        ]
        return float(rows[-1][1]), "Python on the depositor's Proteus export"
    _site()
    import pyngb

    t = pyngb.read_ngb(str(_h().hpath(cid)))
    return float(t.column("time").to_pylist()[-1]) / 60.0, _h().rd("pyngb")


def lowest_cq(o: dict, key: str, target: str | None = None) -> str:
    """`key` (well or sample) of the record with the lowest vendor Cq."""
    r = [x for x in o["records"] if x.get("cq") is not None and (target is None or x["target"] == target)]
    r.sort(key=lambda x: x["cq"])
    assert r[1]["cq"] - r[0]["cq"] > 0.01 or r[0][key] == r[1][key], r[:2]
    return str(r[0][key])


def facts(Fact, H) -> list:
    """The analysis-tier facts of the assays area (computed in the oracle venv, ASSAYS_SITE set)."""
    return [
        Fact(
            C["d_assays_xrdml"],
            "step_2theta",
            "2θ step (degrees) between consecutive points",
            lambda: geddes_step(C["d_assays_xrdml"]),
            lambda: xrdml_step_stdlib(C["d_assays_xrdml"]),
            rel=1e-6,
        ),
        Fact(
            C["d_assays_raw"],
            "peak_2theta",
            "2θ (degrees) of the largest count",
            lambda: geddes_peak(C["d_assays_raw"]),
            lambda: uxd_peak(C["d_assays_raw"]),
            rel=1e-5,
        ),
        Fact(
            C["d_assays_efs"],
            "peak_field_g",
            "field (G) where the real part of the echo-detected field sweep is largest",
            lambda: efs_peak_field(C["d_assays_efs"]),
            lambda: efs_peak_field(C["d_assays_efs"], manual=True),
            rel=1e-6,
        ),
        Fact(
            C["d_assays_mpt"],
            "max_ewe_v",
            "largest Ewe/V in the EC-Lab export",
            lambda: mpt_max(C["d_assays_mpt"], "Ewe/V", pandas=True),
            lambda: mpt_max(C["d_assays_mpt"], "Ewe/V"),
        ),
        Fact(
            C["d_assays_fcs_ww"],
            "median_fl1",
            "median of FL 1 Log over all events (raw values as stored)",
            lambda: H.fcs_raw_median(C["d_assays_fcs_ww"], "FL 1 Log"),
            lambda: H.fcs_raw_median(C["d_assays_fcs_ww"], "FL 1 Log", True),
        ),
        Fact(
            C["d_assays_bmg"],
            "top_well_max",
            "well whose absorbance reaches the highest value over all reads",
            lambda: bmg_top_final_well(C["d_assays_bmg"]),
            lambda: bmg_top_final_well(C["d_assays_bmg"], pandas=True),
        ),
        Fact(
            C["d_assays_gen5"],
            "top_final_well",
            "well with the highest deGFP fluorescence at the last read (first gain), from the depositor's Gen5 export",
            lambda: gen5_top_final_well(C["d_assays_gen5"]),
            lambda: gen5_top_final_well(C["d_assays_gen5"], pandas=True),
        ),
        Fact(
            C["d_assays_gen5"],
            "kinetic_reads",
            "number of kinetic reads in the depositor's Gen5 export",
            lambda: gen5_reads(C["d_assays_gen5"]),
        ),
        Fact(
            C["d_assays_ngb"],
            "duration_min",
            "duration of the sample run (min)",
            lambda: ngb_duration_min(C["d_assays_ngb"]),
            lambda: ngb_duration_min(C["d_assays_ngb"], export=True),
            rel=2e-3,
        ),
    ]


# ---------------------------------------------------------------- questions


def specs(spec, fact, g, H) -> list[Any]:
    raw = H.HINT_RAW
    unit_hint = "a number with its unit"

    def n_of(o: dict, k: int = 0) -> int:
        return o["traces"][k]["n"]

    def ofact(o: dict, path: str):
        return next(f["value"] for f in o["facts"] if f["path"] == path)

    def table(o: dict, k: int = 0) -> dict:
        return o["tables"][k]

    return [
        # ------------------------------------------------ diffraction
        spec(
            "ho-d-bench-xrdml-step",
            "d_assays_xrdml",
            "method",
            "What is the 2θ step size between consecutive points of this powder diffraction scan?",
            fact("step_2theta", lambda v: g.number(v, "°", rel=0.01)),
            "facts-heldout: step_2theta (geddes; the XRDML read with ElementTree agrees)",
            answer_hint="the step in degrees",
        ),
        spec(
            "ho-d-bench-brml-points",
            "d_assays_brml",
            "counts",
            "How many data points does this grazing-incidence diffraction scan contain?",
            lambda o, m: g.integer(n_of(o)),
            "oracle: /traces/0/n (geddes)",
        ),
        spec(
            "ho-d-ana-bench-raw-peak",
            "d_assays_raw",
            "analysis",
            "This is a powder diffraction scan of halloysite nanotubes. At what 2θ angle is the count highest?",
            fact("peak_2theta", lambda v: g.number(v, "°", abs_=0.05)),
            "facts-heldout: peak_2theta (geddes; the depositor's UXD export agrees)",
            answer_hint="2θ in degrees",
        ),
        spec(
            "ho-d-bench-ras-points",
            "d_assays_ras",
            "counts",
            "This diffraction pattern was recorded at 50 K. How many data points does the scan have?",
            lambda o, m: g.integer(n_of(o)),
            "oracle: /traces/0/n (the file's data block read with the standard library)",
        ),
        # ------------------------------------------------ EPR
        spec(
            "ho-d-ana-bench-bes3t-efs-peak",
            "d_assays_efs",
            "analysis",
            "This is a pulse EPR echo-detected field sweep. At what field is the real part of the echo largest?",
            fact("peak_field_g", lambda v: g.number(v, "G", abs_=2.0)),
            "facts-heldout: peak_field_g (DeerLab deerload; NumPy on the DSC/DTA agrees)",
            answer_hint="the field in gauss",
        ),
        # ------------------------------------------------ electrochemistry
        spec(
            "ho-d-bench-mpr-points",
            "d_assays_mpr",
            "counts",
            "How many data points were recorded in this linear sweep voltammetry of a fuel cell?",
            lambda o, m: g.integer(n_of(o)),
            "oracle: /traces/0/n (galvani, black box)",
        ),
        spec(
            "ho-d-ana-bench-mpt-max-ewe",
            "d_assays_mpt",
            "analysis",
            "What is the highest working-electrode potential (Ewe) reached in this battery test?",
            fact("max_ewe_v", lambda v: g.number(v, "V", abs_=0.001)),
            "facts-heldout: max_ewe_v (pandas on the EC-Lab export; the csv module agrees)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-bench-dta-points",
            "d_assays_dta",
            "counts",
            "How many data points does the chronopotentiometry curve in this Gamry file contain?",
            lambda o, m: g.integer(n_of(o)),
            "oracle: /traces/0/n (gamry-parser, black box)",
        ),
        spec(
            "ho-d-bench-nda-records",
            "d_assays_nda",
            "counts",
            "How many data records does this Neware battery-cycler file contain?",
            lambda o, m: g.integer(n_of(o)),
            "oracle: /traces/0/n (neware_reader, black box)",
        ),
        spec(
            "ho-d-bench-ndax-records",
            "d_assays_ndax",
            "counts",
            "How many data records does this Neware battery-test file contain?",
            lambda o, m: g.integer(n_of(o)),
            "oracle: /traces/0/n (NewareNDA, black box)",
        ),
        # ------------------------------------------------ thermal analysis
        spec(
            "ho-d-bench-ngb-mass",
            "d_assays_ngb",
            "sample",
            "What sample mass was loaded for this thermogravimetric run?",
            lambda o, m: g.number(ofact(o, "method.parameters.sample_mass"), "mg", rel=0.001),
            "oracle: /facts method.parameters.sample_mass (pyNGB metadata; the Proteus export header agrees)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-ana-bench-ngb-duration",
            "d_assays_ngb",
            "analysis",
            "How long did this thermogravimetric run last, in minutes?",
            fact("duration_min", lambda v: g.number(v, "min", rel=0.005)),
            "facts-heldout: duration_min (pyNGB; the depositor's Proteus export agrees)",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-bench-ta-mass",
            "d_assays_ta",
            "sample",
            "What was the sample size (mass) of this DSC run?",
            lambda o, m: g.number(ofact(o, "method.parameters.sample_mass"), "mg", rel=0.001),
            "oracle: /facts method.parameters.sample_mass (the depositor's Universal Analysis export header)",
            answer_hint=unit_hint,
        ),
        # ------------------------------------------------ calorimetry, SPR
        spec(
            "ho-d-bench-itc-syringe",
            "d_assays_itc",
            "method",
            "What was the titrant concentration in the syringe for this ITC titration?",
            lambda o, m: g.number(o["depositor_facts"]["syringe_concentration_mM"] * 1000.0, "µM", rel=0.01),
            "oracle: /depositor_facts/syringe_concentration_mM (the depositor's file name), in µM",
            answer_hint=unit_hint,
        ),
        spec(
            "ho-d-bench-blr-instrument",
            "d_assays_blr",
            "instrument",
            "Which Biacore instrument model recorded this SPR run?",
            lambda o, m: g.string("Biacore T100", accept=["T100", "BiacoreT100", "Biacore T100 Control Software"]),
            "oracle: /environment/ProcessingUnit and Application (olefile on the run file's text)",
            answer_hint="the instrument model",
        ),
        spec(
            "ho-d-bench-bme-cycles",
            "d_assays_bme",
            "counts",
            "How many cycles does this Biacore evaluation file hold?",
            lambda o, m: g.integer(o["cycles"]),
            "oracle: /cycles (olefile; allotropy, black box)",
        ),
        # ------------------------------------------------ qPCR
        spec(
            "ho-d-qpcr-rdml-targets",
            "d_assays_rdml",
            "counts",
            "How many different assay targets does this environmental-DNA qPCR plate use?",
            lambda o, m: g.integer(len(o["targets"])),
            "oracle: /targets (rdmlpython)",
        ),
        spec(
            "ho-d-ana-qpcr-rdml-lowest",
            "d_assays_rdml",
            "analysis",
            "For the brook trout assay (target BRK2), which sample has the lowest quantification cycle (Cq)?",
            lambda o, m: g.string(lowest_cq(o, "sample", "BRK2")),
            "oracle: /records (rdmlpython), lowest Cq among the BRK2 reactions",
            answer_hint="the sample name",
        ),
        spec(
            "ho-d-qpcr-eds-plate",
            "d_assays_eds",
            "counts",
            "How many wells does the plate format of this run have?",
            lambda o, m: g.integer(o["rows"] * o["columns"]),
            "oracle: /rows × /columns (the .eds plate setup)",
        ),
        spec(
            "ho-d-ana-qpcr-eds-lowest",
            "d_assays_eds",
            "analysis",
            "Which well has the lowest threshold cycle (Cq) in this run?",
            lambda o, m: g.string(lowest_cq(o, "well")),
            "oracle: /records (the vendor's Cq values in the .eds)",
            answer_hint="the well, e.g. B7",
        ),
        # ------------------------------------------------ flow cytometry
        spec(
            "ho-d-flow-wastewater-events",
            "d_assays_fcs_ww",
            "counts",
            "How many events were recorded in this file of a wastewater bacterial community?",
            lambda o, m: g.integer(table(o)["event_count"]),
            "oracle: /tables/0/event_count (flowio; fcsparser agrees)",
        ),
        spec(
            "ho-d-ana-flow-wastewater-fl1",
            "d_assays_fcs_ww",
            "analysis",
            "What is the median of the FL 1 Log parameter over all events (raw values as stored)?",
            fact("median_fl1", lambda v: g.number(v, None, rel=0.001)),
            "facts-heldout: median_fl1 (flowio; fcsparser agrees)",
            answer_hint=raw,
        ),
        spec(
            "ho-d-flow-ev-events",
            "d_assays_fcs_ev",
            "counts",
            "This is a buffer-only control from an extracellular-vesicle measurement. How many events does it contain?",
            lambda o, m: g.integer(table(o)["event_count"]),
            "oracle: /tables/0/event_count (flowio; fcsparser agrees)",
        ),
        # ------------------------------------------------ plate readers
        spec(
            "ho-d-plate-bmg-reads",
            "d_assays_bmg",
            "counts",
            "This is a growth curve read in absorbance. How many readings were taken per well?",
            lambda o, m: g.integer(o["plate"]["groups"][0]["values"] // o["plate"]["groups"][0]["wells"]),
            "oracle: /plate/groups/0 values ÷ wells (oracle/plate.py bmg_csv_summary)",
        ),
        spec(
            "ho-d-ana-plate-bmg-top",
            "d_assays_bmg",
            "analysis",
            "Which well reaches the highest absorbance at any point of the growth curve?",
            fact("top_well_max", g.string),
            "facts-heldout: top_well_max (the export text; pandas agrees)",
            answer_hint="the well, e.g. B07",
        ),
        spec(
            "ho-d-plate-gen5-reads",
            "d_assays_gen5",
            "counts",
            "This is a kinetic fluorescence run. How many reads did the kinetic loop take?",
            fact("kinetic_reads", g.integer),
            "facts-heldout: kinetic_reads (the depositor's Gen5 comma-separated export of the same run)",
        ),
        spec(
            "ho-d-ana-plate-gen5-top",
            "d_assays_gen5",
            "analysis",
            "At the last read of the first deGFP measurement (the lower gain), which well is brightest?",
            fact("top_final_well", g.string),
            "facts-heldout: top_final_well (the depositor's Gen5 comma-separated export; pandas agrees)",
            answer_hint="the well, e.g. B14",
        ),
    ]
