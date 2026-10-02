"""Ground truth for the qPCR readers (RDML, Applied Biosystems .eds, Rotor-Gene .rex).

    uv run --group qpcr python qpcr.py [--export XLS] [--id ID] FILE [FILE ...]  -> ../corpus/oracle/qpcr/<id>.json
    uv run --group qpcr python qpcr.py --manifest [ID-SUBSTRING ...]   every development qPCR input of
                                                                        corpus/manifest.toml, each with its
                                                                        paired `<id>-xls` vendor export
    uv run --group qpcr python qpcr.py --validate-rdml FILE.rdml     -> rdmlpython schema validation

Readers, all independent of OpenReadout:
- RDML: rdmlpython (RDML consortium, MIT), the reference implementation of the standard. Files
  older than RDML 1.3 are migrated in memory with rdmlpython's own migration functions first.
- .eds: the vendor's own results, read with the Python standard library (zipfile, csv-like
  tab splitting, ElementTree, json): Cq/Ct, Tm, task, sample and target names exactly as the
  vendor software wrote them, and the per-cycle Rn / ΔRn it stored. qslib (EUPL-1.2, run as a
  black box) is a second reader of the multicomponent (per-dye) signal.
- .rex: ElementTree over the XML (raw channel readings; Rotor-Gene files store no results).

For every well x target the oracle records the vendor Cq (or that it is undetermined), the Tm
list, and checksums of the curves: number of points and their sum (compared with a relative
tolerance by crates/openreadout-corpus-tests/tests/qpcr_oracle/mod.rs).

"Undetermined" in the SDS/7500 text results: the vendor software writes a well without a Cq as
`Ct` = the cycle count of the cycling stage (40.0 for 40 cycles), not as text; its own Results
export shows those wells as "Undetermined" (checked on five exports, docs/provenance/qpcr.md
2026-09-24). A `Ct` >= the program's cycle count is therefore recorded as undetermined, with the
stored number in `cq_stored`. With `--export`, the vendor's Results export of the same run (.xls
read with xlrd, BSD; .xlsx with openpyxl, MIT) is a second, independent source: its CT per well
("Undetermined" or a number) and Amp Status go to `export_ct` / `export_undetermined` /
`export_amp_status` of each record.
"""
import hashlib
import json
import math
import os
import re
import sys
import warnings
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parent))
import oracle_json  # noqa: E402  (<id>.json, or <id>.json.gz over 1 MiB in corpus/oracle/heldout)

warnings.filterwarnings("ignore")

OUT = (Path(os.environ["ORACLE_OUT"]) if os.environ.get("ORACLE_OUT")  # held-out: gen_heldout.py
       else Path(__file__).resolve().parent.parent / "corpus" / "oracle" / "qpcr")


def sha256(p: Path) -> str:
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def num(s):
    try:
        v = float(str(s).strip())
    except (TypeError, ValueError):
        return None
    return v if math.isfinite(v) else None


def fsum(v):
    v = [x for x in v if x is not None and math.isfinite(x)]
    return math.fsum(v) if v else 0.0


def row_letters(r: int) -> str:
    s = ""
    n = r + 1
    while n > 0:
        n, rem = divmod(n - 1, 26)
        s = chr(65 + rem) + s
    return s


# ------------------------------------------------------------------ RDML (rdmlpython)

