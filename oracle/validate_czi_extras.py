#!/usr/bin/env python
"""Cross-check openreadout's CZI attachments, time stamps, events and per-plane acquisition
times against czifile (BSD-3, run as a reader). Prints one line per file and a summary.

Usage: cd oracle && uv run python validate_czi_extras.py ../target/release/openreadout [corpus dir]
"""
from __future__ import annotations

import json
import re
import subprocess
import sys
import tempfile
import warnings
from pathlib import Path

import czifile
import xxhash

warnings.filterwarnings("ignore")


def run(binary: str, *args: str) -> dict:
    r = subprocess.run([binary, *args, "--json"], capture_output=True, text=True)
    return json.loads(r.stdout)


def main() -> int:
    binary = sys.argv[1]
    corpus = Path(sys.argv[2] if len(sys.argv) > 2 else Path(__file__).resolve().parent.parent / "corpus/files")
    totals = {"files": 0, "attachments": 0, "attachment_bytes_ok": 0, "time_stamps_ok": 0, "events_ok": 0,
              "planes_with_time": 0, "plane_times_ok": 0, "failures": 0}
    with tempfile.TemporaryDirectory() as tmp:
        for p in sorted(corpus.glob("*.czi")):
            if " (" in p.name or p.stat().st_size > 1_000_000_000:
                continue
            info = run(binary, "info", str(p))
            if not info.get("ok"):
                continue
            totals["files"] += 1
            problems = []
            with czifile.CziFile(p) as f:
                # attachments: raw bytes identical
                for i, a in enumerate(f.attachment_directory):
                    totals["attachments"] += 1
                    want = xxhash.xxh3_128_hexdigest(bytes(a.read_segment_data(f).data(raw=True)))
                    out = Path(tmp) / f"a{i}"
                    got = run(binary, "extract", str(p), f"#{i}", "-o", str(out), "--overwrite")
                    if got.get("ok") and got["data"]["xxh3"] == want and got["data"]["attachment"]["name"] == a.name:
                        totals["attachment_bytes_ok"] += 1
                    else:
                        problems.append(f"attachment #{i} {a.name}: {got.get('error') or 'bytes differ'}")
                # time stamps and events
                extra = info["data"]["images"][0].get("extra", {})
                ts = f.timestamps
                if ts is not None:
                    im0 = info["data"]["images"][0]
                    want = [float(v) for v in ts[: im0["size_t"]]]
                    if extra.get("time_stamps_s") == want:
                        totals["time_stamps_ok"] += 1
                    else:
                        problems.append(f"time stamps {extra.get('time_stamps_s', [])[:3]} != {want[:3]}")
                for a in f.attachment_directory:
                    if a.content_file_type == "CZEVL":
                        evs = a.read_segment_data(f).data()
                        want = [{"time_s": e.time, "description": e.description} for e in evs]
                        got = [{"time_s": e["time_s"], "description": e["description"]} for e in extra.get("events", [])]
                        if got == want:
                            totals["events_ok"] += 1
                        else:
                            problems.append(f"events {got} != {want}")
                # per-plane acquisition times: each must be the AcquisitionTime of a subblock at that (S, C, Z, T)
                dump = run(binary, "info", str(p), "--view", "full", "--no-provenance", "--max-frames", "-1")
                planes = [dict(fr, image=im["index"]) for im in dump["data"]["file"]["images"]
                          for fr in im.get("extra", {}).get("frames", [])] if dump.get("ok") else []
                if any(pl.get("acquired_at") for pl in planes):
                    times: dict[tuple, set] = {}
                    for de in f.subblock_directory:
                        if de.pyramid_type != 0:
                            continue
                        d = dict(zip(de.dims, de.start))
                        sb = de.read_segment_data(f)
                        m = re.search(r"<AcquisitionTime>(.*?)</AcquisitionTime>", sb.metadata(asdict=False) or "")
                        if m:
                            # czifile keeps S and M out of `dims`; the scene is `scene_index` (-1 = none)
                            scene = max(int(getattr(de, "scene_index", -1)), 0)
                            times.setdefault((scene, d.get("C", 0), d.get("Z", 0), d.get("T", 0)), set()).add(m.group(1))
                    scene_ids = sorted({k[0] for k in times})
                    for pl in planes:
                        if not pl.get("acquired_at"):
                            continue
                        totals["planes_with_time"] += 1
                        s = scene_ids[pl["image"]] if pl["image"] < len(scene_ids) else pl["image"]
                        # subblocks spanning several planes (line scans) carry one time for all of them
                        cands = set().union(*[v for k, v in times.items() if k[0] == s])
                        exact = times.get((s, pl["c"], pl["z"], pl["t"]))
                        if (exact and pl["acquired_at"] in exact) or (not exact and pl["acquired_at"] in cands):
                            totals["plane_times_ok"] += 1
                        else:
                            problems.append(f"plane c={pl['c']} z={pl['z']} t={pl['t']}: {pl['acquired_at']} not a subblock time")
                            break
            if problems:
                totals["failures"] += 1
            print(f"{'FAIL' if problems else 'ok  '} {p.name}: {'; '.join(problems[:3])}")
    print(json.dumps(totals, indent=1))
    return 1 if totals["failures"] else 0


if __name__ == "__main__":
    sys.exit(main())
