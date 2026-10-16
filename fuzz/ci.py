#!/usr/bin/env python3
"""Plan, run and report CI's fuzz jobs (.github/workflows/fuzz.yml).

    python3 fuzz/ci.py plan --base HEAD^1 --seconds 15     # which targets, in how many shards
    python3 fuzz/ci.py plan --all --seconds 60
    python3 fuzz/ci.py run --shard 0 --bin-dir DIR         # run one shard's targets
    python3 fuzz/ci.py issues --reports DIR                # open or update one issue per crash

`plan` asks affected.py which targets the change reaches and whether it reaches each one
directly (its code changed) or indirectly (only shared inputs such as fuzz/Cargo.lock changed).
It splits them into shards of about five minutes of fuzzing each and writes the plan to
$GITHUB_OUTPUT as `plan` (JSON), `count` and `shards` (a JSON list for the job matrix).

`run` reads the plan from $FUZZ_PLAN, runs each target of its shard from the committed seeds for
the planned time, keeps crash inputs in fuzz/artifacts/<target>/ and writes
fuzz/reports/shard-<n>.json and a table in the job summary. It fails when a direct target
crashes. A crash in an indirect target is most likely an old bug the change only made reachable,
so it is reported (job summary, issue) without failing the pull request.

`issues` opens an issue per crashing target, or comments on the open one with the same title,
using the GitHub CLI and $GH_TOKEN. Without permission to write issues (pull requests from forks
and Dependabot get a read-only token) it prints a warning and the nightly run reports instead.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import re
import subprocess
import sys
from pathlib import Path

FUZZ = Path(__file__).resolve().parent
ROOT = FUZZ.parent
SHARD_SECONDS = 300  # fuzzing time per shard to aim for
LABEL = "fuzz-crash"


def output(name: str, value: str) -> None:
    path = os.environ.get("GITHUB_OUTPUT")
    if path:
        with open(path, "a") as f:
            f.write(f"{name}={value}\n")


def summary(text: str) -> None:
    path = os.environ.get("GITHUB_STEP_SUMMARY")
    if path:
        with open(path, "a") as f:
            f.write(text)
    else:
        print(text, file=sys.stderr)


def plan(args: argparse.Namespace) -> None:
    which = ["--all"] if args.all else [args.base]
    found: dict[str, str] = json.loads(
        subprocess.run(
            [sys.executable, str(FUZZ / "affected.py"), "--json", *which],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    )
    n = len(found)
    shards = min(args.max_shards, max(1, math.ceil(n * args.seconds / SHARD_SECONDS))) if n else 0
    names = sorted(found)
    layout = [names[i::shards] for i in range(shards)]
    doc = {"seconds": args.seconds, "targets": found, "shards": layout}
    output("plan", json.dumps(doc, separators=(",", ":")))
    output("count", str(n))
    output("shards", json.dumps(list(range(shards))))
    direct = sum(1 for c in found.values() if c == "direct")
    summary(
        f"### Fuzz plan: {n} targets ({direct} direct, {n - direct} indirect), "
        f"{args.seconds} s each, {shards} shards\n\n"
        + (" ".join(f"`{t}`" if found[t] == "direct" else t for t in names) or "nothing to fuzz")
        + "\n\nTargets in `code` font are direct: a crash there fails the pull request. "
        "Crashes in the others are reported in an issue.\n"
    )
    print(json.dumps(doc, indent=1))


def cause(log: str) -> str:
    """The first panic message or sanitizer summary in a libFuzzer log."""
    m = re.search(r"panicked at ([^\n]+)\n([^\n]*)", log)
    if m:
        return f"panic at {m.group(1).strip()}: {m.group(2).strip()}"
    m = re.search(
        r"(ERROR: AddressSanitizer: [^\n]+|ERROR: libFuzzer: [^\n]+|SUMMARY: [^\n]+)", log
    )
    return m.group(1).strip() if m else "unknown (no panic message or sanitizer report in the log)"


def run(args: argparse.Namespace) -> int:
    doc = json.loads(os.environ["FUZZ_PLAN"])
    targets: list[str] = doc["shards"][args.shard]
    secs = str(doc["seconds"])
    bin_dir = Path(args.bin_dir).resolve()
    reports = FUZZ / "reports"
    reports.mkdir(exist_ok=True)
    rows = []
    for t in targets:
        for d in ("work", "artifacts", "corpus", "logs"):
            (FUZZ / d / (t if d != "logs" else "")).mkdir(parents=True, exist_ok=True)
        print(f"::group::{t} ({doc['targets'][t]})", flush=True)
        r = subprocess.run(
            [
                str(bin_dir / t),
                f"work/{t}",
                f"corpus/{t}",
                f"-max_total_time={secs}",
                "-rss_limit_mb=2048",
                "-malloc_limit_mb=2048",
                "-timeout=20",
                f"-artifact_prefix=artifacts/{t}/",
            ],
            cwd=FUZZ,
            capture_output=True,
            text=True,
            errors="replace",
        )
        log = r.stdout + r.stderr
        (FUZZ / "logs" / f"{t}.log").write_text(log)
        print(log[-4000:], flush=True)
        print("::endgroup::", flush=True)
        if r.returncode == 0:
            continue
        files = sorted(p.name for p in (FUZZ / "artifacts" / t).iterdir())
        row = {"target": t, "class": doc["targets"][t], "cause": cause(log), "artifacts": files}
        rows.append(row)
        level = "error" if row["class"] == "direct" else "warning"
        print(f"::{level}::fuzz target {t} crashed ({row['class']}): {row['cause']}")
    (reports / f"shard-{args.shard}.json").write_text(json.dumps(rows, indent=1))
    if rows:
        lines = [
            "| target | how the change reaches it | cause | crash input |",
            "| --- | --- | --- | --- |",
        ]
        for row in rows:
            first = row["artifacts"][0] if row["artifacts"] else ""
            lines.append(
                f"| {row['target']} | {row['class']} | {row['cause'].replace('|', '/')} | {first} |"
            )
        summary(
            f"### Fuzz shard {args.shard}: {len(rows)} of {len(targets)} targets crashed\n\n"
            + "\n".join(lines)
            + "\n\nCrash inputs are in this shard's `fuzz-artifacts` upload. Indirect crashes do not fail "
            "the pull request; the `fuzz report` job files them as issues.\n"
        )
    else:
        summary(f"### Fuzz shard {args.shard}: no crashes in {len(targets)} targets\n")
    return 1 if any(row["class"] == "direct" for row in rows) else 0


def gh(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(["gh", *args], capture_output=True, text=True)


def issues(args: argparse.Namespace) -> int:
    rows = []
    for p in sorted(Path(args.reports).rglob("shard-*.json")):
        rows.extend(json.loads(p.read_text()))
    if not args.include_direct:
        rows = [r for r in rows if r["class"] == "indirect"]
    if not rows:
        print("no crashes to report")
        return 0
    server = os.environ.get("GITHUB_SERVER_URL", "https://github.com")
    repo = os.environ.get("GITHUB_REPOSITORY", "")
    run_url = f"{server}/{repo}/actions/runs/{os.environ.get('GITHUB_RUN_ID', '')}"
    gh(
        "label",
        "create",
        LABEL,
        "--color",
        "B60205",
        "--description",
        "A fuzz target crashed in CI",
        "--force",
    )
    for row in rows:
        t = row["target"]
        title = f"Fuzz target {t} crashes"
        body = (
            f"CI found a crash in fuzz target `{t}` ({args.context}, {run_url}).\n\n"
            f"Cause: `{row['cause']}`\n\n"
            f"Crash inputs: {', '.join(f'`{a}`' for a in row['artifacts']) or 'none kept'} in the run's "
            "`fuzz-artifacts-*` uploads (kept for 30 days).\n\n"
            "Reproduce with `cd fuzz && cargo +nightly fuzz run -O -a "
            f"{t} <crash input>`, then follow fuzz/README.md (fix, add the input as a malformed fixture, "
            "log it in the provenance file)."
        )
        found = gh(
            "issue",
            "list",
            "--state",
            "open",
            "--label",
            LABEL,
            "--search",
            f'"{title}" in:title',
            "--json",
            "number,title",
        )
        existing = (
            [i for i in json.loads(found.stdout or "[]") if i["title"] == title]
            if found.returncode == 0
            else []
        )
        if existing:
            n = str(existing[0]["number"])
            seen = gh(
                "issue",
                "view",
                n,
                "--json",
                "body,comments",
                "--jq",
                '[.body] + [.comments[].body] | join("\\n")',
            )
            if seen.returncode == 0 and f"Cause: `{row['cause']}`" in seen.stdout:
                print(f"{t}: #{n} already reports this cause")
                continue
            r = gh("issue", "comment", n, "--body", body)
            what = f"commented on #{n}"
        else:
            r = gh("issue", "create", "--title", title, "--label", LABEL, "--body", body)
            what = "opened an issue"
        if r.returncode == 0:
            print(f"{t}: {what}")
        else:
            print(
                f"::warning::could not file the crash in {t} as an issue ({r.stderr.strip()}); "
                "the nightly fuzz run on main reports it"
            )
    return 0


def main() -> None:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("plan")
    g = p.add_mutually_exclusive_group(required=True)
    g.add_argument("--base", help="git revision the change starts from")
    g.add_argument("--all", action="store_true", help="every target")
    p.add_argument("--seconds", type=int, required=True, help="fuzzing time per target")
    p.add_argument("--max-shards", type=int, default=8)
    r = sub.add_parser("run")
    r.add_argument("--shard", type=int, required=True)
    r.add_argument("--bin-dir", required=True, help="directory holding the built fuzz targets")
    i = sub.add_parser("issues")
    i.add_argument("--reports", required=True, help="directory with the shards' report files")
    i.add_argument(
        "--context", default="a CI run", help="where the crash was found, for the issue text"
    )
    i.add_argument(
        "--include-direct", action="store_true", help="also report direct crashes (nightly)"
    )
    args = ap.parse_args()
    if args.cmd == "plan":
        plan(args)
    elif args.cmd == "run":
        sys.exit(run(args))
    else:
        sys.exit(issues(args))


if __name__ == "__main__":
    main()