def rdml(p: Path) -> dict:
    import rdmlpython as rp

    try:
        r = rp.Rdml(str(p))
    except Exception:  # instruments name the XML member otherwise (BioRad_qPCR_melt.xml)
        r = rp.Rdml()
        r.load_any_zip(str(p))
    version = r.version()
    migrated = []
    for frm, fn in [("1.0", "migrate_version_1_0_to_1_1"), ("1.1", "migrate_version_1_1_to_1_2"), ("1.2", "migrate_version_1_2_to_1_3")]:
        if r.version() == frm:
            getattr(r, fn)()
            migrated.append(fn)
    samples = {}
    for s in r.samples():
        j = s.tojson()
        types = j.get("types") or []
        quantities = j.get("quantitys") or []
        samples[j["id"]] = {
            "types": {t.get("targetId", ""): t["type"] for t in types},
            "quantities": {q.get("targetId", ""): num(q.get("value")) for q in quantities},
        }
    targets = {}
    for t in r.targets():
        j = t.tojson()
        targets[j["id"]] = {"type": j.get("type"), "dye": j.get("dyeId"), "efficiency": num(j.get("amplificationEfficiency"))}
    runs = []
    records = []
    for e in r.experiments():
        for run in e.runs():
            cols = int(num(run["pcrFormat_columns"]) or 1)
            rows = int(num(run["pcrFormat_rows"]) or 1)
            rl, cl = run["pcrFormat_rowLabel"], run["pcrFormat_columnLabel"]
            j = run.getreactjson()
            runs.append({"experiment": e["id"], "name": run["id"], "rows": rows, "columns": cols, "reactions": len(j["reacts"])})
            for rx in j["reacts"]:
                m = re.fullmatch(r"([A-Za-z]{1,2})(\d+)", str(rx["id"]).strip())
                if m:  # RDML 1.0 labels (A1, H12): rdmlpython's migration keeps them
                    letters, digits = m.group(1).upper(), int(m.group(2))
                    row = 0
                    for ch in letters:
                        row = row * 26 + (ord(ch) - 64)
                    row, col = row - 1, digits - 1
                    rid = None
                else:
                    rid = int(rx["id"])
                    row, col = divmod(rid - 1, cols)
                for d in rx.get("datas", []):
                    cq = num(d.get("cq"))
                    adps = d.get("adps") or []
                    mdps = d.get("mdps") or []
                    sample = rx.get("sample")
                    s = samples.get(sample, {})
                    tar = d.get("tar")
                    typ = s.get("types", {}).get(tar) or s.get("types", {}).get("") or "unkn"
                    qty = s.get("quantities", {}).get(tar)
                    if qty is None:
                        qty = s.get("quantities", {}).get("")
                    records.append({
                        "run": run["id"],
                        "position": rid,
                        "row": row + 1,
                        "col": col + 1,
                        "sample": sample,
                        "target": tar,
                        "dye": targets.get(tar, {}).get("dye"),
                        "task_raw": typ,
                        "quantity": qty,
                        "cq": None if cq is None or cq == -1.0 else cq,
                        "cq_undetermined": cq == -1.0,
                        "tm": [v for v in [num(d.get("meltTemp"))] if v is not None],
                        "excluded": d.get("excl") or None,
                        "amp_n": len(adps),
                        "amp_sum": fsum(num(a[1]) for a in adps),
                        "melt_n": len(mdps),
                        "melt_sum": fsum(num(m[1]) for m in mdps),
                        "melt_t_sum": fsum(num(m[0]) for m in mdps),
                    })
    ok, msg = True, ""
    try:
        res = r.validate()
        ok = "RDML file is valid" in res
        msg = res.strip().splitlines()[-1] if res.strip() else ""
    except Exception as ex:  # validation is information, not a failure of the oracle
        ok, msg = False, f"{type(ex).__name__}: {ex}"
    return {
        "reader": "rdmlpython " + getattr(rp, "__version__", "") + " (MIT)",
        "format": "rdml",
        "rdml_version": version,
        "migrated": migrated,
        "schema_valid": ok,
        "schema_message": msg,
        "runs": runs,
        "targets": targets,
        "samples": sorted(samples),
        "records": records,
    }


# ------------------------------------------------------------------ .eds (stdlib + qslib)

def _text_results(text: str):
    header = None
    out = []
    for line in text.splitlines():
        cells = line.split("\t")
        first = cells[0].strip() if cells else ""
        if header is None:
            if first.lower() == "well":
                header = [c.strip().lower() for c in cells]
            continue
        if first.isdigit():
            out.append({"cells": dict(zip(header, cells)), "series": {}})
        elif out and first:
            out[-1]["series"][first.lower()] = cells[1:]
    return out


def _nums(cells):
    return [num(c) for c in cells if c.strip() != ""]


def _task(s):
    return (s or "").strip()


