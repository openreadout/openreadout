#!/usr/bin/env python
"""Run gen.py for every manifest entry of the given formats that is present in the corpus dir.

Usage: uv run python gen_manifest.py bruker-nmr jcamp-dx
Env:   OPENREADOUT_CORPUS_DIR (default ../corpus/files)

Entries are addressed by id (`gen.py --id <id> <path>`), which is what bundle members such as
Bruker experiment directories need.
"""
import os, subprocess, sys, tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

def main():
    formats = set(sys.argv[1:])
    corpus = Path(os.environ.get("OPENREADOUT_CORPUS_DIR", ROOT / "corpus" / "files"))
    manifest = tomllib.loads((ROOT / "corpus" / "manifest.toml").read_text())
    args = []
    for e in manifest["file"]:
        if e["format"] not in formats or e.get("role", "input") not in ("input", ""):
            continue
        p = corpus / e["filename"]
        if not p.exists():
            print("missing", e["id"])
            continue
        args += ["--id", e["id"], str(p)]
    if args:
        subprocess.run([sys.executable, str(Path(__file__).with_name("gen.py")), *args], check=True)

if __name__ == "__main__":
    main()
