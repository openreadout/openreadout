#!/usr/bin/env python3
"""Decide which CI jobs a change has to run, from the path rules in .github/ci.toml.

    python3 scripts/ci/changes.py              # JSON {job: true|false} for your branch
    python3 scripts/ci/changes.py --explain    # which changed files turn on which job
    python3 scripts/ci/changes.py --github-output   # in CI: outputs `jobs` (JSON) and `corpus-key`

On a pull request in GitHub Actions the change is the pull request's merge commit against its
base (the first parent; the checkout needs `fetch-depth: 2`). Pushes, scheduled and manual runs
run every job. Locally the change is everything since the merge base with origin/main, including
uncommitted and untracked files. `cargo xtask ci` runs this script to pick its jobs.

It also prints `corpus-key`, a hash of the smoke-tier entries of corpus/manifest.toml. CI uses it
as the Actions cache key of the downloaded smoke corpus, so the cache changes only when the smoke
tier does. Needs Python 3.11 or later (tomllib) and git.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
from functools import lru_cache
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # Python < 3.11
    try:
        import tomli as tomllib  # type: ignore[no-redef]
    except ModuleNotFoundError:
        sys.exit("scripts/ci/changes.py needs Python 3.11 or later (or the tomli package)")

ROOT = Path(__file__).resolve().parents[2]
CONFIG = ROOT / ".github" / "ci.toml"


def git(*args: str) -> str:
    return subprocess.run(["git", *args], cwd=ROOT, check=True, capture_output=True, text=True).stdout


@lru_cache(maxsize=None)
def glob_regex(pattern: str) -> re.Pattern[str]:
    """`*` within one path component, `**` across components, `?` one character."""
    out = ""
    i = 0
    while i < len(pattern):
        if pattern.startswith("**/", i):
            out += "(?:.*/)?"
            i += 3
        elif pattern.startswith("**", i):
            out += ".*"
            i += 2
        elif pattern[i] == "*":
            out += "[^/]*"
            i += 1
        elif pattern[i] == "?":
            out += "[^/]"
            i += 1
        else:
            out += re.escape(pattern[i])
            i += 1
    return re.compile(out + r"\Z")


def expand(patterns: list[str], groups: dict[str, list[str]], seen: tuple[str, ...] = ()) -> list[str]:
    out: list[str] = []
    for p in patterns:
        if p.startswith("@"):
            name = p[1:]
            if name not in groups:
                raise SystemExit(f".github/ci.toml: unknown group {p}")
            if name in seen:
                raise SystemExit(f".github/ci.toml: group {p} includes itself")
            out.extend(expand(groups[name], groups, (*seen, name)))
        else:
            out.append(p)
    return out


def matches(path: str, patterns: list[str]) -> bool:
    """The last pattern that matches decides; `!pattern` excludes."""
    result = False
    for p in patterns:
        negate = p.startswith("!")
        if glob_regex(p[1:] if negate else p).match(path):
            result = not negate
    return result


def load() -> tuple[list[str], dict[str, list[str]], dict[str, list[str]]]:
    """The `always` patterns, each job's patterns and each job's `with` list."""
    config = tomllib.loads(CONFIG.read_text())
    groups = config.get("groups", {})
    always = expand(config.get("always", []), groups)
    jobs = {name: expand(job.get("paths", []), groups) for name, job in config["jobs"].items()}
    with_ = {name: job.get("with", []) for name, job in config["jobs"].items()}
    for name, paths in jobs.items():
        if not paths:
            raise SystemExit(f".github/ci.toml: job {name} has no paths")
        for other in with_[name]:
            if other not in jobs:
                raise SystemExit(f".github/ci.toml: job {name} lists unknown job {other} in `with`")
    return always, jobs, with_


def changed_files() -> list[str] | None:
    """Changed paths, or None when every job should run."""
    if os.environ.get("GITHUB_ACTIONS") == "true":
        if os.environ.get("GITHUB_EVENT_NAME") != "pull_request":
            return None
        try:
            return [f for f in git("diff", "--name-only", "HEAD^1", "HEAD").splitlines() if f]
        except subprocess.CalledProcessError:
            return None  # no base commit in the checkout: run everything
    try:
        base = git("merge-base", "origin/main", "HEAD").strip()
    except subprocess.CalledProcessError:
        return None
    files = set(git("diff", "--name-only", base).splitlines())  # committed, staged and unstaged
    files |= set(git("ls-files", "--others", "--exclude-standard").splitlines())
    return sorted(f for f in files if f)


def corpus_key() -> str:
    manifest = tomllib.loads((ROOT / "corpus" / "manifest.toml").read_text())
    smoke = sorted((e for e in manifest["file"] if e.get("tier") == "smoke"), key=lambda e: e["id"])
    text = json.dumps(smoke, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(text.encode()).hexdigest()[:16]


def decide(files: list[str] | None) -> tuple[dict[str, bool], dict[str, list[str]]]:
    always, jobs, with_ = load()
    if files is None or any(matches(f, always) for f in files):
        return {name: True for name in jobs}, {}
    why = {name: [f for f in files if matches(f, paths)] for name, paths in jobs.items()}
    run = {name: bool(hits) for name, hits in why.items()}
    for name, others in with_.items():
        users = [o for o in others if run[o]]
        if users and not run[name]:
            run[name] = True
            why[name] = [f"(needed by {', '.join(users)})"]
    return run, why


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--github-output", action="store_true", help="write job outputs to $GITHUB_OUTPUT")
    ap.add_argument("--explain", action="store_true", help="list the files that turn on each job")
    ap.add_argument("files", nargs="*", help="decide for these paths instead of the current change")
    args = ap.parse_args()

    files = args.files or changed_files()
    run, why = decide(files)
    if args.explain:
        if files is None:
            print("every job runs (not a pull request, or the change could not be computed)")
        for name, on in run.items():
            hits = why.get(name, [])
            detail = ", ".join(hits[:5]) + (f" and {len(hits) - 5} more" if len(hits) > 5 else "")
            print(f"{'run ' if on else 'skip'}  {name:<20} {detail}")
        return
    if args.github_output:
        out = os.environ["GITHUB_OUTPUT"]
        with open(out, "a") as f:
            f.write(f"jobs={json.dumps(run, separators=(',', ':'))}\n")
            f.write(f"corpus-key={corpus_key()}\n")
        summary = os.environ.get("GITHUB_STEP_SUMMARY")
        if summary:
            with open(summary, "a") as f:
                f.write("### Jobs this change runs\n\n| job | runs | because of |\n| --- | --- | --- |\n")
                for name, on in run.items():
                    hits = why.get(name, [])
                    f.write(f"| {name} | {'yes' if on else 'no'} | {', '.join(hits[:3])} |\n")
    print(json.dumps(run, indent=2))


if __name__ == "__main__":
    main()