def eds(p: Path) -> dict:
    z = zipfile.ZipFile(p)
    names = {n.replace("\\", "/").lower(): n for n in z.namelist()}

    def read(name):
        n = names.get(name.lower())
        return z.read(n).decode("utf-8-sig", "replace") if n else None

    out = {"reader": "Python stdlib (zipfile, ElementTree, json) over the vendor's own result files", "format": "applied-biosystems-eds"}
    records = {}
    # plate geometry and setup
    rows = cols = None
    setup_json = read("setup/plate_setup.json")
    wells_setup = {}
    if setup_json:
        ps = json.loads(setup_json)
        block = ps.get("blockType", "")
        rows, cols = (16, 24) if "384" in block else (8, 12)
        for w in ps.get("wells", []):
            for ta in w.get("targetAssignments", []):
                wells_setup[(w["index"], ta["targetName"])] = {"sample": w.get("sampleName"), "task_raw": ta.get("task"), "quantity": ta.get("quantity")}
    else:
        x = read("apldbio/sds/plate_setup.xml")
        if x:
            root = ET.fromstring(x)
            rows = int(root.findtext("Rows") or 8)
            cols = int(root.findtext("Columns") or 12)
            samples = {}
            for fm in root.findall("FeatureMap"):
                fid = fm.findtext("Feature/Id")
                for fv in fm.findall("FeatureValue"):
                    idx = int(fv.findtext("Index"))
                    if fid == "sample":
                        samples[idx] = fv.findtext("FeatureItem/Sample/Name")
                    elif fid == "detector-task":
                        for dt in fv.findall("FeatureItem/DetectorTaskList/DetectorTask"):
                            wells_setup[(idx, dt.findtext("Detector/Name"))] = {"task_raw": dt.findtext("Task"), "quantity": num(dt.findtext("Concentration")), "dye": dt.findtext("Detector/Reporter")}
            for (idx, _t), v in wells_setup.items():
                v["sample"] = samples.get(idx)
    out["rows"], out["columns"] = rows, cols

    def rec(idx, target):
        key = f"{idx}|{target}"
        if key not in records:
            s = wells_setup.get((idx, target), {})
            r_, c_ = divmod(idx, cols)
            records[key] = {"position": idx + 1, "row": r_ + 1, "col": c_ + 1, "well": f"{row_letters(r_)}{c_ + 1}", "target": target,
                            "sample": s.get("sample"), "task_raw": s.get("task_raw"), "quantity": s.get("quantity"),
                            "cq": None, "cq_undetermined": False, "tm": []}
        return records[key]

    # cycle count of the cycling stage (the vendor writes "no Cq" as Ct = this count)
    cycles = None
    tc0 = read("apldbio/sds/tcprotocol.xml")
    if tc0:
        for st in ET.fromstring(tc0).findall("TCStage"):
            if st.findtext("StageFlag") == "CYCLING":
                cycles = int(st.findtext("NumOfRepetitions"))
                break

    # SDS / 7500 text results
    ar = read("apldbio/sds/analysis_result.txt")
    if ar:
        for r in _text_results(ar):
            c = r["cells"]
            if "call" in c:
                out["genotyping"] = True
                continue
            idx = int(c["well"])
            target = (c.get("detector") or c.get("target name") or "").strip()
            x = rec(idx, target)
            ct = c.get("ct", c.get("cq", "")).strip()
            v = num(ct)
            if v is not None and v >= 0 and (cycles is None or v < cycles):
                x["cq"] = v
            elif ct:
                x["cq_undetermined"] = True
                if v is not None:
                    x["cq_stored"] = v
            amp = (c.get("amp status") or "").strip()
            if amp:
                x["amp_status_raw"] = amp
            if x["sample"] is None and c.get("sample name", "").strip():
                x["sample"] = c["sample name"].strip()
            rn = _nums(r["series"].get("rn values", []))
            drn = _nums(r["series"].get("delta rn values", []))
            x["amp_n"] = len(rn)
            x["amp_sum"] = fsum(rn)
            if drn and len(drn) == len(rn):
                x["corrected_sum"] = fsum(drn)
            dd = r["series"].get("ddct values")
            if dd:
                x["vendor_delta_cq"] = num(dd[7]) if len(dd) > 7 else None
                x["vendor_rq"] = num(dd[11]) if len(dd) > 11 else None
    mr = read("apldbio/sds/meltcuve_result.txt")
    if mr:
        for r in _text_results(mr):
            c = r["cells"]
            idx = int(c["well"])
            target = (c.get("detector") or c.get("target name") or "").strip()
            x = rec(idx, target)
            x["tm"] = [v for v in (num(t) for t in re.split(r"[,;]", c.get("tm", ""))) if v is not None]
            t = _nums(r["series"].get("sample temperatures", []))
            f = _nums(r["series"].get("rn values", []))
            n = min(len(t), len(f))
            x["melt_n"] = n
            x["melt_sum"] = fsum(f[:n])
            x["melt_t_sum"] = fsum(t[:n])
    # JSON results
    aj = read("primary/analysis_result.json")
    if aj:
        a = json.loads(aj)  # Python's json accepts the bare NaN these files contain
        for w in a.get("wellResults", []):
            for rr in w.get("reactionResults", []):
                x = rec(w["wellIndex"], rr["targetName"])
                if x["sample"] is None:
                    x["sample"] = w.get("sampleName")
                amp = rr.get("amplificationResult") or {}
                cq = amp.get("cq")
                if cq is not None and math.isfinite(cq) and cq >= 0:
                    x["cq"] = cq
                elif "cq" in amp:
                    x["cq_undetermined"] = True
                x["threshold"] = amp.get("ctThreshold")
                x["baseline"] = [amp.get("ctBaselineStart"), amp.get("ctBaselineEnd")]
                x["amp_status"] = amp.get("ampStatus")
                rn = [v for v in amp.get("rn", []) if v is not None]
                x["amp_n"] = len(amp.get("rn", []))
                x["amp_sum"] = fsum(rn)
                if amp.get("deltaRn"):
                    x["corrected_sum"] = fsum(amp["deltaRn"])
    sc = read("extensions/am.sc/standard_curve_result.json")
    if sc:
        out["standard_curves"] = [{k: c.get(k) for k in ("targetName", "dye", "slope", "yIntercept", "r2", "efficiency")} for c in json.loads(sc).get("standardCurves", [])]
    # setup-only wells
    for (idx, target) in wells_setup:
        rec(idx, target)
    out["records"] = sorted(records.values(), key=lambda r: (r["position"], r["target"] or ""))
    # thermal program: annealing / read temperature and cycles
    tc = read("apldbio/sds/tcprotocol.xml")
    rm = read("setup/run_method.json")
    if rm:
        m = json.loads(rm)
        for st in m.get("stages", []):
            if st.get("repeat", 1) > 1:
                out["cycles"] = st["repeat"]
                for s in st.get("steps", []):
                    if (s.get("hold") or {}).get("collectionProfile"):
                        out["acquisition_temperature_c"] = s["ramp"]["temperature"]
    elif tc:
        root = ET.fromstring(tc)
        for st in root.findall("TCStage"):
            if st.findtext("StageFlag") == "CYCLING":
                out["cycles"] = int(st.findtext("NumOfRepetitions"))
                reads = [num(s.findtext("Temperature")) for s in st.findall("TCStep") if s.findtext("CollectionFlag") == "1"]
                out["acquisition_temperatures_c"] = reads
                if len(reads) == 1:  # one read step: the annealing/extension temperature
                    out["acquisition_temperature_c"] = reads[0]
    # second reader of the multicomponent signal: qslib (EUPL-1.2, black box)
    try:
        import qslib
        ex = qslib.Experiment.from_file(str(p))
        mc = ex.multicomponent_data
        # the amplification stage: the one with the most distinct cycles
        stage = int(mc.groupby("stage")["cycle"].nunique().idxmax())
        mc = mc[mc["stage"] == stage]
        dyes = [c for c in mc.columns if c not in ("temperature", "stage", "cycle", "step", "point")]
        sig = {}
        for well, g in mc.groupby(level=0):
            sig[str(well)] = {d: {"n": int(g[d].notna().sum()), "sum": fsum(g[d].tolist())} for d in dyes}
        out["qslib_multicomponent"] = {"reader": "qslib (EUPL-1.2, black box)", "stage": stage, "wells": sig}
    except Exception as e:  # qslib does not read every layout (7500 files, some StepOne files)
        out["qslib_multicomponent"] = {"error": f"{type(e).__name__}: {str(e)[:200]}"}
    return out


