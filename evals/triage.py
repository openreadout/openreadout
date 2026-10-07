"""Failure triage: why each wrong answer was wrong, and which part of OpenReadout to fix.

The optimization loop is: run the dev split, triage, fix the cause, rerun. Every failure gets
one cause from the record alone, and every failure of the `with` condition also gets a fix
area from an offline probe (no model call) that runs OpenReadout on the staged file and looks
for the expected answer in its output:

    cause (first that applies)       meaning
    harness                          the run touched the repository/oracle, or crashed without a result
    turn_cap / budget_cap / timeout  the agent ran out of turns, dollars or time
    no_answer                        no ANSWER line, or "unknown"
    tool_not_used                    `with` only: the agent never called OpenReadout
    output_file                      conversion: the answer was right, the written file failed its check
    unit_slip                        the number is right after a factor of 10^±3, 10^±6, 10^±4 or 60
    off_by_one                       an integer answer one away from the truth
    wrong_answer                     anything else

    fix area (`with` failures)       from the probe
    reader                           neither `info --view full` nor `info --view explain` has it: parser or model gap
    agent                            the answer is in OpenReadout's output but the agent did not get to it:
                                     skill text, MCP tool descriptions, `info --view explain`, output shape, hints
    review                           the probe cannot tell (analysis, conversion, search, booleans)

Only the dev split is triaged by default: test transcripts stay unread, so the test score
stays an honest estimate. `--split test --unseal` overrides that, for a final audit only.

    uv run python triage.py --run-name 2026-09-24-sonnet-5          # results/<run>-triage-dev.md
"""

from __future__ import annotations

import argparse
import json
import math
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path
from typing import Any

import run
import score
import stats

EVALS = Path(__file__).resolve().parent
RESULTS = EVALS / "results"
FACTORS = [1e3, 1e-3, 1e6, 1e-6, 1e4, 1e-4, 60.0, 1 / 60, 1e9, 1e-9]
# Categories whose answers live in per-plane/scan/sweep data (`stats`, `scans`, `spectrum`, `trace`) or in
# written files, not in `info --view full|explain`: the probe cannot tell reader gaps from agent misses there.
NO_PROBE = ("analysis", "batch", "conversion", "search", "values", "quantitation", "visual")


def cause_of(q: dict, rec: dict, verdict: dict) -> str:
    stop = {"error_max_turns": "turn_cap", "error_max_budget_usd": "budget_cap", "timeout": "timeout"}
    if rec.get("contamination"):
        return "harness"
    if rec.get("subtype") in stop:
        return stop[rec["subtype"]]
    if rec.get("final_text") is None:
        return "harness"
    if verdict.get("answer") is None or verdict.get("reason") == "answered unknown":
        return "no_answer"
    if rec.get("condition") == "with" and not rec.get("used_openreadout"):
        return "tool_not_used"
    if "task" in q and score.score_answer(q["answer"], rec.get("final_text")).get("correct"):
        return "output_file"
    ans = q["answer"]
    got = verdict.get("converted", verdict.get("parsed"))
    if ans["type"] == "number" and isinstance(got, int | float):
        expected = float(ans["value"])
        for f in FACTORS:
            if score.within(got * f, expected, ans["tolerance"]):
                return "unit_slip"
        if float(expected).is_integer() and abs(got - expected) == 1:
            return "off_by_one"
    return "wrong_answer"


# ---------------------------------------------------------------- offline probe


def _leaves(doc: Any, path: str = "") -> list[tuple[str, Any]]:
    if isinstance(doc, dict):
        return [x for k, v in doc.items() for x in _leaves(v, f"{path}/{k}")]
    if isinstance(doc, list):
        return [x for i, v in enumerate(doc[:2000]) for x in _leaves(v, f"{path}/{i}")]
    return [(path, doc)]


