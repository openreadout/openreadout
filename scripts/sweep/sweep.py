#!/usr/bin/env python3
"""Run the CLI over the development corpus and record crashes, memory and nondeterminism.

Python 3.11 or newer, standard library only. See README.md in this folder.
"""

import argparse
import ctypes
import hashlib
import json
import os
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import tomllib

ROOT = Path(__file__).resolve().parents[2]
LIMIT = 3 * 1024**3
LOCK = threading.Lock()
PROC = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True) if sys.platform == "darwin" else None


def rss(pid):
    if PROC:
        buf = ctypes.create_string_buffer(1024)
        if PROC.proc_pid_rusage(pid, 2, ctypes.byref(buf)) == 0:
            return struct.unpack_from("Q", buf, 64)[0]
    else:
        try:
            return int(Path(f"/proc/{pid}/statm").read_text().split()[1]) * os.sysconf(
                "SC_PAGE_SIZE"
            )
        except (FileNotFoundError, ProcessLookupError):
            pass
    return 0


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def objects(value):
    if isinstance(value, dict):
        yield value
        for item in value.values():
            yield from objects(item)
    elif isinstance(value, list):
        for item in value:
            yield from objects(item)


def execute(command, folder, timeout):
    """Run one command and return its exit status, peak RSS, output and parsed JSON.

    The process is killed when its RSS passes LIMIT or it runs longer than `timeout` seconds.
    RSS is sampled every 50 ms, so a short spike between samples can pass the limit; the peak
    reported by `wait4` after exit catches it.
    """
    start = time.monotonic()
    peak = 0
    reason = None
    env = dict(os.environ, RAYON_NUM_THREADS="1", NO_COLOR="1")
    with (folder / "stdout").open("wb") as out, (folder / "stderr").open("wb") as err:
        p = subprocess.Popen(command, stdout=out, stderr=err, env=env, start_new_session=True)
        while True:
            pid, status, usage = os.wait4(p.pid, os.WNOHANG)
            if pid:
                break
            peak = max(peak, rss(p.pid))
            if peak > LIMIT:
                reason = "memory"
            elif time.monotonic() - start > timeout:
                reason = "timeout"
            if reason:
                os.killpg(p.pid, signal.SIGKILL)
                _, status, usage = os.wait4(p.pid, 0)
                break
            time.sleep(0.05)
        p.returncode = os.waitstatus_to_exitcode(status)
    # ru_maxrss is in bytes on macOS and in KiB on Linux.
    peak = max(peak, usage.ru_maxrss * (1 if sys.platform == "darwin" else 1024))
    stdout = (folder / "stdout").read_text(errors="replace")
    stderr = (folder / "stderr").read_text(errors="replace")
    try:
        value = json.loads(stdout)
    except ValueError:
        value = None
    return {
        "exit": p.returncode,
        "seconds": round(time.monotonic() - start, 4),
        "rss_bytes": peak,
        "terminated": reason,
        "stdout": stdout,
        "stderr": stderr,
        "json": value,
    }


