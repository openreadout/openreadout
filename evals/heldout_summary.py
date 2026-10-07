"""Summarize a held-out probe run (evals/heldout_probe.py JSON) for a generalization report.

    uv run python heldout_summary.py <reports>/heldout-<date>.json \\
        [--classes <reports>/heldout-<date>-classes.json] [--draw 2026-09-26c]

Per held-out input it takes one verdict. Without an adjudication, from the run alone:
  - `agree`: the deep comparison (`heldout_agreement_is_recorded`) passed and the probe's field
    comparison found no disagreement;
  - `self-consistency`: it passed against an oracle that is a second implementation of our own
    notes (`self-consistency` in the detail);
  - `disagree`: the deep comparison FAILed or PANICked, a probe field disagreed, or `info` failed;
  - `no-oracle`: no usable ground truth (no oracle JSON, or the oracle itself failed).
An adjudication (`--classes`, JSON {id: class}; written by a person, recorded next to the report)
overrides it for the files it names: `reader` (OpenReadout is wrong or incomplete), `unresolved`
(no independent reader can settle it yet; counted like `reader`), `oracle` (the ground truth or its
converter is wrong or follows another convention), `gap` (the reader refuses an unsupported variant
with exit 6 and there is no oracle), `no-oracle`, `self`.

Two agreement rates, over the files with an independent oracle:
  - raw: agree / (agree + reader + unresolved + oracle + unadjudicated disagreements), as the first
    two reports counted;
  - adjudicated: (agree + oracle) / the same, i.e. only reader errors and unresolved cases count
    against OpenReadout.
The assurance tables set each file's `assurance.level` (docs/assurance.md) against its class: how
many reader errors `--strict` would have refused (level `unvalidated`) and how many correct files
it refuses too.

Exposed inputs (manifest `exposed`: their source record was developed on before a draw reserved it,
docs/benchmark/heldout.md) are left out of every table and rate above and reported on their own at
the end: they are measured, but they do not count toward generalization.
"""

from __future__ import annotations

import argparse
import collections
import json
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LEVELS = ["validated", "partially_validated", "unvalidated", "none"]


def verdict(row: dict) -> str:
    ct = row.get("corpus_test") or {}
    st = ct.get("status")
    if row.get("status") in ("panic", "fails"):
        return "disagree"
    if st in ("FAIL", "PANIC") or row.get("disagree"):
        return "disagree"
    if st == "pass":
        return "self-consistency" if "self-consistency" in ct.get("detail", "") else "agree"
    return "no-oracle"


CLASS_TO_VERDICT = {
    "reader": "reader",
    "unresolved": "unresolved",
    "oracle": "oracle",
    "gap": "gap",
    "no-oracle": "no-oracle",
    "self": "self-consistency",
}


def classify(row: dict, classes: dict) -> str:
    if row["id"] in classes:
        return CLASS_TO_VERDICT[classes[row["id"]]]
    return verdict(row)


def rates(c: collections.Counter) -> tuple[str, str]:
    bad = c["reader"] + c["unresolved"] + c["disagree"]
    n = c["agree"] + c["oracle"] + bad
    if not n:
        return "-", "-"
    return (
        f"{c['agree']}/{n} ({100 * c['agree'] / n:.1f} %)",
        f"{c['agree'] + c['oracle']}/{n} ({100 * (c['agree'] + c['oracle']) / n:.1f} %)",
    )


