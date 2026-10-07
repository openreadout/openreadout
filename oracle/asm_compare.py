#!/usr/bin/env python
"""Compare `openreadout export --format asm` with allotropy's ASM for the same plate-reader exports.

Usage: uv run python asm_compare.py [--bin PATH] [--corpus DIR] [--only SUBSTR] [--json OUT]

For every `format = "plate"` corpus file: write our ASM with the binary, validate it against the
public ASM schema (asm_validate.py; also the stricter per-detector pass), run allotropy (MIT) on the
same file, validate its output the same way, and diff the two field by field:
  - document counts (plate reader documents, measurement documents, calculated-data documents);
  - measurement documents matched by (well, measure field, ordinal within that well and field);
    for each matched pair every leaf path (array indices dropped, identifiers ignored) is
    `same`, `differs`, `only ours` or `only allotropy`;
  - the device system and data system documents, leaf by leaf.
Tecan i-control exports have no allotropy parser: only our schema validation is reported.
"""
import argparse, collections, json, math, os, subprocess, sys, tempfile, tomllib
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
sys.path.insert(0, str(HERE))
import asm_validate, plate  # noqa: E402

IGNORE = {"measurement identifier", "calculated data identifier", "data source identifier", "ASM file identifier",
          "UNC path", "ASM converter name", "ASM converter version", "file name"}
MEASURES = set(asm_validate.DETECTORS)


def leaves(o, prefix=""):
    if isinstance(o, dict):
        for k, v in o.items():
            if k in IGNORE:
                continue
            yield from leaves(v, f"{prefix}/{k}" if prefix else k)
    elif isinstance(o, list):
        if o and all(isinstance(x, (int, float)) or x is None for x in o):
            yield prefix, tuple(o)
        else:
            for x in o:
                yield from leaves(x, prefix)
    else:
        yield prefix, o


def same(a, b):
    if isinstance(a, float) or isinstance(b, float):
        try:
            return math.isclose(float(a), float(b), rel_tol=1e-9, abs_tol=1e-12)
        except (TypeError, ValueError):
            return False
    return a == b


def measurements(asm):
    out = {}
    counter = collections.Counter()
    for d in asm["plate reader aggregate document"].get("plate reader document", []):
        for m in d["measurement aggregate document"]["measurement document"]:
            well = m.get("sample document", {}).get("location identifier")
            field = next((k for k in m if k in MEASURES), None)
            key = (well, field, counter[(well, field)])
            counter[(well, field)] += 1
            # the aggregate's fields apply to every measurement in it
            agg = {k: v for k, v in d["measurement aggregate document"].items() if k != "measurement document"}
            out[key] = {**m, "(measurement aggregate)": agg}
    return out


