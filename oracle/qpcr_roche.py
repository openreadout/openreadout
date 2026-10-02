"""Ground truth for Roche LightCycler files, read without our Rust code.

- LightCycler 96 (`.lc96p`): the depositors' table of the LightCycler 96 software's results
  (openpyxl), one record per exported well: sample name, gene name, Cq, Cq Mean, Cq Error, as
  printed. `tests/qpcr_roche.rs` checks that our Cqs of each sample x gene are exactly these,
  that no other reaction has a Cq, and our replicate mean / SD.
- LightCycler 480 (`.ixo`): an independent decode with the Python standard library of what the
  file states: per plate position the sample name and the analysis' Call / CrossingPoint /
  IsIncluded, and per program and channel the number of readings and their sum (the
  acquisition store: base64, zlib, f32 little-endian). No vendor export exists for these runs.

Written to corpus/oracle/qpcr-roche/<id>.json.

Usage:
  qpcr_roche.py lc96 ID XLSX SHEET REPLICATES   (REPLICATES: comma list, or `all`)
  qpcr_roche.py ixo ID FILE.ixo
"""
import base64
import json
import struct
import sys
import xml.etree.ElementTree as ET
import zlib
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "corpus" / "oracle" / "qpcr-roche"


def lc96(xlsx: Path, sheet: str, reps: str) -> dict:
    import openpyxl

    ws = openpyxl.load_workbook(xlsx, data_only=True)[sheet]
    rows = [list(r) for r in ws.iter_rows(values_only=True)]
    head = [str(c).strip() if c is not None else "" for c in rows[0]]
    col = {name: head.index(name) for name in ("Replicate", "Sample Name", "Gene Name", "Cq", "Cq Mean", "Cq Error")}
    keep = None if reps == "all" else {int(x) for x in reps.split(",")}
    recs = []
    for r in rows[1:]:
        if r[col["Replicate"]] is None:
            continue
        rep = int(r[col["Replicate"]])
        if keep is not None and rep not in keep:
            continue

        def txt(k):
            v = r[col[k]]
            return str(v).strip() if v is not None else ""

        recs.append({
            "replicate": rep,
            "sample": txt("Sample Name"),
            "gene": txt("Gene Name"),
            "cq": txt("Cq"),
            "cq_mean": txt("Cq Mean"),
            "cq_error": txt("Cq Error"),
        })
    return {"reader": f"openpyxl {openpyxl.__version__}", "export": xlsx.name, "sheet": sheet, "records": recs}


def props(o):
    return {p.get("name"): (p.text or "").strip() for p in o if p.tag == "prop"}


def ixo(path: Path) -> dict:
    b = path.read_bytes()
    end = b.rfind(b"</objectstream>") + len(b"</objectstream>")
    root = ET.fromstring(b[:end])
    exp = root.find("obj[@name='root']")
    run = exp.find("obj[@name='run']")
    rp = props(run)
    progs = run.find("obj[@name='Protocol']/obj[@name='Programs']")
    programs = [props(p) for p in progs.find("list[@name='emlist']")]
    fmt = progs.find("obj[@name='DetectionFormats']")
    chans = [props(c) for c in fmt.find("list[@name='emlist']/obj/list[@name='emlist']")]
    names = {}
    for s in exp.iter("obj"):
        if s.get("class") == "SamplePropInfo":
            pos = int(props(s)["ContainerPosition"])
            for o in s.iter("obj"):
                if o.get("class") == "GenSampleEditName":
                    names[pos] = props(o).get("name")
    calls = {}
    for o in exp.iter("obj"):
        if o.get("class") == "QuantSampleB":
            p = props(o)
            calls[int(p["Pos"])] = {"call": int(p["Call"]), "cp": float(p["CrossingPoint"]), "included": p["IsIncluded"] == "1"}
    store = ET.fromstring(zlib.decompress(base64.b64decode(rp["AcquisitionStore"])))
    sums = {}
    for c in store.find("list[@name='Cycles']"):
        cp = props(c)
        for a in c.find("list[@name='Acquisitions']"):
            ap = props(a)
            raw = base64.b64decode(ap["FloPoints"])
            vals = struct.unpack("<%df" % (len(raw) // 4), raw)
            key = f"{cp['Program']}/{ap['Channel']}"
            n, s, t = sums.get(key, (0, 0.0, 0.0))
            temp = struct.unpack(">d", bytes.fromhex(ap["Temp"][1:]))[0]
            sums[key] = (n + 1, s + sum(float(v) for v in vals), t + temp)
    return {
        "reader": "Python standard library (xml.etree, zlib, base64, struct)",
        "experiment": exp.find("prop[@name='name']").text,
        "instrument": rp.get("InstrumentName"),
        "started_at": rp.get("StartTime"),
        "programs": [{"name": p.get("name"), "cycles": int(p.get("Cycles", "0")), "mode": p.get("AnalysisMode")} for p in programs],
        "channels": [c.get("name") for c in chans],
        "positions": [
            {"pos": pos, "sample": names.get(pos), **calls.get(pos, {})} for pos in sorted(set(names) | set(calls))
        ],
        # "program/channel": [readings, sum of all positions' values, sum of read temperatures]
        "readings": {k: list(v) for k, v in sorted(sums.items())},
    }


def main():
    kind, ident = sys.argv[1], sys.argv[2]
    if kind == "lc96":
        out = lc96(Path(sys.argv[3]), sys.argv[4], sys.argv[5])
    elif kind == "ixo":
        out = ixo(Path(sys.argv[3]))
    else:
        sys.exit(__doc__)
    out = {"id": ident, "kind": kind, **out}
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / f"{ident}.json").write_text(json.dumps(out, indent=1, ensure_ascii=False) + "\n")
    print(ident, len(out.get("records", out.get("positions", []))))


if __name__ == "__main__":
    main()