# ------------------------------------------------------------------ vendor Results export (.xls/.xlsx)

def export_results(path: Path) -> dict:
    """The `Results` sheet of a QuantStudio / ViiA 7 / 7500 results export: a key/value header,
    then a table whose header row starts with `Well`. Returns {"wells": {(index0, target): {...}}}
    with the vendor's CT ("Undetermined" or a number), Amp Status and Ct Mean."""
    if path.read_bytes()[:4] == b"PK\x03\x04":
        import openpyxl
        wb = openpyxl.load_workbook(path, read_only=True, data_only=True)
        rows = [list(r) for r in wb["Results"].iter_rows(values_only=True)]
        reader = "openpyxl " + openpyxl.__version__ + " (MIT)"
    else:
        import xlrd
        sh = xlrd.open_workbook(str(path)).sheet_by_name("Results")
        rows = [sh.row_values(i) for i in range(sh.nrows)]
        reader = "xlrd " + xlrd.__version__ + " (BSD-3-Clause)"
    hi = next(i for i, r in enumerate(rows) if r and str(r[0]).strip() == "Well")
    head = [str(c).strip() for c in rows[hi]]

    def col(*names):
        return next((i for i, h in enumerate(head) if h in names), None)

    ct, tn, amp, mean, pos = col("CT", "Cт", "Ct", "Cq", "CЃ"), col("Target Name"), col("Amp Status"), col("Ct Mean", "Cт Mean", "Cq Mean"), col("Well Position")
    wells = {}
    header0 = {str(r[0]).strip(): str(r[1]).strip() for r in rows[:hi] if r and len(r) > 1 and str(r[0]).strip()}
    block = header0.get("Block Type", "")
    columns = 24 if "384" in block else 8 if block.startswith("48") else 12
    for r in rows[hi + 1:]:
        if not r or r[0] in ("", None):
            continue
        try:
            w = int(float(r[0])) - 1  # the export counts wells from 1
        except (TypeError, ValueError):
            # StepOne Software names wells (`A1`, `H12`)
            m = re.fullmatch(r"([A-P])(\d{1,2})", str(r[0]).strip())
            if not m:
                continue
            w = (ord(m.group(1)) - 65) * columns + int(m.group(2)) - 1
        t = str(r[tn]).strip() if tn is not None else None
        v = r[ct] if ct is not None else None
        rec = {"well": str(r[pos]).strip() if pos is not None else None}
        if isinstance(v, str) and v.strip().lower() == "undetermined":
            rec["undetermined"] = True
        else:
            rec["ct"] = num(v)
        if amp is not None and str(r[amp]).strip():
            rec["amp_status"] = str(r[amp]).strip()
        if mean is not None:
            rec["ct_mean"] = num(r[mean])
        wells[(w, t)] = rec
    header = {str(r[0]).strip(): str(r[1]).strip() for r in rows[:hi] if r and len(r) > 1 and str(r[0]).strip()}
    return {"reader": reader, "wells": wells, "instrument_type": header.get("Instrument Type"),
            "experiment_type": header.get("Experiment Type")}


