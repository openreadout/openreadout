"""Keep the question counts in evals/README.md in step with evals/questions/*.jsonl.

    uv run python readme_counts.py           # rewrite the counts in README.md
    uv run python readme_counts.py --check   # exit 1 if README.md is out of date

`generate.py` runs it after writing the questions (and `generate.py --check` checks it), so the
counts are never edited by hand: the total and tier sizes in "What is measured", the category
table, the per-family counts, the dev/test split sizes and the held-out split's size.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import Counter
from pathlib import Path

HERE = Path(__file__).resolve().parent
README = HERE / "README.md"
QUESTIONS = HERE / "questions"

FAMILY_NAMES = {
    "microscopy": "microscopy",
    "em": "electron microscopy",
    "flow": "flow cytometry",
    "ephys": "electrophysiology",
    "nmr": "NMR",
    "ms": "mass spectrometry",
    "chromatography": "chromatography",
    "plates": "plate readers",
    "hcs": "high-content screening",
    "qpcr": "qPCR",
    "spectroscopy": "vibrational spectroscopy",
    "bench": "bench instruments",
}


def load(questions_dir: Path = QUESTIONS) -> tuple[list[dict], list[dict]]:
    """(dev + test questions, held-out questions)."""
    main, heldout = [], []
    for p in sorted(questions_dir.glob("*.jsonl")):
        for line in p.read_text().splitlines():
            if line.strip():
                q = json.loads(line)
                (heldout if q.get("split") == "heldout" else main).append(q)
    return main, heldout


def _families(qs: list[dict]) -> str:
    c = Counter(q.get("family") for q in qs)
    order = [f for f in FAMILY_NAMES if c.get(f)] + sorted(f for f in c if f not in FAMILY_NAMES)
    return ", ".join(f"{FAMILY_NAMES.get(f, f)} {c[f]}" for f in order if f)


def refresh(text: str, main: list[dict], heldout: list[dict]) -> str:
    cat = Counter(q["category"] for q in main)
    split = Counter(q.get("split") for q in main)

    def sub(pattern: str, repl, s: str) -> str:
        out, n = re.subn(pattern, repl, s, count=1, flags=re.M)
        if n != 1:
            raise SystemExit(f"README.md: pattern not found: {pattern}")
        return out

    lead = r" questions \(`questions/\*\.jsonl`, not counting the held-out split\): lookups about single corpus files, "
    text = sub(
        r"^\d+" + lead + r"\d+ about whole staged shares, \d+ in the analysis tier",
        str(len(main)) + lead.replace("\\", "") + f"{cat['search']} about whole staged shares, "
        f"{cat['analysis']} in the analysis tier",
        text,
    )
    text = sub(r"(, )\d+( in the quantitation tier)", rf"\g<1>{cat['quantitation']}\g<2>", text)
    text = sub(r"(, )\d+( in the batch tier)", rf"\g<1>{cat['batch']}\g<2>", text)
    text = sub(r"(, )\d+( in the scenario tier)", rf"\g<1>{cat['scenario']}\g<2>", text)
    text = sub(r"( and )\d+( in the visual tier)", rf"\g<1>{cat['visual']}\g<2>", text)

    def row(m: re.Match) -> str:
        name = m.group(1)
        if name not in cat:
            raise SystemExit(f"README.md: category {name!r} has no questions")
        return f"| {name} | {cat[name]} |"

    body = text.split("| category | questions | examples |", 1)
    if len(body) != 2:
        raise SystemExit("README.md: category table not found")
    head, rest = body
    table_end = rest.find("\n\n")
    table, tail = rest[:table_end], rest[table_end:]
    table = re.sub(r"^\| ([a-z-]+) \| \d+ \|", row, table, flags=re.M)
    listed = set(re.findall(r"^\| ([a-z-]+) \| \d+ \|", table, flags=re.M))
    for name in sorted(set(cat) - listed):
        table += f"\n| {name} | {cat[name]} | |"
    text = head + "| category | questions | examples |" + table + tail

    text = sub(r"^By family: .*$", "By family: " + _families(main) + ".", text)
    text = sub(r"(`dev` \()\d+(\) or `test` \()\d+(\))", rf"\g<1>{split['dev']}\g<2>{split['test']}\g<3>", text)
    text = sub(r"(`heldout` \()\d+( questions, `questions/heldout\.jsonl`)", rf"\g<1>{len(heldout)}\g<2>", text)
    return text


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true", help="exit 1 if README.md is out of date")
    args = ap.parse_args(argv)
    old = README.read_text()
    new = refresh(old, *load())
    if args.check:
        if new != old:
            print("evals/README.md question counts are out of date; run evals/readme_counts.py", file=sys.stderr)
            return 1
        print("evals/README.md question counts are up to date")
        return 0
    if new != old:
        README.write_text(new)
        print("evals/README.md: question counts updated")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
