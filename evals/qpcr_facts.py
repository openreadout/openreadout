"""qPCR facts and questions (real-time PCR: RDML, Applied Biosystems .eds, Rotor-Gene .rex).

Imported by `analysis.py` (facts go to `evals/facts/analysis.json`, questions to the analysis tier
and to `questions/qpcr.jsonl`). Every value is read with readers independent of OpenReadout:

- `.eds`: the vendor's own result and setup files inside the zip (`apldbio/sds/analysis_result.txt`,
  `meltcuve_result.txt`, `tcprotocol.xml`, `setup/plate_setup.json`, `setup/run_method.json`,
  `extensions/am.sc/standard_curve_result.json`), parsed with the Python standard library (zipfile,
  ElementTree, json). The Cq, Tm, RQ and standard-curve values are the ones the vendor software
  computed and stored;
- RDML: rdmlpython (RDML consortium, MIT), the standard's reference implementation;
- `.rex`: ElementTree over the XML.

This module imports only the standard library at the top; rdmlpython is imported where it is used.
"""

from __future__ import annotations

import json
import re
import statistics
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path
from typing import Any, Callable


def _zip_text(p: Path, name: str) -> str | None:
    z = zipfile.ZipFile(p)
    names = {n.replace("\\", "/").lower(): n for n in z.namelist()}
    n = names.get(name.lower())
    return z.read(n).decode("utf-8-sig", "replace") if n else None


def _result_lines(text: str) -> list[dict[str, str]]:
    """Result lines of `analysis_result.txt` / `meltcuve_result.txt` as {header: cell}."""
    header: list[str] | None = None
    out = []
    for line in text.splitlines():
        cells = line.split("\t")
        if header is None:
            if cells and cells[0].strip().lower() == "well":
                header = [c.strip().lower() for c in cells]
            continue
        if cells and cells[0].strip().isdigit():
            out.append(dict(zip(header, (c.strip() for c in cells), strict=False)))
    return out


def _well_name(index: int, columns: int) -> str:
    r, c = divmod(index, columns)
    return f"{chr(65 + r)}{c + 1}"


def eds_cq(p: Path, well: str, target: str, columns: int = 12) -> tuple[float, str]:
    for r in _result_lines(_zip_text(p, "apldbio/sds/analysis_result.txt")):
        if _well_name(int(r["well"]), columns) == well and r["detector"] == target:
            return float(r["ct"]), "zipfile + analysis_result.txt (vendor Ct)"
    raise KeyError((well, target))


def eds_tm(p: Path, well: str, target: str, columns: int = 12) -> tuple[float, str]:
    for r in _result_lines(_zip_text(p, "apldbio/sds/meltcuve_result.txt")):
        if _well_name(int(r["well"]), columns) == well and r["detector"] == target:
            tms = [float(t) for t in re.split(r"[,;]", r["tm"]) if t.strip()]
            assert len(tms) == 1, tms
            return tms[0], "zipfile + meltcuve_result.txt (vendor Tm)"
    raise KeyError((well, target))


def eds_rq(p: Path, sample: str, target: str) -> tuple[float, str]:
    """The RQ the vendor stored on the `DDCT Values` lines of a sample x target's wells."""
    text = _zip_text(p, "apldbio/sds/analysis_result.txt")
    values = set()
    current: dict[str, str] | None = None
    header = None
    for line in text.splitlines():
        cells = line.split("\t")
        if header is None:
            if cells[0].strip().lower() == "well":
                header = [c.strip().lower() for c in cells]
            continue
        if cells[0].strip().isdigit():
            current = dict(zip(header, (c.strip() for c in cells), strict=False))
        elif cells[0].strip() == "DDCT Values" and current is not None:
            if current["sample name"] == sample and current["detector"] == target:
                values.add(float(cells[12]))  # label, well, -, sample, target, task, Ct, -, ΔCt, SD, SE, -, RQ
    assert len(values) == 1, values
    return values.pop(), "zipfile + analysis_result.txt `DDCT Values` (vendor RQ)"


def eds_json_ntc_wells(p: Path) -> tuple[int, str]:
    ps = json.loads(_zip_text(p, "setup/plate_setup.json"))
    wells = {w["index"] for w in ps["wells"] for t in w.get("targetAssignments", []) if t.get("task") == "NTC"}
    return len(wells), "zipfile + json: setup/plate_setup.json (wells with task NTC)"


def eds_json_read_temperature(p: Path) -> tuple[float, str]:
    m = json.loads(_zip_text(p, "setup/run_method.json"))
    temps = [
        s["ramp"]["temperature"]
        for st in m["stages"]
        if st.get("repeat", 1) > 1
        for s in st["steps"]
        if (s.get("hold") or {}).get("collectionProfile")
    ]
    assert len(temps) == 1, temps
    return temps[0], "zipfile + json: setup/run_method.json (cycling step with a collection profile)"