def attach_export(out: dict, path: Path) -> None:
    """Add the export's CT, Undetermined and Amp Status to the records of an .eds oracle."""
    ex = export_results(path)
    by_well = {}
    for (w, t), v in ex["wells"].items():
        by_well.setdefault(w, []).append((t, v))
    matched = und = 0
    for rec in out.get("records", []):
        w = rec["position"] - 1
        v = ex["wells"].get((w, rec["target"]))
        if v is None and len(by_well.get(w, [])) == 1 and by_well[w][0][0] is None:
            v = by_well[w][0][1]  # the export has no Target Name column: one result per well
        if v is None:
            continue
        matched += 1
        if v.get("undetermined"):
            rec["export_undetermined"] = True
            und += 1
        elif v.get("ct") is not None:
            rec["export_ct"] = v["ct"]
        if "amp_status" in v:
            rec["export_amp_status"] = v["amp_status"]
        if v.get("ct_mean") is not None:
            rec["export_ct_mean"] = v["ct_mean"]
    out["export"] = {"file": path.name, "sha256": sha256(path), "reader": ex["reader"], "wells": len(ex["wells"]),
                     "matched_records": matched, "undetermined": und, "instrument_type": ex["instrument_type"],
                     "experiment_type": ex["experiment_type"]}


# ------------------------------------------------------------------ .rex (ElementTree)

