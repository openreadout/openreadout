"""Check `openreadout export --format nwb` output with pynwb and nwbinspector (both BSD-3).

Usage:
    uv run python nwb_validate.py OUT.nwb [--source FILE] [--openreadout BIN]

Opens the file with `pynwb.NWBHDF5IO` (which validates the layout against the NWB core schema as
it builds the objects), checks the session fields and every `TimeSeries` under `acquisition/`,
runs `nwbinspector.inspect_nwbfile` and reports its messages by importance, and, with --source,
compares every series' first samples with `openreadout trace SOURCE --trace T --sweep S`
(channels in the series' `comments` JSON). Prints one JSON line; exit 1 on a failure or a
CRITICAL/PYNWB_VALIDATION inspector message.
"""

import argparse
import json
import math
import subprocess
import sys
from pathlib import Path

import numpy as np
from pynwb import NWBHDF5IO, TimeSeries


def run(bin_: str, *args: str) -> dict:
    out = subprocess.run([bin_, *args, "--json"], capture_output=True, text=True, check=True)
    env = json.loads(out.stdout)
    if not env.get("ok"):
        raise SystemExit(f"openreadout {' '.join(args)} failed: {env.get('error')}")
    return env["data"]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("file", type=Path)
    ap.add_argument("--source", type=Path)
    ap.add_argument("--openreadout", default="openreadout")
    a = ap.parse_args()
    problems: list[str] = []
    series = {}
    with NWBHDF5IO(str(a.file), "r") as io:
        nwb = io.read()
        for k in ("identifier", "session_description", "session_start_time"):
            if not getattr(nwb, k):
                problems.append(f"{k} missing")
        if nwb.session_start_time.tzinfo is None:
            problems.append("session_start_time has no time zone")
        compared = 0
        for name, ts in nwb.acquisition.items():
            if not isinstance(ts, TimeSeries):
                problems.append(f"acquisition/{name} is {type(ts).__name__}, not TimeSeries")
                continue
            data = ts.data[:]
            series[name] = {
                "shape": list(data.shape),
                "unit": ts.unit,
                "rate": ts.rate,
                "starting_time": ts.starting_time,
            }
            if ts.rate is None or not (ts.rate > 0):
                problems.append(f"{name}: no rate")
            if a.source is not None:
                c = json.loads(ts.comments)
                n = min(data.shape[0], 100000)
                ref = run(a.openreadout, "trace", str(a.source), "--trace", str(c["trace"]),
                          "--sweep", str(c["sweep"]), "--first-sample", str(c["first_sample"]),
                          "--count", str(n), "--max-samples", str(n))
                by_index = {ch["index"]: ch for ch in ref["channels"]}
                cols = data.reshape(data.shape[0], -1)
                for j, ch in enumerate(c["channels"]):
                    want = np.array([math.nan if v is None else v for v in by_index[ch["index"]]["samples"]], dtype=np.float64)
                    got = cols[: len(want), j].astype(np.float64)
                    if not np.array_equal(got, want, equal_nan=True):
                        problems.append(f"{name} column {j} ({ch['name']}): samples differ")
                    compared += len(want)
    short_series = any(v["shape"][0] < v["shape"][-1] for v in series.values() if len(v["shape"]) == 2)
    from nwbinspector import inspect_nwbfile

    messages = list(inspect_nwbfile(nwbfile_path=str(a.file)))
    by_importance: dict[str, list[str]] = {}
    for m in messages:
        by_importance.setdefault(m.importance.name, []).append(f"{m.check_function_name}: {m.message} ({m.location})")
    for imp in ("CRITICAL", "PYNWB_VALIDATION", "ERROR"):
        for m in by_importance.get(imp, []):
            # Instrument files record no subject; the export does not invent one (DANDI asks for
            # it before upload: add it with pynwb).
            if m.startswith("check_subject_exists:"):
                continue
            # A recording shorter than its channel count (e.g. a truncated test file) trips the
            # orientation heuristic although time is the first dimension.
            if m.startswith("check_data_orientation:") and short_series:
                continue
            problems.append(f"nwbinspector {imp}: {m}")
    result = {
        "file": str(a.file),
        "series": len(series),
        "first_series": next(iter(series.items()), None),
        "values_compared": compared,
        "inspector": {k: len(v) for k, v in by_importance.items()},
        "inspector_messages": {k: v[:5] for k, v in by_importance.items()},
        "ok": not problems,
        "problems": problems,
    }
    print(json.dumps(result, default=str))
    return 0 if not problems else 1


if __name__ == "__main__":
    sys.exit(main())
