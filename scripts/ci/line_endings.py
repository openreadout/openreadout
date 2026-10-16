#!/usr/bin/env python3
"""Check that files the Rust code embeds keep LF line endings on every platform.

    python3 scripts/ci/line_endings.py

Git on Windows checks text files out with CRLF unless .gitattributes says otherwise. A file
that `include_str!` or `include_bytes!` embeds, or that a test compares byte for byte with
something it generates, then differs on Windows only, and the failure shows up late in the
Windows test job. This script finds every file named in an `include_str!` or `include_bytes!`
under crates/ and xtask/ and fails when git treats it as text without `eol=lf`. Binary files
(git's `-text`) are fine. Fix a failure by adding the path to .gitattributes with `text eol=lf`.

Run it from anywhere in the repository. It needs git and Python 3.9 or later.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
INCLUDE = re.compile(r'include_(?:str|bytes)!\(\s*"([^"]+)"\s*\)')
# A path built with concat!/env! cannot be resolved here; those are listed so a person can look.
DYNAMIC = re.compile(r"include_(?:str|bytes)!\(\s*(?:concat|env)!")


def git(*args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=ROOT, check=True, capture_output=True, text=True
    ).stdout


def embedded_files() -> tuple[set[str], list[str]]:
    sources = [p for p in git("ls-files", "crates", "xtask").splitlines() if p.endswith(".rs")]
    found: set[str] = set()
    dynamic: list[str] = []
    for rel in sources:
        text = (ROOT / rel).read_text(encoding="utf-8", errors="replace")
        for m in INCLUDE.finditer(text):
            target = (ROOT / rel).parent / m.group(1)
            try:
                found.add(target.resolve().relative_to(ROOT).as_posix())
            except ValueError:
                continue  # outside the repository
        if DYNAMIC.search(text):
            dynamic.append(rel)
    return found, dynamic


def main() -> int:
    files, _dynamic = embedded_files()
    tracked = set(git("ls-files").splitlines())
    files = sorted(f for f in files if f in tracked)
    if not files:
        return 0
    eol = {}
    for line in git("ls-files", "--eol", "--", *files).splitlines():
        info, path = line.split("\t", 1)
        eol[path] = info.split()
    bad = []
    for path in files:
        index, _work, attr = eol[path][0], eol[path][1], " ".join(eol[path][2:])
        if index == "i/-text" or "-text" in attr or "eol=lf" in attr:
            continue
        bad.append(f"{path} ({index}, {attr or 'no attributes'})")
    if bad:
        print(
            "Embedded text files without `eol=lf` in .gitattributes (Windows would check them out with CRLF):"
        )
        for b in bad:
            print(f"  {b}")
        return 1
    print(f"line endings: {len(files)} embedded files keep LF on every platform")
    return 0


if __name__ == "__main__":
    sys.exit(main())
