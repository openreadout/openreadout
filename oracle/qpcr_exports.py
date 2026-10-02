"""Ground truth for Applied Biosystems StepOne/7500-layout `.eds` runs from the vendor's Results
export of the same run (StepOne Software `.xls`, wells named `A1`), read with xlrd, not with our
Rust code. One record per exported well x target: sample, target, task, the Ct as written
(a number or `Undetermined`), Ct Mean, Ct SD, Automatic Ct, Ct Threshold, Automatic Baseline,
Baseline Start/End, Tm1-Tm3. Written to corpus/oracle/qpcr-exports/<id>.json; checked by
crates/openreadout-corpus-tests/tests/qpcr_exports.rs.

Usage: qpcr_exports.py ID EXPORT.xls
"""
import json
import sys
from pathlib import Path

import xlrd

OUT = Path(__file__).resolve().parent.parent / "corpus" / "oracle" / "qpcr-exports"


def cell(v):
    if isinstance(v, float):
        return v
    s = str(v).strip()
    return s or None


def read(path: Path) -> dict:
    sheet = xlrd.open_workbook(path).sheet_by_name("Results")
    header, records, meta = None, [], {}
    for i in range(sheet.nrows):
        row = [cell(c.value) for c in sheet.row(i)]
        if header is None:
            if row and row[0] == "Well":
                header = [str(h) if h is not None else "" for h in row]
            elif row and row[0] and len(row) > 1 and row[1] is not None and row[0] != "Experiment File Name":
                meta[str(row[0])] = row[1]
            continue
        if not row or row[0] is None:
            break
        r = dict(zip(header, row))
        if not r.get("Target Name"):
            continue  # an empty well
        ct_col = next(h for h in header if h in ("Cт", "Ct", "CT"))
        ct = r.get(ct_col)
        rec = {
            "well": r["Well"],
            "sample": r.get("Sample Name"),
            "target": r.get("Target Name"),
            "task": r.get("Task"),
            "ct": ct,
            "ct_mean": r.get(ct_col + " Mean"),
            "ct_sd": r.get(ct_col + " SD"),
            "automatic_ct": r.get("Automatic Ct Threshold") or r.get("Automatic Ct"),
            "threshold": r.get("Ct Threshold"),
            "automatic_baseline": r.get("Automatic Baseline"),
            "baseline_start": r.get("Baseline Start"),
            "baseline_end": r.get("Baseline End"),
            "tm": [r[k] for k in ("Tm1", "Tm2", "Tm3") if isinstance(r.get(k), float)],
        }
        records.append(rec)
    return {"reader": f"xlrd {xlrd.__version__}", "export": path.name, "header": meta, "records": records}


def main():
    ident, export = sys.argv[1], Path(sys.argv[2])
    out = {"id": ident, **read(export)}
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / f"{ident}.json").write_text(json.dumps(out, indent=1, ensure_ascii=False) + "\n")
    print(ident, len(out["records"]))


if __name__ == "__main__":
    main()
