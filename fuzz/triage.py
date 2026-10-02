#!/usr/bin/env python3
"""Replay every crash artifact under fuzz/artifacts/<target>/ and group them by cause.

    python3 fuzz/triage.py [target ...]

Prints, per target, each distinct first panic message / sanitizer summary with the
top frames that belong to this repository, and one example artifact.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
HOST = subprocess.run(["rustc", "+nightly", "-vV"], capture_output=True, text=True).stdout
TRIPLE = re.search(r"host: (\S+)", HOST).group(1)
BIN = ROOT / "target" / TRIPLE / "release"


def cause(out: str) -> tuple[str, list[str]]:
    m = re.search(r"panicked at ([^\n]+)\n([^\n]*)", out)
    if m:
        key = f"panic at {m.group(1)}: {m.group(2)}"
    else:
        m = re.search(r"(ERROR: AddressSanitizer: [\w-]+|ERROR: libFuzzer: [\w -]+|SUMMARY: [^\n]+)", out)
        key = m.group(1) if m else "unknown (no panic or sanitizer report)"
    frames = [
        f.strip()
        for f in re.findall(r"#\d+ 0x[0-9a-f]+ in ([^\n]+)", out)
        if "openreadout" in f or "jpegxr" in f
    ][:4]
    return key, frames


def main() -> None:
    targets = sys.argv[1:] or sorted(p.name for p in (ROOT / "artifacts").iterdir() if p.is_dir())
    for t in targets:
        arts = sorted((ROOT / "artifacts" / t).glob("*"))
        if not arts:
            continue
        groups: dict[str, tuple[list[str], list[Path]]] = {}
        for a in arts:
            r = subprocess.run(
                [str(BIN / t), str(a), "-rss_limit_mb=2048", "-malloc_limit_mb=2048", "-timeout=30"],
                capture_output=True,
                text=True,
                errors="replace",
                timeout=300,
            )
            out = r.stdout + r.stderr
            if r.returncode == 0:
                key, frames = "no longer reproduces", []
            else:
                key, frames = cause(out)
            groups.setdefault(key, (frames, []))[1].append(a)
        print(f"== {t}: {len(arts)} artifacts, {len(groups)} causes")
        for key, (frames, files) in groups.items():
            print(f"  [{len(files)}] {key}")
            for f in frames:
                print(f"        {f[:160]}")
            print(f"        e.g. {files[0].relative_to(ROOT)}")


if __name__ == "__main__":
    main()
