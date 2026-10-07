#!/usr/bin/env python3
"""Count the findings, commands and exit codes in a sweep results file, with examples."""

import argparse
import json
from collections import Counter, defaultdict
from pathlib import Path


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("results", type=Path)
    a = p.parse_args()
    findings = Counter()
    commands = Counter()
    exits = Counter()
    skips = Counter()
    files = set()
    examples = defaultdict(list)
    with a.results.open() as f:
        for line in f:
            r = json.loads(line)
            if "skip" in r:
                skips[r["skip"]] += 1
                continue
            files.add((r["id"], r["filename"]))
            commands[r["command"]] += 1
            findings.update(r["findings"])
            for run in r["runs"]:
                exits[str(run["exit"])] += 1
            for category in r["findings"]:
                if len(examples[category]) < 20:
                    examples[category].append(
                        {
                            "id": r["id"],
                            "command": r["command"],
                            "exit": [run["exit"] for run in r["runs"]],
                            "rss_bytes": max(run["rss_bytes"] for run in r["runs"]),
                            "error": (r["runs"][0].get("json") or {}).get("error"),
                        }
                    )
    print(
        json.dumps(
            {
                "files": len(files),
                "command_pairs": sum(commands.values()),
                "commands": commands,
                "findings": findings,
                "exits": exits,
                "skips": skips,
                "examples": examples,
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
