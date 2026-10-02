"""Benchmark statistics across conditions and repeats: the numbers worth quoting.

The unit of analysis is the question, not the run: each question gets a pass rate over its
repeats, a condition's score is the mean of those rates, and confidence intervals come from a
bootstrap that resamples questions (runs of one question are not independent evidence).
Conditions are compared on the questions both answered, pairwise: the mean difference with a
bootstrap interval, and an exact two-sided sign test over questions whose pass rates differ.

    uv run python stats.py --run-name 2026-09-24-sonnet-5            # writes results/<run>-benchmark.{md,json}
    uv run python stats.py --run-name ... --split dev                # one split only
    uv run python stats.py --run-name ... --split heldout            # the held-out set, vs the test split

Reported per condition and split: accuracy (mean pass rate, 95 % CI), pass^k (share of
questions answered correctly on every repeat), estimated cost per run and per correct answer,
median turns and wall time; per family and per tier (lookup, analysis, conversion, integrity,
search, quantitation, batch, scenario, visual), and the share of runs that looked at the data as a
picture with the accuracy of runs that did and did not.
"""

from __future__ import annotations

import argparse
import json
import math
import random
import statistics
import sys
from collections import defaultdict
from pathlib import Path

import score

EVALS = Path(__file__).resolve().parent
RESULTS = EVALS / "results"
CONDITIONS = ("with", "baseline", "without")
# `all` is dev + test. `heldout` (evals/heldout.py: files from sources no reader was developed on)
# is reported on its own, next to the test split, as the generalization check.
SPLITS = ("all", "dev", "test", "heldout")
BOOTSTRAP = 10_000
SEED = 20260923
# Turn budgets for accuracy-at-budget: the prompt never states the cap, so a run that finished
# within k turns under a high cap would have behaved the same under a cap of k.
TURN_BUDGETS = (10, 20, 40)
CAPPED = ("error_max_turns", "error_max_budget_usd", "timeout")


def tier_of(q: dict) -> str:
    if q["category"] in (
        "analysis",
        "batch",
        "conversion",
        "integrity",
        "search",
        "quantitation",
        "scenario",
        "visual",
    ):
        return q["category"]
    return "lookup"


def load(run_name: str, conditions: list[str]) -> dict[str, list[dict]]:
    """Records per condition; a repeated (id, rep) keeps its last record."""
    out = {}
    for cond in conditions:
        p = RESULTS / f"{run_name}-{cond}.records.jsonl"
        if not p.exists():
            continue
        latest: dict[tuple[str, int], dict] = {}
        for rec in score.load_records(p):
            latest[(rec["id"], rec.get("rep", 0))] = rec
        out[cond] = list(latest.values())
    return out


def per_question(records: list[dict], questions: dict[str, dict]) -> dict[str, dict]:
    """Pass rate and resource use per question, from its repeats (rescored with the current scorer)."""
    runs: dict[str, list[dict]] = defaultdict(list)
    for rec in records:
        q = questions.get(rec["id"])
        if q is None:
            continue
        runs[rec["id"]].append(
            {**rec, "correct": bool(score.score_record(q, rec)["correct"]), "looked": score.looked(rec)}
        )
    out = {}
    for qid, rs in runs.items():
        k = len(rs)
        c = sum(r["correct"] for r in rs)
        out[qid] = {
            "k": k,
            "correct": c,
            "rate": c / k,
            "all": c == k,
            "cost": [r["cost_usd"] for r in rs if r.get("cost_usd") is not None],
            "turns": [r["num_turns"] for r in rs if r.get("num_turns") is not None],
            "wall": [r["wall_s"] for r in rs if r.get("wall_s") is not None],
            "capped": sum(r.get("subtype") in CAPPED for r in rs),
            "at_turns": {b: sum(r["correct"] and (r.get("num_turns") or 0) <= b for r in rs) / k for b in TURN_BUDGETS},
            "looked": sum(r["looked"] for r in rs),
            "looked_correct": sum(r["looked"] and r["correct"] for r in rs),
        }
    return out