def rex(p: Path) -> dict:
    root = ET.fromstring(p.read_bytes())
    tubes = {}
    for s in root.findall("Samples/Page/Sample"):
        pos = int(float(s.findtext("TubePosition") or 0))
        tubes[pos] = {"sample": (s.findtext("Name") or "").strip() or None, "type_code": (s.findtext("Type") or "").strip()}
    channels = []
    for c in root.findall("RawChannels/RawChannel"):
        readings = [[float(v) for v in (r.text or "").split()] for r in c.findall("Reading")]
        channels.append({"name": c.findtext("Name"), "readings": [{"tube": i + 1, "n": len(r), "sum": fsum(r)} for i, r in enumerate(readings)]})
    cyc = root.find("Profile/Cycle")
    acq = None
    if cyc is not None:
        for pt in cyc.findall("NormalCyclePoint"):
            if pt.find("AcquireTo") is not None:
                acq = num(pt.findtext("Temperature"))
    return {"reader": "Python stdlib ElementTree", "format": "rotor-gene-rex", "tubes": tubes, "channels": channels,
            "cycles": int(cyc.findtext("RepeatCount")) if cyc is not None else None, "acquisition_temperature_c": acq}


def finite(o):
    """NaN and infinities (bare in the vendor's JSON) become null: the ground truth is strict JSON."""
    if isinstance(o, float) and not math.isfinite(o):
        return None
    if isinstance(o, dict):
        return {k: finite(v) for k, v in o.items()}
    if isinstance(o, (list, tuple)):
        return [finite(v) for v in o]
    return o


def main():
    args = sys.argv[1:]
    if args and args[0] == "--validate-rdml":
        import rdmlpython as rp
        for f in args[1:]:
            r = rp.Rdml(f)
            res = r.validate()
            print(f, "valid" if "RDML file is valid" in res else "INVALID", res.strip().splitlines()[-1] if res.strip() else "")
            if "RDML file is valid" not in res:
                sys.exit(1)
        return
    OUT.mkdir(parents=True, exist_ok=True)
    todo = []
    if args and args[0] == "--manifest":
        import tomllib
        root = Path(__file__).resolve().parent.parent
        corpus = Path(os.environ.get("OPENREADOUT_CORPUS_DIR", root / "corpus" / "files"))
        only = args[1:]
        entries = tomllib.loads((root / "corpus" / "manifest.toml").read_text())["file"]
        exports = {e["id"]: e for e in entries if e.get("role") == "oracle-export" and e.get("tier") != "heldout"}
        for e in entries:
            if (e.get("role") != "input" or e.get("tier") == "heldout"
                    or e["format"] not in ("rdml", "applied-biosystems-eds", "rotor-gene-rex")
                    or (only and not any(o in e["id"] for o in only))):
                continue
            p = corpus / e["filename"]
            if not p.exists():
                print("missing", e["id"])
                continue
            x = exports.get(e["id"] + "-xls")
            todo.append((e["id"], p, corpus / x["filename"] if x else None))
        args = []
    export = None
    while args:
        a = args.pop(0)
        if a == "--export":
            export = Path(args.pop(0))
        elif a == "--id":
            todo.append((args.pop(0), Path(args.pop(0)), export))
            export = None
        else:
            todo.append((None, Path(a), export))
            export = None
    for given, p, export in todo:
        ext = p.suffix.lower()
        try:
            data = {".rdml": rdml, ".rdm": rdml, ".lc96p": rdml, ".eds": eds, ".rex": rex}[ext](p)
            if export is not None:
                attach_export(data, export)
        except Exception as e:
            data = {"error": f"{type(e).__name__}: {e}"}
        fid = given or p.stem
        head = {"id": fid, "file": p.name, "size": p.stat().st_size, "sha256": sha256(p)}
        text = json.dumps(finite({**head, **data}), indent=1, default=str, allow_nan=False)
        oracle_json.write_text(OUT / f"{fid}.json", text + "\n")
        n = len(data.get("records", data.get("channels", [])))
        print("wrote", f"{fid}.json", data.get("error", f"{n} records"))


if __name__ == "__main__":
    main()