def eds_json_std_efficiency(p: Path, target: str) -> tuple[float, str]:
    sc = json.loads(_zip_text(p, "extensions/am.sc/standard_curve_result.json"))
    (c,) = [c for c in sc["standardCurves"] if c["targetName"] == target]
    return c["efficiency"], "zipfile + json: extensions/am.sc/standard_curve_result.json (vendor fit)"


def rex_cycles(p: Path) -> tuple[int, str]:
    root = ET.fromstring(p.read_bytes())
    return int(root.findtext("Profile/Cycle/RepeatCount")), "ElementTree: Profile/Cycle/RepeatCount"


def _rdml(p: Path):
    import rdmlpython as rp

    try:
        r = rp.Rdml(str(p))
    except Exception:
        r = rp.Rdml()
        r.load_any_zip(str(p))
    return r


def _rdml_version() -> str:
    from importlib.metadata import version

    return f"rdmlpython {version('rdmlpython')}"


def rdml_highest_mean_cq_target(p: Path) -> tuple[str, str]:
    """Mean Cq per target over every reaction with a Cq (not excluded); the target with the highest."""
    r = _rdml(p)
    cqs: dict[str, list[float]] = {}
    for e in r.experiments():
        for run in e.runs():
            for rx in run.getreactjson()["reacts"]:
                for d in rx.get("datas", []):
                    cq = d.get("cq")
                    if cq in (None, "") or float(cq) < 0 or d.get("excl"):
                        continue
                    cqs.setdefault(d["tar"], []).append(float(cq))
    means = {t: statistics.fmean(v) for t, v in cqs.items()}
    ranked = sorted(means, key=means.get, reverse=True)
    assert means[ranked[0]] - means[ranked[1]] > 0.5, means  # a clear winner
    return ranked[0], _rdml_version()


def rdml_cq(p: Path, run_id: str, well: str, target: str) -> tuple[float, str]:
    r = _rdml(p)
    row, col = ord(well[0]) - 65, int(well[1:]) - 1
    for e in r.experiments():
        for run in e.runs():
            if run["id"] != run_id:
                continue
            cols = int(run["pcrFormat_columns"])
            for rx in run.getreactjson()["reacts"]:
                if int(rx["id"]) == row * cols + col + 1:
                    for d in rx["datas"]:
                        if d["tar"] == target:
                            return float(d["cq"]), _rdml_version()
    raise KeyError((run_id, well, target))


# ---------------------------------------------------------------- facts and questions

EDS_SYBR = "eds-qs35-sybr-water-test"
EDS_DDCT = "eds-7500-abhd17c-ddct"
EDS_STD = "eds-qs7pro-tb18s-stdcurve"
RDML_UPR = "rdml-kphaffii-upr-linregpcr"
RDML_STEPONE = "rdml-stepone-std"
RDML_CFX = "rdml-biorad-cfx-melt"
REX = "rex-rotorgene-72well"


def facts(Fact: type, path: Callable[[str], Path]) -> list:
    """The qPCR facts, as `analysis.Fact`s."""
    return [
        Fact(
            EDS_SYBR,
            "cq_bactin_g3",
            "vendor Ct of target Bactin in well G3 (analysis_result.txt)",
            lambda: eds_cq(path(EDS_SYBR), "G3", "Bactin"),
        ),
        Fact(
            EDS_DDCT,
            "cq_18s_a2",
            "vendor Ct of target 18s in well A2 (analysis_result.txt)",
            lambda: eds_cq(path(EDS_DDCT), "A2", "18s"),
        ),
        Fact(
            EDS_DDCT,
            "tm_b3",
            "vendor Tm of the product in well B3 (meltcuve_result.txt; one peak)",
            lambda: eds_tm(path(EDS_DDCT), "B3", "18s"),
        ),
        Fact(
            EDS_DDCT,
            "rq_abhd17c_oe",
            "vendor RQ (2^-ΔΔCt) of ABHD17C in sample `ABHD17C OE`, reference 18s, "
            "calibrator `Lenvatinib/Vector` (DDCT Values lines)",
            lambda: eds_rq(path(EDS_DDCT), "ABHD17C OE", "ABHD17C"),
        ),
        Fact(
            EDS_STD,
            "ntc_wells",
            "wells with task NTC in setup/plate_setup.json",
            lambda: eds_json_ntc_wells(path(EDS_STD)),
        ),
        Fact(
            EDS_STD,
            "read_temperature_c",
            "temperature of the cycling step that collects fluorescence (run_method.json)",
            lambda: eds_json_read_temperature(path(EDS_STD)),
        ),
        Fact(
            EDS_STD,
            "std_efficiency_pct",
            "the vendor's standard-curve efficiency (%) for Tb18s",
            lambda: eds_json_std_efficiency(path(EDS_STD), "Tb18s"),
        ),
        Fact(
            RDML_UPR,
            "highest_mean_cq_target",
            "mean Cq per target over reactions with a Cq (rdmlpython); the highest",
            lambda: rdml_highest_mean_cq_target(path(RDML_UPR)),
        ),
        Fact(
            RDML_CFX,
            "cq_katg_d7",
            "Cq of well D7 (sample katG 315, target EvaGreen) in run `Amp Step 3_FAM` (rdmlpython)",
            lambda: rdml_cq(path(RDML_CFX), "Amp Step 3_FAM", "D7", "EvaGreen"),
        ),
        Fact(REX, "cycles", "Profile/Cycle/RepeatCount", lambda: rex_cycles(path(REX))),
    ]