def find_answer(answer: dict, doc: Any) -> str | None:
    """JSON pointer of a leaf in OpenReadout's output that holds the expected answer, if any."""
    leaves = _leaves(doc)
    t = answer["type"]
    if t == "number":
        expected = float(answer["value"])
        for f in (1.0, *FACTORS):
            for p, v in leaves:
                if isinstance(v, int | float) and not isinstance(v, bool) and math.isfinite(v):
                    if score.within(v * f, expected, answer["tolerance"]):
                        return p + ("" if f == 1.0 else f" (× {f:g})")
        return None
    if t == "date":
        want = answer["value"][:10]
        return next((p for p, v in leaves if isinstance(v, str) and want in v), None)
    if t in ("string", "list"):
        wanted = [answer["value"], *answer.get("accept", [])] if t == "string" else answer["value"]
        text = {p: score.normalize(v) for p, v in leaves if isinstance(v, str)}
        hits = []
        for w in wanted if t == "list" else [None]:
            variants = wanted if t == "string" else [w, *answer.get("variants", {}).get(w, [])]
            hit = next(
                (p for p, v in text.items() for x in variants if score.contains_word(v, score.normalize(x))), None
            )
            if hit is None:
                return None
            hits.append(hit)
            if t == "string":
                break
        return ", ".join(hits)
    return None


def probe(q: dict, binary: Path, corpus: Path) -> dict:
    """Run OpenReadout on the staged question file; does its output contain the answer?"""
    if q["category"] in NO_PROBE or q["answer"]["type"] == "boolean" or "share" in q:
        return {"fix_area": "review", "evidence": f"not probed ({q['category']}, {q['answer']['type']})"}
    with tempfile.TemporaryDirectory(prefix="openreadout-triage-") as tmp:
        wd = Path(tmp)
        run.stage(q, wd, corpus)
        target = wd / q["file"]["stage_as"]
        docs = []
        for cmd in (
            ["info", str(target), "--view", "full", "--vendor", "--json"],
            ["info", str(target), "--view", "explain", "--json"],
        ):
            try:
                out = subprocess.run([str(binary), *cmd], capture_output=True, text=True, timeout=180)
                docs.append(json.loads(out.stdout))
            except (subprocess.TimeoutExpired, json.JSONDecodeError):
                continue
    hit = next((h for d in docs if (h := find_answer(q["answer"], d))), None)
    if hit:
        return {"fix_area": "agent", "evidence": f"in OpenReadout output at {hit}"}
    return {"fix_area": "reader", "evidence": "not found in `info --view full` or `info --view explain`"}


# ---------------------------------------------------------------- report


def triage(run_name: str, conditions: list[str], split: str, binary: Path | None) -> dict:
    questions = {k: v for k, v in score.load_questions().items() if split == "all" or v.get("split") == split}
    recs = stats.load(run_name, conditions)
    corpus = run.corpus_dir() if binary else None
    probes: dict[str, dict] = {}
    failures = []
    for cond, rs in recs.items():
        for rec in sorted(rs, key=lambda r: (r["id"], r.get("rep", 0))):
            q = questions.get(rec["id"])
            if q is None:
                continue
            verdict = score.score_record(q, rec)
            if verdict["correct"]:
                continue
            item = {
                "id": rec["id"],
                "condition": cond,
                "rep": rec.get("rep", 0),
                "family": q["family"],
                "category": q["category"],
                "cause": cause_of(q, rec, verdict),
                "answer": verdict.get("answer"),
                "expected": score.expected_text(q["answer"]),
                "reason": verdict.get("reason"),
                "turns": rec.get("num_turns"),
                "transcript": f"results/transcripts/{run_name}/{rec['id']}.{cond}"
                + (f".r{rec['rep']}" if rec.get("rep") else "")
                + ".jsonl",
            }
            if cond == "with":
                if binary is None:
                    item.update(fix_area="review", evidence="no binary to probe with")
                else:
                    if rec["id"] not in probes:
                        probes[rec["id"]] = probe(q, binary, corpus)
                    item.update(probes[rec["id"]])
            failures.append(item)
    pq = {c: stats.per_question(rs, questions) for c, rs in recs.items()}
    regressions = {}
    if "with" in pq:
        for other in ("baseline", "without"):
            if other in pq:
                regressions[other] = sorted(
                    q for q in set(pq["with"]) & set(pq[other]) if pq["with"][q]["rate"] < pq[other][q]["rate"]
                )
    return {"run_name": run_name, "split": split, "failures": failures, "with_worse_than": regressions}


