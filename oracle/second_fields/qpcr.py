"""qPCR: a second reading of run metadata and the plate layout.

- Applied Biosystems .eds: qslib (the Rust/Python QuantStudio library; the primary oracle is a
  standard-library reading of the same zip, with vendor exports where the depositor gave them):
  run start and end, and each well's sample name, where qslib reads the file (its plate-setup
  parser needs QuantStudio-era XML; older 7500 / StepOne files are refused and get no checks).
- RDML: the XML read with ElementTree (the primary oracle is rdmlpython): each run's start, the
  number of cycles of each reaction's amplification data, and each reaction's sample.
"""
from __future__ import annotations

import re
import zipfile
from importlib.metadata import version
from pathlib import Path

from . import Rec

FAMILY = "qpcr"
FORMATS = {"applied-biosystems-eds", "rdml"}


def eds(rec: Rec, path: Path) -> None:
    import warnings
    warnings.filterwarnings("ignore")
    import qslib
    e = qslib.Experiment.from_file(str(path))
    r = rec.reader(f"qslib {version('qslib')} (EUPL-1.2, run as a black box)")
    if e.runstarttime is not None and e.runstarttime.year > 1990:
        rec.check("run start", "/experiment/acquisition/started_at", e.runstarttime.isoformat(), cmp="time",
                  abs=1.0, reader=r)
    if e.runendtime is not None and e.runendtime.year > 1990:
        rec.check("run end", "/experiment/acquisition/ended_at", e.runendtime.isoformat(), cmp="time", abs=1.0,
                  reader=r)
    ps = e.plate_setup
    try:
        table = ps.to_polars_by_well() if hasattr(ps, "to_polars_by_well") else None
    except Exception:
        table = None
    if table is None or "well" not in table.columns:
        return
    samples = {}
    for row in table.iter_rows(named=True):
        s = row.get("sample") or row.get("name")
        w = row.get("well")
        if s and w:
            samples[str(w)] = str(s)
    rec.check("well samples (amplification channels)", "/traces/0/channels", samples, by="/extra/well",
              each="/extra/sample", cmp="text", reader=r)


def rdml(rec: Rec, path: Path) -> None:
    import xml.etree.ElementTree as ET
    if zipfile.is_zipfile(path):
        with zipfile.ZipFile(path) as z:
            name = next((n for n in z.namelist() if n.lower().endswith(".xml")), None)
            if name is None:
                return
            text = z.read(name)
    else:
        text = path.read_bytes()
    root = ET.fromstring(text)
    ns = {"r": root.tag.split("}")[0].strip("{")} if root.tag.startswith("{") else {"r": ""}
    q = (lambda t: f"r:{t}") if ns["r"] else (lambda t: t)
    r = rec.reader("the RDML XML read with Python's ElementTree")
    runs = root.findall(f".//{q('run')}", ns)
    if not runs:
        return
    run = runs[0]
    date = run.findtext(q("runDate"), namespaces=ns)
    if date:
        rec.check("first run: runDate", "/experiment/acquisition/started_at", date.strip(), cmp="time", abs=1.0,
                  reader=r)
    rec.check("first run: instrument", "/experiment/instrument/model", run.findtext(q("instrument"), namespaces=ns),
              cmp="text", reader=r)
    cols = run.find(f"{q('pcrFormat')}/{q('columns')}", ns)
    ncol = int(cols.text) if cols is not None and (cols.text or "").isdigit() else 12
    label = run.findtext(f"{q('pcrFormat')}/{q('columnLabel')}", namespaces=ns) or "123"
    # a reaction names its sample by id; the sample's description (when it has one) is its name
    names = {}
    for smp in root.findall(q("sample"), ns):
        d = (smp.findtext(q("description"), namespaces=ns) or "").strip()
        names[smp.get("id")] = d or smp.get("id")
    samples, cycles = {}, {}
    for react in run.findall(q("react"), ns):
        rid = react.get("id")
        s = react.find(q("sample"), ns)
        if rid is None or not rid.isdigit() or s is None:
            continue
        k = int(rid) - 1
        well = f"{chr(ord('A') + k // ncol)}{k % ncol + 1}" if label in ("123",) else rid
        samples[well] = names.get(s.get("id"), s.get("id"))
        d = react.find(q("data"), ns)
        if d is not None:
            cycles[well] = len(d.findall(q("adp"), ns))
    if not any(cycles.values()):
        return  # no amplification data (Cq or melt only): nothing to set against our traces
    rec.check("first run: reaction samples", "/traces/0/channels", samples, by="/extra/well", each="/extra/sample",
              cmp="text", reader=r)
    if cycles and len(set(cycles.values())) == 1:
        rec.check("first run: amplification points per reaction", "/traces/0/sample_count", next(iter(cycles.values())),
                  reader=r)


def run(rec: Rec, entry: dict, path: Path, ctx) -> None:
    fmt = entry["format"]
    if fmt == "applied-biosystems-eds":
        try:
            eds(rec, path)
        except ValueError as e:  # qslib's plate-setup parser refuses pre-QuantStudio XML
            rec.note(f"qslib: {e}")
    elif fmt == "rdml" and path.suffix.lower() == ".rdml":
        rdml(rec, path)