def bootstrap_ci(values: list[float], rng: random.Random, n: int = BOOTSTRAP) -> tuple[float, float]:
    if not values:
        return (math.nan, math.nan)
    if len(values) == 1:
        return (values[0], values[0])
    means = sorted(statistics.fmean(rng.choices(values, k=len(values))) for _ in range(n))
    return (means[int(0.025 * n)], means[int(0.975 * n) - 1])


def sign_test(wins: int, losses: int) -> float:
    """Exact two-sided binomial sign test (ties dropped)."""
    n = wins + losses
    if n == 0:
        return 1.0
    k = min(wins, losses)
    tail = sum(math.comb(n, i) for i in range(k + 1)) / 2**n
    return min(1.0, 2 * tail)


def condition_summary(pq: dict[str, dict], rng: random.Random) -> dict:
    rates = [v["rate"] for v in pq.values()]
    costs = [c for v in pq.values() for c in v["cost"]]
    turns = [t for v in pq.values() for t in v["turns"]]
    wall = [w for v in pq.values() for w in v["wall"]]
    correct_runs = sum(v["correct"] for v in pq.values())
    n_runs = sum(v["k"] for v in pq.values())
    looked = sum(v.get("looked", 0) for v in pq.values())
    looked_ok = sum(v.get("looked_correct", 0) for v in pq.values())
    lo, hi = bootstrap_ci(rates, rng)
    return {
        "questions": len(pq),
        "runs": sum(v["k"] for v in pq.values()),
        "accuracy": statistics.fmean(rates) if rates else None,
        "accuracy_ci95": [lo, hi],
        "pass_all_k": statistics.fmean([v["all"] for v in pq.values()]) if pq else None,
        "cost_per_run_usd": statistics.fmean(costs) if costs else None,
        "cost_per_correct_usd": (sum(costs) / correct_runs) if costs and correct_runs else None,
        "median_turns": statistics.median(turns) if turns else None,
        "median_wall_s": statistics.median(wall) if wall else None,
        "capped_rate": (sum(v["capped"] for v in pq.values()) / sum(v["k"] for v in pq.values())) if pq else None,
        "accuracy_at_turns": {
            str(b): statistics.fmean([v["at_turns"][b] for v in pq.values()]) if pq else None for b in TURN_BUDGETS
        },
        # runs that looked at the data as a picture (score.looked), and per-run accuracy either way
        "looked_rate": looked / n_runs if n_runs else None,
        "accuracy_when_looked": looked_ok / looked if looked else None,
        "accuracy_when_not_looked": (correct_runs - looked_ok) / (n_runs - looked) if n_runs - looked else None,
    }


def paired(a: dict[str, dict], b: dict[str, dict], rng: random.Random) -> dict:
    common = sorted(set(a) & set(b))
    diffs = [a[q]["rate"] - b[q]["rate"] for q in common]
    wins = sum(d > 0 for d in diffs)
    losses = sum(d < 0 for d in diffs)
    lo, hi = bootstrap_ci(diffs, rng)
    return {
        "questions": len(common),
        "diff": statistics.fmean(diffs) if diffs else None,
        "diff_ci95": [lo, hi],
        "wins": wins,
        "losses": losses,
        "ties": len(diffs) - wins - losses,
        "sign_test_p": sign_test(wins, losses),
        "losses_ids": [q for q, d in zip(common, diffs, strict=True) if d < 0],
    }


def analyse(run_name: str, conditions: list[str], split: str = "all") -> dict:
    questions = score.load_questions()
    exposed = 0
    if split == "heldout":
        # exposed held-out files (docs/benchmark/heldout.md) do not count toward generalization
        exposed = sum(1 for v in questions.values() if v.get("split") == split and v.get("exposed"))
        questions = {k: v for k, v in questions.items() if v.get("split") == split and not v.get("exposed")}
    elif split != "all":
        questions = {k: v for k, v in questions.items() if v.get("split") == split}
    else:
        questions = {k: v for k, v in questions.items() if v.get("split") != "heldout"}
    recs = load(run_name, conditions)
    conds = [c for c in conditions if c in recs]
    pq = {c: per_question(recs[c], questions) for c in conds}
    rng = random.Random(SEED)
    report: dict = {"run_name": run_name, "split": split, "conditions": conds, "overall": {}, "paired": {}}
    if exposed:
        report["exposed_questions_excluded"] = exposed
    meta = next((r.get("meta") for c in conds for r in recs[c] if r.get("meta")), {}) or {}
    report["meta"] = {k: meta.get(k) for k in ("model", "date", "claude_version", "openreadout_version", "max_turns")}
    report["auth"] = sorted({str(r.get("api_key_source")) for c in conds for r in recs[c]})
    for c in conds:
        report["overall"][c] = condition_summary(pq[c], rng)
    for i, x in enumerate(conds):
        for y in conds[i + 1 :]:
            report["paired"][f"{x} vs {y}"] = paired(pq[x], pq[y], rng)
    for dim, key in (("by_tier", tier_of), ("by_family", lambda q: q["family"])):
        groups = sorted({key(questions[q]) for c in conds for q in pq[c]})
        report[dim] = {}
        for g in groups:
            report[dim][g] = {
                c: condition_summary({q: v for q, v in pq[c].items() if key(questions[q]) == g}, rng) for c in conds
            }
    return report


