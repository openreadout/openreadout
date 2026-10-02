#!/usr/bin/env python
"""Tables of docs/benchmark/second-opinions.md from the second-opinion test's report.

    SECOND_REPORT=/tmp/second.jsonl cargo test -p openreadout-corpus-tests --features corpus \\
        --release --test second_fields
    python oracle/second_report.py /tmp/second.jsonl [docs/benchmark/second-opinions.md]

The report has one line per check (`id, format, family, field, reader, status, normalized,
right, why`). The script rewrites the text between `<!-- BEGIN GENERATED -->` and
`<!-- END GENERATED -->` of the page: per family and format, files, checks, agreements,
adjudicated differences (ours right / theirs right / neither), gaps (theirs only), and the files
whose acquisition time and instrument model a second reader confirmed. The prose around it is
written by hand.
"""
from __future__ import annotations

import collections
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BEGIN, END = "<!-- BEGIN GENERATED -->", "<!-- END GENERATED -->"
FAMILY_NAMES = {
    "mass-spectrometry": "Mass spectrometry",
    "chromatography": "Chromatography",
    "flow-cytometry": "Flow cytometry",
    "electrophysiology": "Electrophysiology",
    "nmr": "NMR",
    "vibrational-spectroscopy": "Vibrational spectroscopy",
    "plate-reader": "Plate readers",
    "qpcr": "Real-time PCR",
    "bench": "Bench instruments (EPR, XRD, electrochemistry, thermal, FPLC, SPR, ITC, microarrays)",
    "imaging": "Image formats (metadata)",
}
TIME, MODEL = "acquisition.started_at", "instrument.model"


def tables(rows: list[dict]) -> str:
    fams: dict[str, dict[str, collections.Counter]] = collections.defaultdict(lambda: collections.defaultdict(collections.Counter))
    files: dict[tuple[str, str], set] = collections.defaultdict(set)
    readers: dict[str, set] = collections.defaultdict(set)
    confirmed: dict[tuple[str, str, str], set] = collections.defaultdict(set)
    for r in rows:
        fam, fmt = r["family"], r["format"]
        c = fams[fam][fmt]
        files[(fam, fmt)].add(r["id"])
        readers[fam].add(r["reader"].split(" (")[0])
        c["checks"] += 1
        st, right = r["status"], r.get("right")
        if st == "agree":
            c["agree"] += 1
        elif st in ("differ", "error"):
            key = {"openreadout": "ours", "second": "theirs", "neither": "neither"}.get(right or "", "open")
            c[key] += 1
        else:
            c["gap"] += 1
        n = r.get("normalized")
        if n in (TIME, MODEL) and st == "agree":  # as the test: only agreement confirms a field
            confirmed[(fam, fmt, n)].add(r["id"])
    out = []
    tot = collections.Counter()
    for fam in sorted(fams, key=lambda f: list(FAMILY_NAMES).index(f) if f in FAMILY_NAMES else 99):
        out.append(f"### {FAMILY_NAMES.get(fam, fam)}\n")
        out.append("Second readers: " + ", ".join(sorted(readers[fam])) + ".\n")
        out.append("| format | files | checks | agree | ours right | theirs right | neither | unadjudicated | gaps | time confirmed | model confirmed |")
        out.append("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |")
        for fmt, c in sorted(fams[fam].items()):
            nt = len(confirmed.get((fam, fmt, TIME), ()))
            nm = len(confirmed.get((fam, fmt, MODEL), ()))
            nf = len(files[(fam, fmt)])
            out.append(f"| `{fmt}` | {nf} | {c['checks']} | {c['agree']} | {c['ours']} | {c['theirs']} | {c['neither']} | {c['open']} | {c['gap']} | {nt} | {nm} |")
            tot.update(c)
            tot["files"] += nf
            tot["time"] += nt
            tot["model"] += nm
        out.append("")
    head = (f"All families: {tot['files']} files, {tot['checks']} checks, {tot['agree']} agree, "
            f"{tot['ours'] + tot['theirs'] + tot['neither']} adjudicated differences ({tot['ours']} ours right, "
            f"{tot['theirs']} theirs right, {tot['neither']} neither), {tot['open']} unadjudicated, {tot['gap']} gaps; "
            f"acquisition time confirmed on {tot['time']} files, instrument model on {tot['model']}.\n")
    return head + "\n" + "\n".join(out)


def main(argv: list[str]) -> int:
    report = Path(argv[1])
    page = Path(argv[2]) if len(argv) > 2 else ROOT / "docs" / "benchmark" / "second-opinions.md"
    rows = [json.loads(l) for l in report.read_text().splitlines() if l.strip()]
    text = page.read_text()
    a, b = text.index(BEGIN) + len(BEGIN), text.index(END)
    page.write_text(text[:a] + "\n\n" + tables(rows) + "\n" + text[b:])
    print(f"{page}: {len(rows)} checks")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