def table(title: str, groups: dict[str, collections.Counter]) -> None:
    cols = ["agree", "oracle", "reader", "unresolved", "disagree", "gap", "self-consistency", "no-oracle"]
    print(f"| {title} | inputs | " + " | ".join(cols) + " | raw agreement | adjudicated |")
    print("| --- | --- | " + " | ".join("---" for _ in cols) + " | --- | --- |")
    tot = collections.Counter()
    for k, c in sorted(groups.items()):
        tot += c
        r, a = rates(c)
        print(f"| {k} | {sum(c.values())} | " + " | ".join(str(c[x]) for x in cols) + f" | {r} | {a} |")
    r, a = rates(tot)
    print(f"| all | {sum(tot.values())} | " + " | ".join(str(tot[x]) for x in cols) + f" | {r} | {a} |")
    print()


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("probe")
    ap.add_argument("--draw", help="only this draw (manifest `draw`)")
    ap.add_argument("--classes", help="JSON {id: reader|unresolved|oracle|gap|no-oracle|self}")
    args = ap.parse_args()
    probe = json.loads(Path(args.probe).read_text())
    manifest = {
        e["id"]: e
        for e in tomllib.loads((ROOT / "corpus/manifest.toml").read_text())["file"]
        if e.get("role") == "heldout"
    }
    classes = (
        {k: v for k, v in json.loads(Path(args.classes).read_text()).items() if not k.startswith("_")}
        if args.classes
        else {}
    )
    drawn = [r for r in probe["files"] if not args.draw or manifest.get(r["id"], {}).get("draw") == args.draw]
    exposed = [r for r in drawn if manifest.get(r["id"], {}).get("exposed")]
    rows = [r for r in drawn if not manifest.get(r["id"], {}).get("exposed")]
    by_draw: dict[str, collections.Counter] = collections.defaultdict(collections.Counter)
    by_family: dict[str, collections.Counter] = collections.defaultdict(collections.Counter)
    by_level: dict[str, collections.Counter] = collections.defaultdict(collections.Counter)
    for r in rows:
        v = classify(r, classes)
        by_draw[manifest.get(r["id"], {}).get("draw", "?")][v] += 1
        by_family[r.get("family") or "?"][v] += 1
        by_level[(r.get("assurance") or {}).get("level") or "none"][v] += 1
    table("draw", by_draw)
    table("family", by_family)
    table("assurance level", by_level)
    # --strict as a predictor of reader errors (files with an independent verdict)
    judged = [r for r in rows if classify(r, classes) in ("agree", "oracle", "reader", "unresolved", "disagree")]
    unval = [r for r in judged if (r.get("assurance") or {}).get("level") == "unvalidated"]
    errors = [r for r in judged if classify(r, classes) in ("reader", "unresolved", "disagree")]
    caught = [r for r in errors if (r.get("assurance") or {}).get("level") == "unvalidated"]
    print(
        f"files with an independent verdict: {len(judged)}; unvalidated: {len(unval)}; "
        f"reader errors (incl. unresolved): {len(errors)}"
    )
    print(
        f"recall (reader errors --strict refuses): {len(caught)}/{len(errors)}: " + ", ".join(r["id"] for r in caught)
    )
    print(
        "missed: "
        + ", ".join(f"{r['id']} ({(r.get('assurance') or {}).get('level')})" for r in errors if r not in caught)
    )
    print(f"precision (unvalidated files that are reader errors): {len(caught)}/{len(unval)}")
    ok_unval = [r for r in unval if classify(r, classes) in ("agree", "oracle")]
    print(
        f"correct but unvalidated (refused by --strict although right): {len(ok_unval)}: "
        + ", ".join(r["id"] for r in ok_unval)
    )
    refused = [r for r in rows if (r.get("info") or {}).get("exit") == 6]
    print(f"refused by the reader itself (exit 6): {len(refused)}: " + ", ".join(r["id"] for r in refused))
    if exposed:
        print()
        print(
            f"exposed inputs, not counted above ({len(exposed)}; their source record was developed on "
            "before the draw reserved it):"
        )
        by_exposed: dict[str, collections.Counter] = collections.defaultdict(collections.Counter)
        for r in exposed:
            by_exposed[r.get("family") or "?"][classify(r, classes)] += 1
        table("exposed (family)", by_exposed)
        for r in exposed:
            print(f"- {r['id']}: {classify(r, classes)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