def specs(g: Any) -> list:
    """The qPCR questions (`generate.Spec`s)."""
    src = "facts-analysis: {} ({})"
    return [
        g.Spec(
            "qpcr-eds-cq-well",
            EDS_SYBR,
            "values",
            "What is the Ct (Cq) of the Bactin assay in well G3, as the instrument software called it?",
            lambda f, m: g.number(f["cq_bactin_g3"], None, abs_=0.01),
            src.format("cq_bactin_g3", "vendor analysis_result.txt"),
            answer_hint="a number (cycles)",
        ),
        g.Spec(
            "qpcr-eds-cq-18s",
            EDS_DDCT,
            "values",
            "What is the Ct of the 18s reference gene in well A2?",
            lambda f, m: g.number(f["cq_18s_a2"], None, abs_=0.01),
            src.format("cq_18s_a2", "vendor analysis_result.txt"),
            answer_hint="a number (cycles)",
        ),
        g.Spec(
            "qpcr-eds-tm-well",
            EDS_DDCT,
            "values",
            "What is the melting temperature (Tm) of the PCR product in well B3?",
            lambda f, m: g.number(f["tm_b3"], "°C", abs_=0.05),
            src.format("tm_b3", "vendor meltcuve_result.txt"),
            answer_hint="a temperature in °C",
        ),
        g.Spec(
            "qpcr-eds-ntc-count",
            EDS_STD,
            "counts",
            "How many wells on this plate are no-template controls (NTC)?",
            lambda f, m: g.integer(f["ntc_wells"]),
            src.format("ntc_wells", "setup/plate_setup.json"),
            answer_hint="the number of wells",
        ),
        g.Spec(
            "qpcr-eds-read-temperature",
            EDS_STD,
            "method",
            "At what temperature was fluorescence read during the PCR cycles (the annealing/extension step)?",
            lambda f, m: g.number(f["read_temperature_c"], "°C", abs_=0.05),
            src.format("read_temperature_c", "setup/run_method.json"),
            answer_hint="a temperature in °C",
        ),
        g.Spec(
            "qpcr-rdml-cfx-cq",
            RDML_CFX,
            "values",
            "In the FAM run of this Bio-Rad export, what is the Cq of well D7?",
            lambda f, m: g.number(f["cq_katg_d7"], None, abs_=0.01),
            src.format("cq_katg_d7", "rdmlpython"),
            answer_hint="a number (cycles)",
        ),
        g.Spec(
            "qpcr-rex-cycles",
            REX,
            "counts",
            "How many PCR cycles did this Rotor-Gene run perform?",
            lambda f, m: g.integer(f["cycles"]),
            src.format("cycles", "ElementTree"),
            answer_hint="the number of cycles",
        ),
    ]


def analysis_specs(g: Any) -> list:
    """qPCR questions of the analysis tier (ids `ana-*`)."""
    src = "facts-analysis: {} ({})"
    return [
        g.Spec(
            "ana-qpcr-rdml-highest-mean-cq",
            RDML_UPR,
            "analysis",
            "Across all runs in this file, which target (gene) has the highest mean Cq over its reactions "
            "(ignore reactions without a Cq)?",
            lambda f, m: g.string(f["highest_mean_cq_target"]),
            src.format("highest_mean_cq_target", "rdmlpython"),
            answer_hint="the target name",
        ),
        g.Spec(
            "ana-qpcr-eds-ddct-rq",
            EDS_DDCT,
            "analysis",
            "Using 18s as the reference gene and the sample `Lenvatinib/Vector` as the calibrator, what is the "
            "relative expression (fold change, 2^-ΔΔCt) of ABHD17C in the sample `ABHD17C OE`?",
            lambda f, m: g.number(f["rq_abhd17c_oe"], None, rel=0.01),
            src.format("rq_abhd17c_oe", "vendor RQ, DDCT Values"),
            answer_hint="a number (fold change)",
        ),
        g.Spec(
            "ana-qpcr-eds-std-efficiency",
            EDS_STD,
            "analysis",
            "From the standard wells on this plate (known quantities), what is the PCR amplification efficiency "
            "of the Tb18s assay in percent (from the slope of Cq against log10 quantity)?",
            lambda f, m: g.number(f["std_efficiency_pct"], None, abs_=1.0),
            src.format("std_efficiency_pct", "vendor standard curve"),
            answer_hint="a percentage",
        ),
    ]