def pair(args, entry, name, options, folder, output=None):
    previous = getattr(args, "completed", {}).get((entry["id"], entry["filename"], name))
    if previous is not None:
        return previous
    command = [str(args.binary), "--threads", "1", *options]
    runs = []
    for _ in range(2):
        artifacts = folder / "output"
        shutil.rmtree(artifacts, ignore_errors=True)
        artifacts.mkdir()
        r = execute(command, folder, args.timeout)
        r["artifacts"] = {
            str(p.relative_to(artifacts)): digest(p)
            for p in sorted(artifacts.rglob("*"))
            if p.is_file()
        }
        runs.append(r)
    findings = set()
    for r in runs:
        if (
            r["exit"] == 101
            or "panicked" in r["stderr"]
            or "error: internal error:" in r["stderr"]
            or any(o.get("code") == "internal_panic" for o in objects(r["json"]))
        ):
            findings.add("panic")
        if r["exit"] is not None and r["exit"] < 0:
            findings.add("signal")
        if r["terminated"]:
            findings.add(r["terminated"])
        if r["rss_bytes"] > LIMIT:
            findings.add("memory")
        if r["exit"] in (4, 5) and entry["id"] in args.passed:
            findings.add("oracle_pass_exit_4_5")
        if isinstance(r["json"], (dict, list)):
            errors = [o["error"] for o in objects(r["json"]) if isinstance(o.get("error"), dict)]
            if any(not e.get("hint") for e in errors):
                findings.add("missing_hint")
        if name.startswith("export"):
            reports = [o["verified"] for o in objects(r["json"]) if "verified" in o]
            if r["exit"] == 0 and (not reports or not all(reports)):
                findings.add("readback")
            if r["exit"] and any(
                s in (r["stdout"] + r["stderr"]).lower()
                for s in ("read-back", "readback", "verification", "verify")
            ):
                findings.add("readback_candidate")

    def comparable(r):
        return (
            r["exit"],
            r["json"] if r["json"] is not None else r["stdout"],
            r["stderr"],
            r["artifacts"],
        )

    if comparable(runs[0]) != comparable(runs[1]):
        findings.add("nondeterministic")
    record = {
        "id": entry["id"],
        "filename": entry["filename"],
        "format": entry["format"],
        "command": name,
        "argv": command,
        "findings": sorted(findings),
        "runs": runs,
    }
    with LOCK:
        with args.results.open("a") as f:
            f.write(json.dumps(record, separators=(",", ":")) + "\n")
    return runs[0].get("json") or {}


def commands(info, entry):
    yield "detect", ["info", "--view", "format"], None
    yield "check", ["check"], None
    yield "preview", ["preview", "--max-size", "256", "--output", "@preview.png"], "preview.png"
    yield (
        "stats",
        ["stats", "--image", "0", "--select", "c=0", "--select", "z=0", "--select", "t=0"],
        None,
    )
    if info.get("traces"):
        yield (
            "trace",
            ["trace", "--trace", "0", "--sweep", "0", "--count", "32", "--max-samples", "32"],
            None,
        )
    if info.get("tables"):
        yield "table", ["table", "--table", "0", "--max-rows", "16"], None
    if info.get("spectra"):
        yield "spectrum", ["spectrum", "--spectrum", "0", "--max-points", "32"], None
        yield "scans", ["scans", "--limit", "16"], None
    if info.get("spectra") or info.get("traces"):
        yield "chromatogram", ["analyze", "chromatogram", "--max-points", "32"], None


def exports(info, entry):
    if info.get("images"):
        for fmt, ext in [("ome-tiff", "ome.tiff"), ("ome-zarr", "ome.zarr")]:
            yield (
                fmt,
                ext,
                [
                    "--image",
                    "0",
                    "--select",
                    "c=0",
                    "--select",
                    "z=0",
                    "--select",
                    "t=0",
                    "--pyramid",
                    "none",
                ],
            )
    if info.get("tables") or info.get("traces"):
        target = ["--table", "0"] if info.get("tables") else ["--trace", "0", "--sweep", "0"]
        for fmt in ("csv", "parquet", "arrow"):
            yield fmt, fmt, [*target, "--rows", "0-15"]
    if info.get("spectra"):
        yield "mzml", "mzML", []
        for fmt in ("parquet", "arrow"):
            yield fmt + "-spectra", fmt, ["--spectra"]
    if info.get("traces"):
        # Writers decide whether the reported trace has the required axis and units.
        yield "nwb", "nwb", ["--trace", "0", "--sweep", "0", "--rows", "0-31"]
        yield "jcamp", "jdx", ["--trace", "0", "--sweep", "0", "--rows", "0-31"]
    if entry["format"] == "plate":
        yield "asm", "asm.json", []
    if entry["format"] in (
        "rdml",
        "applied-biosystems-eds",
        "rotor-gene-rex",
        "bio-rad-pcrd",
        "roche-lightcycler-ixo",
        "qpcr-results-export",
    ):
        yield "rdml", "rdml", []


