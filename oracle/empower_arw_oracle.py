"""Ground truth for Waters Empower ASCII exports (`.arw`) from Appia's own reading of the same
files (Appia, MIT, https://github.com/PlethoraChutney/Appia: its `processed-tests/*-wide.csv`,
one column `<SampleName> <channel>` per export, read with the `csv` module). The export's header
row is read here only to find the sample name and channel that name Appia's column.

    python empower_arw_oracle.py --id ID EXPORT.arw APPIA_WIDE.csv   -> ../corpus/oracle/<id>.json

The comparison is the generic trace comparison of the corpus test (every value's xxh3, first
and last retention time).
"""
import argparse
import csv
import hashlib
import json
from pathlib import Path

import gen  # the trace-record helper
import oracle_json

OUT = Path(__file__).resolve().parent.parent / "corpus" / "oracle"


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--id", required=True)
    ap.add_argument("export")
    ap.add_argument("appia")
    a = ap.parse_args()
    p = Path(a.export)
    text = p.read_bytes().decode("utf-8", errors="replace").replace("\r\n", "\n").replace("\r", "\n")
    lines = [l for l in text.split("\n") if l.strip()]
    meta = dict(zip([x.strip('"') for x in lines[0].split("\t")], [x.strip('"') for x in lines[1].split("\t")]))
    rows = list(csv.reader(open(a.appia, newline="")))
    head = rows[0]
    # Appia names the column "<SampleName> <channel without the detector prefix>"
    col = head.index(meta["SampleName"] + " " + meta["Channel"].split(" ", 1)[1])
    pts = [(float(r[0]), float(r[col])) for r in rows[1:] if len(r) > col and r[col] != ""]
    x = [t for t, _ in pts]
    y = [v for _, v in pts]
    trace = gen._chrom_trace(0, [meta["Channel"]], [""], x, [y])
    trace["name"] = f"{meta['SampleName']} / {meta['Channel']}"
    out = {
        "id": a.id,
        "file": p.name,
        "size": p.stat().st_size,
        "sha256": hashlib.sha256(p.read_bytes()).hexdigest(),
        "reader": "Appia (MIT) processed output (processed-tests/*-wide.csv)",
        "oracle_note": "values from Appia's wide CSV column; Appia's times are the exported ones in single precision (within 2e-6 min)",
        "traces": [trace],
    }
    oracle_json.write_text(OUT / f"{a.id}.json", json.dumps(out, indent=1) + "\n")  # over 1 MiB: .json.gz
    print(a.id, len(y), "points")


if __name__ == "__main__":
    main()