def markdown(t: dict) -> str:
    fails = t["failures"]
    conds = [c for c in stats.CONDITIONS if any(f["condition"] == c for f in fails)]
    by = Counter((f["cause"], f["condition"]) for f in fails)
    causes = sorted({f["cause"] for f in fails})
    lines = [
        f"# Failure triage: {t['run_name']} (split: {t['split']})",
        "",
        f"{len(fails)} failed runs. Fix `with` failures first; `baseline` and `without` failures show where",
        "an agent without OpenReadout struggles (the value story), not what to fix.",
        "",
        "| cause | " + " | ".join(conds) + " |",
        "| --- |" + " --- |" * len(conds),
    ]
    lines += [f"| {c} | " + " | ".join(str(by[(c, k)]) for k in conds) + " |" for c in causes]
    for other, ids in t["with_worse_than"].items():
        if ids:
            lines += ["", f"**`with` scored below `{other}` on:** " + ", ".join(f"`{i}`" for i in ids)]
    w = [f for f in fails if f["condition"] == "with"]
    if w:
        areas = Counter(f.get("fix_area") for f in w)
        lines += [
            "",
            "## `with` failures: what to fix",
            "",
            "Fix areas: " + ", ".join(f"{k} {v}" for k, v in areas.most_common()),
            "",
            "| id | rep | cause | fix area | answer | expected | evidence | transcript |",
            "| --- | --- | --- | --- | --- | --- | --- | --- |",
        ]
        order = {"reader": 0, "agent": 1, "review": 2}
        for f in sorted(w, key=lambda f: (order.get(f.get("fix_area"), 3), f["id"], f["rep"])):
            ans = (f["answer"] or "–").replace("|", "\\|")[:50]
            lines.append(
                f"| `{f['id']}` | {f['rep']} | {f['cause']} | {f.get('fix_area')} | {ans} | "
                f"{f['expected'].replace('|', '/')} | {f.get('evidence', '')} | `{f['transcript']}` |"
            )
    for cond in ("baseline", "without"):
        rows = [f for f in fails if f["condition"] == cond]
        if not rows:
            continue
        lines += [
            "",
            f"## `{cond}` failures",
            "",
            "| id | rep | cause | answer | expected |",
            "| --- | --- | --- | --- | --- |",
        ]
        for f in rows:
            ans = (f["answer"] or "–").replace("|", "\\|")[:50]
            lines.append(f"| `{f['id']}` | {f['rep']} | {f['cause']} | {ans} | {f['expected'].replace('|', '/')} |")
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--run-name", required=True)
    ap.add_argument("--conditions", default=",".join(stats.CONDITIONS))
    ap.add_argument("--split", choices=["dev", "test", "all", "heldout"], default="dev")
    ap.add_argument("--unseal", action="store_true", help="allow triaging the sealed test split")
    ap.add_argument("--binary", help="openreadout for the offline probe (default: PATH, then target/release)")
    ap.add_argument("--no-probe", action="store_true")
    args = ap.parse_args(argv)
    if args.split != "dev" and not args.unseal:
        raise SystemExit(
            f"the {args.split} split is sealed (test and heldout: read only their aggregate scores); "
            "triage it only for a final audit, with --unseal"
        )
    binary = None if args.no_probe else run.find_binary(args.binary)
    t = triage(args.run_name, [c for c in args.conditions.split(",") if c], args.split, binary)
    prefix = RESULTS / f"{args.run_name}-triage-{args.split}"
    prefix.with_suffix(".json").write_text(json.dumps(t, indent=2, ensure_ascii=False) + "\n")
    prefix.with_suffix(".md").write_text(markdown(t))
    print(f"wrote {prefix}.md ({len(t['failures'])} failures)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
