#!/usr/bin/env python3
"""Print the fuzz targets a change could affect, one per line.

Run it from anywhere in the repository. It needs git and cargo.

    python3 fuzz/affected.py BASE_REV          # targets affected by BASE_REV..HEAD
    python3 fuzz/affected.py --all             # all targets
    python3 fuzz/affected.py --json BASE_REV   # {target: "direct" | "indirect"}

A target counts as affected if the diff changes its source file, its seeds in
fuzz/corpus/<target>/, or a crate it depends on. The script finds those crates from the
`openreadout_*` names in the target's source, plus their dependencies from `cargo metadata`.
Changes to the harness (fuzz/src/, fuzz/Cargo.toml) affect all targets.

`--json` also says how. A target is "direct" when the change touches its own code (the cases
above). It is "indirect" when it is only picked because the change touches something every
target builds with: fuzz/Cargo.lock, the root Cargo.toml or rust-toolchain.toml. A
crash in an indirect target is most likely an old bug that new dependency versions or a new
toolchain let the fuzzer reach, so CI reports it in an issue instead of failing the pull request
(fuzz/ci.py).
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FUZZ = ROOT / "fuzz"
HARNESS = ("fuzz/src/", "fuzz/Cargo.toml")
SHARED = ("fuzz/Cargo.lock", "Cargo.toml", "rust-toolchain.toml")
CRATE_PATH = re.compile(r"\bopenreadout_[a-z0-9_]+")


def run(*args: str) -> str:
    return subprocess.run(args, cwd=ROOT, check=True, capture_output=True, text=True).stdout


def targets() -> dict[str, Path]:
    """Fuzz target name -> source file, from the [[bin]] tables of fuzz/Cargo.toml."""
    text = (FUZZ / "Cargo.toml").read_text()
    pairs = re.findall(r'^\[\[bin\]\]\s*\nname = "([^"]+)"\s*\npath = "([^"]+)"', text, re.M)
    if not pairs:
        sys.exit("fuzz/affected.py: no [[bin]] targets found in fuzz/Cargo.toml")
    return {name: FUZZ / path for name, path in pairs}


def crates() -> dict[str, tuple[str, list[str]]]:
    """Workspace crate name -> (directory relative to the root, internal dependencies)."""
    meta = json.loads(run("cargo", "metadata", "--no-deps", "--format-version", "1"))
    out = {}
    for p in meta["packages"]:
        rel = Path(p["manifest_path"]).parent.relative_to(ROOT).as_posix() + "/"
        deps = p["dependencies"]
        out[p["name"]] = (rel, [d["name"] for d in deps if d.get("path") and d["kind"] != "dev"])
    return out


def closure(roots: set[str], graph: dict[str, tuple[str, list[str]]]) -> set[str]:
    seen: set[str] = set()
    todo = [r for r in roots if r in graph]
    while todo:
        name = todo.pop()
        if name not in seen:
            seen.add(name)
            todo.extend(d for d in graph[name][1] if d in graph)
    return seen


def classify(base: str) -> dict[str, str]:
    """Affected targets, each "direct" or "indirect" (see the module docstring)."""
    all_targets = targets()
    changed = [f for f in run("git", "diff", "--name-only", base, "HEAD").splitlines() if f]

    def touches(prefixes: tuple[str, ...]) -> bool:
        return any(
            f == e or (e.endswith("/") and f.startswith(e)) for f in changed for e in prefixes
        )

    if touches(HARNESS):
        return dict.fromkeys(all_targets, "direct")
    found = dict.fromkeys(all_targets, "indirect") if touches(SHARED) else {}
    graph = crates()
    for name, src in all_targets.items():
        rel = src.relative_to(ROOT).as_posix()
        roots = {m.replace("_", "-") for m in CRATE_PATH.findall(src.read_text())}
        dirs = tuple(graph[c][0] for c in closure(roots, graph))
        seeds = f"fuzz/corpus/{name}/"
        if any(f == rel or f.startswith(seeds) or f.startswith(dirs) for f in changed):
            found[name] = "direct"
    return {name: found[name] for name in all_targets if name in found}


def main() -> None:
    args = sys.argv[1:]
    as_json = "--json" in args
    args = [a for a in args if a != "--json"]
    if len(args) != 1:
        sys.exit(__doc__)
    found = dict.fromkeys(targets(), "direct") if args[0] == "--all" else classify(args[0])
    print(json.dumps(found, indent=1) if as_json else "\n".join(found))


if __name__ == "__main__":
    main()
