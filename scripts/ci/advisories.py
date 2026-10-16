#!/usr/bin/env python3
"""Fail when a change adds a crate version that has a security advisory.

    python3 scripts/ci/advisories.py [--base REV]

New advisories are published every week against crates we have depended on for months. If every
pull request checked all of Cargo.lock, an advisory for an old dependency would fail whichever
pull request happened to run next. So pull requests only answer for what they add: this script
compares Cargo.lock with the base, runs `cargo deny check advisories`, and fails only for
advisories against crate versions that are new in this change. Advisories for crates already on
main are listed as notes; advisories.yml checks all of Cargo.lock daily and on every Cargo.lock
change on main, and files them as an issue. Yanked versions are warnings in deny.toml, so they
fail no pull request; advisories.yml lists them in the same issue.

The base is the first parent in GitHub Actions (CI checks out a pull request's merge commit with
`fetch-depth: 2`) and the merge base with origin/main elsewhere. Needs git and cargo-deny.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PACKAGE = re.compile(r'^\[\[package\]\]\nname = "([^"]+)"\nversion = "([^"]+)"', re.M)


def git(*args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=ROOT, check=True, capture_output=True, text=True
    ).stdout


def packages(lock: str) -> set[tuple[str, str]]:
    return set(PACKAGE.findall(lock))


def default_base() -> str:
    if os.environ.get("GITHUB_ACTIONS") == "true":
        return "HEAD^1"
    return git("merge-base", "origin/main", "HEAD").strip()


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--base", help="git revision to compare Cargo.lock with")
    args = ap.parse_args()
    try:
        base = args.base or default_base()
        before = git("show", f"{base}:Cargo.lock")
    except subprocess.CalledProcessError as e:
        print(
            f"could not read Cargo.lock at the base ({e.stderr.strip()}); checking every crate instead"
        )
        before = ""
    now = (ROOT / "Cargo.lock").read_text()
    if before == now:
        print("Cargo.lock is unchanged; nothing to check (advisories.yml checks all of it daily)")
        return 0
    added = packages(now) - packages(before)
    if not added:
        print("this change adds no crate versions")
        return 0
    listed = ", ".join(f"{n} {v}" for n, v in sorted(added)[:20])
    print(
        f"this change adds {len(added)} crate versions: {listed}{', ...' if len(added) > 20 else ''}"
    )
    r = subprocess.run(
        ["cargo", "deny", "--format", "json", "check", "advisories"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    new, old = [], []
    for line in r.stderr.splitlines():
        try:
            d = json.loads(line)
        except json.JSONDecodeError:
            continue
        f = d.get("fields", {})
        if d.get("type") != "diagnostic" or f.get("severity") != "error":
            continue
        adv = f.get("advisory") or {}
        krates = {
            (g["Krate"]["name"], g["Krate"]["version"]) for g in f.get("graphs", []) if "Krate" in g
        }
        text = f"{adv.get('id', f.get('code', '?'))} {f.get('message', '')} ({', '.join(f'{n} {v}' for n, v in sorted(krates))})"
        (new if krates & added else old).append(text)
    if not new and not old and r.returncode != 0:
        print(r.stderr[-3000:])
        print(
            "::error::cargo deny check advisories failed without a diagnostic this script understands"
        )
        return 1
    for text in old:
        print(f"::notice::advisory for a crate already on main (advisories.yml reports it): {text}")
    for text in new:
        print(f"::error::this change adds a crate with a security advisory: {text}")
    if new:
        print(
            "Pick a version without the advisory, or add the advisory to [advisories] ignore in deny.toml with a reason."
        )
        return 1
    print("no advisories for the crates this change adds")
    return 0


if __name__ == "__main__":
    sys.exit(main())