def compare(ours, theirs):
    stats = collections.defaultdict(collections.Counter)
    examples = {}
    mo, mt = measurements(ours), measurements(theirs)
    matched = set(mo) & set(mt)
    for key in matched:
        lo = collections.defaultdict(list)
        lt = collections.defaultdict(list)
        for p, v in leaves(mo[key]):
            lo[p].append(v)
        for p, v in leaves(mt[key]):
            lt[p].append(v)
        for p in set(lo) | set(lt):
            if p not in lt:
                stats[p]["only ours"] += 1
            elif p not in lo:
                stats[p]["only allotropy"] += 1
            elif len(lo[p]) == len(lt[p]) and all(same(a, b) for a, b in zip(lo[p], lt[p])):
                stats[p]["same"] += 1
            else:
                stats[p]["differs"] += 1
                examples.setdefault(p, (lo[p][:2], lt[p][:2]))
    for doc in ("device system document", "data system document"):
        a = dict(leaves(ours["plate reader aggregate document"].get(doc, {})))
        b = dict(leaves(theirs["plate reader aggregate document"].get(doc, {})))
        for p in set(a) | set(b):
            key = f"{doc}/{p}"
            if p not in b:
                stats[key]["only ours"] += 1
            elif p not in a:
                stats[key]["only allotropy"] += 1
            else:
                stats[key]["same" if same(a[p], b[p]) else "differs"] += 1
                if not same(a[p], b[p]):
                    examples.setdefault(key, (a[p], b[p]))
    counts = {}
    for name, asm in (("ours", ours), ("allotropy", theirs)):
        agg = asm["plate reader aggregate document"]
        counts[name] = {
            "documents": len(agg.get("plate reader document", [])),
            "measurements": sum(len(d["measurement aggregate document"]["measurement document"]) for d in agg.get("plate reader document", [])),
            "calculated": len(agg.get("calculated data aggregate document", {}).get("calculated data document", [])),
        }
    return {"counts": counts, "matched_measurements": len(matched), "only_ours_measurements": len(set(mo) - set(mt)),
            "only_allotropy_measurements": len(set(mt) - set(mo)), "fields": {p: dict(c) for p, c in sorted(stats.items())},
            "examples": {p: [str(x)[:160] for x in v] for p, v in sorted(examples.items())}}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", default=str(ROOT / "target" / "release" / "openreadout"))
    ap.add_argument("--corpus", default=os.environ.get("OPENREADOUT_CORPUS_DIR", str(ROOT / "corpus" / "files")))
    ap.add_argument("--only")
    ap.add_argument("--json")
    a = ap.parse_args()
    manifest = tomllib.loads((ROOT / "corpus" / "manifest.toml").read_text())
    entries = [e for e in manifest["file"] if e["format"] == "plate" and (not a.only or a.only in e["id"])]
    report = {}
    totals = collections.defaultdict(collections.Counter)
    with tempfile.TemporaryDirectory() as td:
        for e in entries:
            src = Path(a.corpus) / e["filename"]
            if not src.exists():
                continue
            out = Path(td) / f"{e['id']}.asm.json"
            r = subprocess.run([a.bin, "export", str(src), "--format", "asm", "-o", str(out), "--json"], capture_output=True, text=True)
            if r.returncode != 0:
                report[e["id"]] = {"error": r.stdout[-500:] + r.stderr[-500:]}
                print(f"{e['id']}: export failed"); continue
            ours = json.loads(out.read_text())
            rec = {"ours_schema_errors": asm_validate.errors(ours)}
            vendor = plate.vendor_for(e["id"])
            if vendor:
                try:
                    theirs = plate.allotropy_asm(src, vendor)
                    rec["allotropy_schema_errors"] = asm_validate.errors(theirs)
                    rec.update(compare(ours, theirs))
                    for p, c in rec["fields"].items():
                        totals[p].update(c)
                except Exception as ex:  # an allotropy failure is information too
                    rec["allotropy_error"] = f"{type(ex).__name__}: {ex}"
            report[e["id"]] = rec
            c = rec.get("counts", {})
            diffs = sum(v.get("differs", 0) for v in rec.get("fields", {}).values())
            print(f"{e['id']:<40} schema(ours) {'VALID' if not rec['ours_schema_errors'] else 'INVALID ' + rec['ours_schema_errors'][0]}"
                  + (f" | allotropy {'VALID' if not rec.get('allotropy_schema_errors') else 'INVALID'}" if 'allotropy_schema_errors' in rec else "")
                  + (f" | docs {c['ours']['documents']}/{c['allotropy']['documents']} meas {c['ours']['measurements']}/{c['allotropy']['measurements']}"
                     f" calc {c['ours']['calculated']}/{c['allotropy']['calculated']} matched {rec['matched_measurements']} differing leaves {diffs}" if c else "")
                  + (f" | allotropy error: {rec['allotropy_error'][:120]}" if 'allotropy_error' in rec else ""))
    print("\nfield-level totals over matched measurement documents (same / differs / only ours / only allotropy):")
    for p, c in sorted(totals.items(), key=lambda kv: -sum(kv[1].values())):
        print(f"  {p:<95} {c.get('same', 0):>6} {c.get('differs', 0):>6} {c.get('only ours', 0):>6} {c.get('only allotropy', 0):>6}")
    if a.json:
        Path(a.json).write_text(json.dumps({"files": report, "totals": {p: dict(c) for p, c in totals.items()}}, indent=1))
    bad = [k for k, v in report.items() if v.get("error") or v.get("ours_schema_errors")]
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