# ---------------------------------------------------------------- markdown


def _pct(x: float | None) -> str:
    return "–" if x is None or (isinstance(x, float) and math.isnan(x)) else f"{x * 100:.0f} %"


def _ci(ci: list[float]) -> str:
    return "–" if any(math.isnan(v) for v in ci) else f"{ci[0] * 100:.0f}–{ci[1] * 100:.0f}"


def _usd(x: float | None) -> str:
    return "–" if x is None else f"${x:.3f}"


def markdown(r: dict) -> str:
    conds = r["conditions"]
    m = r["meta"]
    lines = [
        f"# Benchmark: {r['run_name']} (split: {r['split']})",
        "",
        f"Model `{m.get('model')}`, Claude Code {m.get('claude_version')}, OpenReadout "
        f"{m.get('openreadout_version')}, max {m.get('max_turns')} turns. Auth source(s): {', '.join(r['auth'])}. "
        "Costs are Claude Code's estimates at API prices "
        "(a usage measure; subscription runs are not billed per token).",
        "",
    ]
    if r.get("exposed_questions_excluded"):
        lines += [
            f"{r['exposed_questions_excluded']} questions about exposed held-out files (developed on before "
            "their draw, docs/benchmark/heldout.md) are scored but not counted here.",
            "",
        ]
    lines += [
        "| | " + " | ".join(f"`{c}`" for c in conds) + " |",
        "| --- |" + " --- |" * len(conds),
    ]
    rows = [
        ("questions × runs", lambda s: f"{s['questions']} × {s['runs'] / max(s['questions'], 1):.3g}"),
        ("accuracy", lambda s: f"**{_pct(s['accuracy'])}**"),
        ("95 % CI", lambda s: _ci(s["accuracy_ci95"])),
        ("right on every repeat (pass^k)", lambda s: _pct(s["pass_all_k"])),
        ("est. cost per run", lambda s: _usd(s["cost_per_run_usd"])),
        ("est. cost per correct answer", lambda s: _usd(s["cost_per_correct_usd"])),
        ("median turns", lambda s: str(s["median_turns"])),
        ("median wall time (s)", lambda s: str(s["median_wall_s"])),
        ("runs that hit a cap (turns/time/budget)", lambda s: _pct(s["capped_rate"])),
        *[(f"accuracy within {b} turns", lambda s, b=b: _pct(s["accuracy_at_turns"][str(b)])) for b in TURN_BUDGETS],
        ("runs that looked at the data as a picture", lambda s: _pct(s.get("looked_rate"))),
        (
            "accuracy of runs that looked / did not",
            lambda s: f"{_pct(s.get('accuracy_when_looked'))} / {_pct(s.get('accuracy_when_not_looked'))}",
        ),
    ]
    for name, fn in rows:
        lines.append(f"| {name} | " + " | ".join(fn(r["overall"][c]) for c in conds) + " |")
    lines += [
        "",
        "## Paired comparisons (per question)",
        "",
        "| | questions | difference | 95 % CI | better / worse / tied | sign test p |",
        "| --- | --- | --- | --- | --- | --- |",
    ]
    for name, p in r["paired"].items():
        diff = "–" if p["diff"] is None else f"{p['diff'] * 100:+.1f} pts"
        ci = (
            "–"
            if any(math.isnan(v) for v in p["diff_ci95"])
            else (f"{p['diff_ci95'][0] * 100:+.1f} to {p['diff_ci95'][1] * 100:+.1f}")
        )
        lines.append(
            f"| {name} | {p['questions']} | {diff} | {ci} | {p['wins']} / {p['losses']} / {p['ties']} "
            f"| {p['sign_test_p']:.3g} |"
        )
    if r["split"] != "test":
        for name, p in r["paired"].items():
            if name.startswith("with ") and p["losses_ids"]:
                lines += [
                    "",
                    f"Questions where `{name.split(' vs ')[0]}` did worse than `{name.split(' vs ')[1]}`: "
                    + ", ".join(f"`{q}`" for q in p["losses_ids"]),
                ]
    for dim, title in (("by_tier", "By tier"), ("by_family", "By family")):
        lines += [
            "",
            f"## {title} (accuracy, 95 % CI)",
            "",
            "| | " + " | ".join(conds) + " |",
            "| --- |" + " --- |" * len(conds),
        ]
        for g, per in r[dim].items():
            cells = []
            for c in conds:
                s = per.get(c)
                cells.append(
                    "–"
                    if not s or not s["questions"]
                    else f"{_pct(s['accuracy'])} ({_ci(s['accuracy_ci95'])}, n={s['questions']})"
                )
            lines.append(f"| {g} | " + " | ".join(cells) + " |")
    return "\n".join(lines) + "\n"