def sweep_file(args, entry):
    path = args.corpus / entry["filename"]
    with tempfile.TemporaryDirectory(prefix="sweep-", dir=args.scratch) as tmp:
        folder = Path(tmp)
        envelope = pair(args, entry, "info", ["info", str(path), "--json"], folder)
        info = envelope.get("data", envelope)
        if not isinstance(info, dict):
            info = {}
        for name, options, output in commands(info, entry):
            options = [str(folder / "output" / o[1:]) if o.startswith("@") else o for o in options]
            pair(args, entry, name, [*options, str(path), "--json"], folder, output)
        size = (
            path.stat().st_size
            if path.is_file()
            else sum(p.stat().st_size for p in path.rglob("*") if p.is_file())
        )
        if size <= 2 * 1024**3:
            for fmt, ext, options in exports(info, entry):
                pair(
                    args,
                    entry,
                    "export-" + fmt,
                    [
                        "export",
                        str(path),
                        "--format",
                        fmt.removesuffix("-spectra"),
                        "--output",
                        str(folder / "output" / ("export." + ext)),
                        "--json",
                        *options,
                    ],
                    folder,
                )
        else:
            with LOCK, args.results.open("a") as f:
                f.write(
                    json.dumps(
                        {
                            "id": entry["id"],
                            "filename": entry["filename"],
                            "skip": "exports: input exceeds 2 GiB",
                            "bytes": size,
                        }
                    )
                    + "\n"
                )
    with LOCK:
        print(entry["id"], flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/openreadout")
    parser.add_argument(
        "--corpus",
        type=Path,
        default=os.environ.get("OPENREADOUT_CORPUS_DIR", ROOT / "corpus/files"),
    )
    parser.add_argument("--results", type=Path, required=True)
    parser.add_argument("--oracle-results", type=Path)
    parser.add_argument("--jobs", type=int, choices=range(1, 4), default=2)
    parser.add_argument("--timeout", type=int, default=120)
    parser.add_argument("--only", default="")
    parser.add_argument(
        "--resume", action="store_true", help="reuse completed command pairs in the results file"
    )
    parser.add_argument("--scratch", type=Path, default=ROOT / "target/sweep-tmp")
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.scratch.mkdir(parents=True, exist_ok=True)
    args.results.parent.mkdir(parents=True, exist_ok=True)
    args.completed = {}
    if args.resume and args.results.exists():
        with args.results.open() as f:
            for line in f:
                r = json.loads(line)
                if "runs" in r:
                    args.completed[r["id"], r["filename"], r["command"]] = (
                        r["runs"][0].get("json") or {}
                    )
    elif args.results.exists():
        parser.error("results already exist; choose a new path or use --resume")
    args.passed = set()
    if args.oracle_results:
        args.passed = {
            r["id"]
            for line in args.oracle_results.read_text().splitlines()
            if (r := json.loads(line)).get("status") == "pass"
        }
    manifest = tomllib.loads((ROOT / "corpus/manifest.toml").read_text())["file"]
    entries = []
    for entry in manifest:
        reason = (
            "heldout"
            if entry.get("role") == "heldout" or entry.get("tier") == "heldout"
            else "missing"
            if not (args.corpus / entry["filename"]).exists()
            else None
        )
        if args.only and args.only not in entry["id"]:
            continue
        if reason:
            with args.results.open("a") as f:
                f.write(
                    json.dumps({"id": entry["id"], "filename": entry["filename"], "skip": reason})
                    + "\n"
                )
        else:
            entries.append(entry)
    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        list(pool.map(lambda entry: sweep_file(args, entry), entries))


if __name__ == "__main__":
    main()