def generalization(run_name: str, conditions: list[str], heldout: dict) -> dict:
    """Held-out accuracy next to the test split's, per condition: the gap is how much of the
    development-corpus score does not carry over to files no reader was developed on."""
    test = analyse(run_name, conditions, "test")
    out = {}
    for c in heldout["conditions"]:
        h, t = heldout["overall"][c], test["overall"].get(c)
        if not t or not t["questions"] or h["accuracy"] is None or t["accuracy"] is None:
            continue
        out[c] = {"heldout": h["accuracy"], "test": t["accuracy"], "gap": h["accuracy"] - t["accuracy"]}
    return out


def generalization_markdown(gen: dict) -> str:
    if not gen:
        return "\nNo test-split records in this run: run `--split test` too to measure the generalization gap.\n"
    lines = [
        "",
        "## Generalization (held-out vs test split)",
        "",
        "Questions about exposed held-out files (developed on before their draw) are not counted.",
        "",
        "| condition | held-out | test | gap |",
        "| --- | --- | --- | --- |",
    ]
    for c, v in gen.items():
        lines.append(f"| {c} | {_pct(v['heldout'])} | {_pct(v['test'])} | {v['gap'] * 100:+.1f} pts |")
    return "\n".join(lines) + "\n"


def write(run_name: str, conditions: list[str], split: str = "all") -> list[Path]:
    r = analyse(run_name, conditions, split)
    if not r["conditions"]:
        return []
    if split == "heldout":
        # only when the run holds held-out records (a dev/test run gets no empty held-out report)
        if not any(s["questions"] for s in r["overall"].values()):
            return []
        r["generalization"] = generalization(run_name, conditions, r)
    suffix = "" if split == "all" else f"-{split}"
    prefix = RESULTS / f"{run_name}-benchmark{suffix}"
    prefix.with_suffix(".json").write_text(json.dumps(r, indent=2, ensure_ascii=False) + "\n")
    md = markdown(r) + (generalization_markdown(r["generalization"]) if split == "heldout" else "")
    prefix.with_suffix(".md").write_text(md)
    return [prefix.with_suffix(".json"), prefix.with_suffix(".md")]


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--run-name", required=True)
    ap.add_argument("--conditions", default=",".join(CONDITIONS))
    ap.add_argument(
        "--split", choices=[*SPLITS], default=None, help="default: every report (heldout only when the run has it)"
    )
    args = ap.parse_args(argv)
    conds = [c for c in args.conditions.split(",") if c]
    splits = [args.split] if args.split else list(SPLITS)
    written = [p for s in splits for p in write(args.run_name, conds, s)]
    for p in written:
        print(f"wrote {p}")
    return 0 if written else 1


if __name__ == "__main__":
    sys.exit(main())
