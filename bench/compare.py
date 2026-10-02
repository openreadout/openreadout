#!/usr/bin/env python3
"""End-to-end comparison: OpenReadout vs Bio-Formats (bftools) vs bioio and native Python readers.

Times, on the same files with a warm page cache and several repetitions (median, min, max):

* metadata: `openreadout info --json` vs `showinf -nopix` vs bioio (or the format's usual
  Python reader: flowio, pyabf, pyteomics) loading the same header metadata;
* export:   `openreadout export --to ome-tiff` vs `bfconvert` vs bioio `BioImage.save`, all
  writing an uncompressed OME-TIFF (plus OpenReadout with its default deflate);
* scaling:  `openreadout export` with `--threads 1/2/4/8` (wall time and peak RSS).

Every command runs as its own process under `/usr/bin/time` (peak RSS). Process start-up is
measured separately (`openreadout --version`, `showinf -version`, `python -c "import bioio..."`),
and the Python tools also time their work *inside* the process, so JVM/Python start-up is never
silently charged to (or hidden from) per-file work.

Results are merged into a JSON file (default bench/results/compare.json) keyed by
(section, tool, file), so a long run can be split (`--section`, `--only`) and resumed; `report`
renders the Markdown tables used in book/src/project/performance.md.

Run it with the oracle environment, which has bioio, its plugins and the Python readers:

    uv run --project oracle python bench/compare.py run --bin target/release/openreadout
    uv run --project oracle python bench/compare.py report

Benchmarks on a busy machine are noise: before each measurement the script waits (up to
--wait seconds) for the 1-minute load average to drop below --max-load and records the load
average with every number; a measurement taken above the threshold is flagged `provisional`.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_OUT = ROOT / "bench" / "results" / "compare.json"
RSS_CAP = 4 << 30  # kill any benchmark process above ~4 GiB resident

# (id, file, kind): kind picks the Python comparator and whether export runs.
FILES = [
    ("czi", "aics-s-3-t-1-c-3-z-5.czi", "image"),
    ("czi-3.7GB", "zenodo10577621-Young-mouse.czi", "image-header-only"),
    ("lif-3GB", "ome-imagesc-110520-AMR1.lif", "image-header-only"),
    ("nd2", "ome-jonas-control002.nd2", "image"),
    ("lif", "zenodo14976703-Convalaria-LambdaScan.lif", "image"),
    ("ome-tiff", "aics-s-3-t-1-c-3-z-5.ome.tiff", "image"),
    ("fcs", "flowio-m0-wm278-s1-zero-gain.fcs", "fcs"),
    ("abf", "pyabf-2020-07-29-0062.abf", "abf"),
    ("thermo-raw", "mtbls404-QC1_001.raw", "thermo"),
    ("mzml", "mtbls404-QC1_001.mzML", "mzml"),
]
# Inputs for the --threads scaling runs: (id, file, compression). Raw ND2 is I/O and memcpy
# bound, JPEG XR CZI decode bound, deflate output compression bound; the 3 GB LIF shows that
# peak memory does not grow with the file.
SCALING_FILES = [
    ("nd2-218MB-none", "ome-karl-sample-image.nd2", "none"),
    ("nd2-218MB-deflate", "ome-karl-sample-image.nd2", "deflate"),
    ("czi-jpegxr-69MB-none", "openslide-zeiss-5-jxr.czi", "none"),
    ("lif-3GB-none", "ome-imagesc-110520-AMR1.lif", "none"),
]


# --------------------------------------------------------------------------------------------
# in-process helpers (run as `compare.py _py <op> <kind> <file> [out]`): print one JSON line
# with the seconds spent after imports.
# --------------------------------------------------------------------------------------------


def _py_work(op: str, kind: str, path: str, out: str | None) -> float:
    """Import first (start-up), then time only the per-file work."""
    if kind.startswith("image"):
        import bioio_czi  # noqa: F401  (plugins imported up front: start-up, not per-file work)
        import bioio_lif  # noqa: F401
        import bioio_nd2  # noqa: F401
        import bioio_ome_tiff  # noqa: F401
        from bioio import BioImage

        t0 = time.perf_counter()
        img = BioImage(path)
        if op == "info":
            for s in img.scenes:
                img.set_scene(s)
                _ = (img.dims, img.dtype, img.channel_names, img.physical_pixel_sizes)
            _ = img.metadata
        else:
            img.save(out)
    elif kind == "fcs":
        import flowio

        t0 = time.perf_counter()
        flowio.FlowData(path, only_text=True)
    elif kind == "abf":
        import pyabf

        t0 = time.perf_counter()
        abf = pyabf.ABF(path, loadData=False)
        _ = (abf.channelCount, abf.sweepCount, abf.dataRate, abf.adcNames, abf.adcUnits)
    elif kind == "mzml":
        from pyteomics import mzml

        t0 = time.perf_counter()
        with mzml.PreIndexedMzML(path) as r:
            _ = len(r)
            _ = r[0]
            _ = r[len(r) - 1]
    else:
        raise SystemExit(f"no Python comparator for {kind}")
    return time.perf_counter() - t0


def py_helper(argv: list[str]) -> None:
    op, kind, path = argv[0], argv[1], argv[2]
    out = argv[3] if len(argv) > 3 else None
    print(json.dumps({"work_s": _py_work(op, kind, path, out)}))


# --------------------------------------------------------------------------------------------
# measurement
# --------------------------------------------------------------------------------------------


def wait_quiet(max_load: float, wait_s: float) -> tuple[float, bool]:
    """Wait until the 1-minute load average is below max_load (or wait_s passes)."""
    deadline = time.monotonic() + wait_s
    while True:
        load = os.getloadavg()[0]
        if load < max_load:
            return load, True
        if time.monotonic() >= deadline:
            return load, False
        print(f"  load {load:.2f} >= {max_load}; waiting...", file=sys.stderr, flush=True)
        time.sleep(min(60.0, max(1.0, deadline - time.monotonic())))


def _parse_rss(stderr: str) -> int | None:
    m = re.search(r"(\d+)\s+maximum resident set size", stderr)  # macOS: bytes
    if m:
        return int(m.group(1))
    m = re.search(r"Maximum resident set size \(kbytes\):\s*(\d+)", stderr)  # GNU time -v
    if m:
        return int(m.group(1)) * 1024
    return None


def _time_prefix() -> list[str]:
    if not Path("/usr/bin/time").exists():
        return []
    return ["/usr/bin/time", "-l"] if sys.platform == "darwin" else ["/usr/bin/time", "-v"]


def run_once(cmd: list[str], env: dict | None = None) -> dict:
    """Run cmd once; return wall seconds, peak RSS and the inner work time if reported."""
    try:
        import psutil
    except ImportError:  # watchdog is best effort
        psutil = None
    full = _time_prefix() + cmd
    # Output goes to temporary files, not pipes: a tool that prints a lot (showinf) would
    # otherwise block on a full pipe while we poll it.
    fout = tempfile.TemporaryFile()
    ferr = tempfile.TemporaryFile()
    t0 = time.perf_counter()
    p = subprocess.Popen(full, stdin=subprocess.DEVNULL, stdout=fout, stderr=ferr, env=env)
    killed = threading.Event()
    done = threading.Event()

    def watchdog() -> None:
        # Separate thread, so the wall time is taken the moment the process exits.
        try:
            proc = psutil.Process(p.pid)
        except psutil.Error:
            return
        while not done.wait(0.05):
            try:
                rss = sum(c.memory_info().rss for c in [proc, *proc.children(recursive=True)])
            except psutil.Error:
                continue
            if rss > RSS_CAP:
                for c in proc.children(recursive=True):
                    c.kill()
                p.kill()
                killed.set()
                return

    w = threading.Thread(target=watchdog, daemon=True) if psutil is not None else None
    if w:
        w.start()
    p.wait()
    wall = time.perf_counter() - t0
    done.set()
    if w:
        w.join()
    fout.seek(0)
    ferr.seek(0)
    out, err = fout.read(), ferr.read()
    fout.close()
    ferr.close()
    serr = err.decode(errors="replace")
    res = {"wall_s": wall, "rss_bytes": _parse_rss(serr), "exit": p.returncode}
    if killed.is_set():
        res["killed_rss_cap"] = True
    for line in out.decode(errors="replace").splitlines()[::-1]:
        if line.startswith('{"work_s"'):
            res["work_s"] = json.loads(line)["work_s"]
            break
    if p.returncode != 0:
        res["stderr_tail"] = serr[-600:]
    return res


def measure(cmd: list[str], reps: int, max_load: float, wait_s: float, cleanup=None) -> dict:
    load_before, quiet = wait_quiet(max_load, wait_s)
    run_once(cmd)  # warm the page cache (and the JIT/bytecode caches the tool keeps on disk)
    if cleanup:
        cleanup()
    runs = []
    for _ in range(reps):
        runs.append(run_once(cmd))
        if cleanup:
            cleanup()
    ok = [r for r in runs if r["exit"] == 0]
    out = {
        "cmd": " ".join(cmd),
        "reps": reps,
        "ok": len(ok) == len(runs),
        "load_1m_before": round(load_before, 2),
        "load_1m_after": round(os.getloadavg()[0], 2),
        "provisional": not quiet,
    }
    if not ok:
        out["error"] = runs[-1].get("stderr_tail", "")
        return out
    walls = [r["wall_s"] for r in ok]
    out.update(
        median_s=statistics.median(walls),
        min_s=min(walls),
        max_s=max(walls),
        peak_rss_bytes=max((r["rss_bytes"] or 0) for r in ok),
    )
    works = [r["work_s"] for r in ok if "work_s" in r]
    if works:
        out.update(work_median_s=statistics.median(works), work_min_s=min(works), work_max_s=max(works))
    return out


# --------------------------------------------------------------------------------------------
# tools
# --------------------------------------------------------------------------------------------


def bftools_dir() -> Path | None:
    for c in [os.environ.get("BFTOOLS_DIR"), ROOT / "oracle" / "bftools" / "bftools"]:
        if c and (Path(c) / "showinf").exists():
            return Path(c)
    # a git worktree: fall back to the main checkout's oracle/bftools
    for parent in ROOT.parents:
        c = parent / "oracle" / "bftools" / "bftools"
        if (c / "showinf").exists():
            return c
    return None


def versions(bin_path: str) -> dict:
    v = {
        "machine": platform.machine(),
        "os": f"{platform.system()} {platform.release()}",
        "python": platform.python_version(),
    }
    if sys.platform == "darwin":
        v["os"] = "macOS " + subprocess.run(["sw_vers", "-productVersion"], capture_output=True, text=True).stdout.strip()
        v["cpu"] = subprocess.run(["sysctl", "-n", "machdep.cpu.brand_string"], capture_output=True, text=True).stdout.strip()
        v["cpus"] = os.cpu_count()
        mem = subprocess.run(["sysctl", "-n", "hw.memsize"], capture_output=True, text=True).stdout.strip()
        v["memory_gib"] = int(mem) // (1 << 30) if mem.isdigit() else None
    v["openreadout"] = subprocess.run([bin_path, "--version"], capture_output=True, text=True).stdout.strip()
    bf = bftools_dir()
    if bf:
        o = subprocess.run([str(bf / "showinf"), "-version", "-no-upgrade"], capture_output=True, text=True)
        m = re.search(r"Version:\s*(\S+)", o.stdout)
        v["bio-formats"] = m.group(1) if m else o.stdout.strip()[:80]
        j = subprocess.run(["java", "-version"], capture_output=True, text=True)
        v["java"] = (j.stderr.splitlines() or [""])[0]
    try:
        import importlib.metadata as md

        for n in ["bioio", "bioio-czi", "bioio-nd2", "bioio-lif", "bioio-ome-tiff", "tifffile", "flowio", "pyabf", "pyteomics"]:
            try:
                v[n] = md.version(n)
            except md.PackageNotFoundError:
                pass
    except ImportError:
        pass
    return v


def load_results(path: Path) -> dict:
    if path.exists():
        return json.loads(path.read_text())
    return {"environment": {}, "results": {}}


def save_results(path: Path, data: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(data, indent=1, sort_keys=True))
    tmp.replace(path)


def cmd_run(a: argparse.Namespace) -> None:
    corpus = Path(a.corpus)
    bin_path = str(Path(a.bin).resolve())
    bf = None if a.ours_only else bftools_dir()
    others = not a.ours_only
    data = load_results(Path(a.out))
    data["environment"] = versions(bin_path)
    data["environment"]["reps"] = a.reps
    data["environment"]["max_load"] = a.max_load
    if a.label:
        data["environment"]["label"] = a.label
    py = sys.executable
    work = Path(tempfile.mkdtemp(prefix="openreadout-bench-", dir=a.tmp))
    sections = set(a.section)

    def record(section: str, tool: str, fid: str, res: dict) -> None:
        key = f"{section}|{tool}|{fid}"
        res["recorded_at"] = time.strftime("%Y-%m-%dT%H:%M:%S")
        data["results"][key] = res
        save_results(Path(a.out), data)
        med = res.get("median_s")
        extra = f" work {res['work_median_s']:.3f}s" if "work_median_s" in res else ""
        rss = f" rss {res.get('peak_rss_bytes', 0) / (1 << 20):.0f} MiB" if med is not None else ""
        state = f"{med:.3f}s" if med is not None else f"FAILED {res.get('error', '')[-200:]}"
        print(f"{section:8} {tool:22} {fid:16} {state}{extra}{rss} load {res['load_1m_before']}", flush=True)

    def selected(fid: str) -> bool:
        return not a.only or any(o in fid for o in a.only)

    if "startup" in sections:
        record("startup", "openreadout", "-", measure([bin_path, "--version"], a.reps, a.max_load, a.wait))
        if bf:
            record("startup", "bio-formats", "-", measure([str(bf / "showinf"), "-version", "-no-upgrade"], a.reps, a.max_load, a.wait))
        if others:
            record("startup", "python+bioio", "-", measure([py, "-c", "import bioio, bioio_czi, bioio_nd2, bioio_lif, bioio_ome_tiff"], a.reps, a.max_load, a.wait))
            record("startup", "python", "-", measure([py, "-c", "pass"], a.reps, a.max_load, a.wait))

    for fid, name, kind in FILES:
        f = corpus / name
        if not selected(fid):
            continue
        if not f.exists():
            print(f"skip {fid}: {f} not found", file=sys.stderr)
            continue
        if "info" in sections:
            record("info", "openreadout", fid, measure([bin_path, "info", str(f), "--json"], a.reps, a.max_load, a.wait))
            if bf and kind.startswith("image"):
                record("info", "bio-formats showinf", fid, measure([str(bf / "showinf"), "-nopix", "-no-upgrade", str(f)], a.reps, a.max_load, a.wait))
            if others and kind != "thermo":
                tool = "bioio" if kind.startswith("image") else {"fcs": "flowio", "abf": "pyabf", "mzml": "pyteomics"}[kind]
                record("info", tool, fid, measure([py, __file__, "_py", "info", kind, str(f)], a.reps, a.max_load, a.wait))
        if "export" in sections and kind == "image":
            out = work / "out.ome.tiff"

            def clean() -> None:
                for p in work.iterdir():
                    shutil.rmtree(p) if p.is_dir() else p.unlink()

            record("export", "openreadout (none)", fid, measure([bin_path, "export", str(f), "-o", str(out), "--overwrite", "--compression", "none", "--quiet"], a.reps, a.max_load, a.wait, clean))
            record("export", "openreadout (deflate)", fid, measure([bin_path, "export", str(f), "-o", str(out), "--overwrite", "--quiet"], a.reps, a.max_load, a.wait, clean))
            if bf:
                record("export", "bfconvert", fid, measure([str(bf / "bfconvert"), "-no-upgrade", "-overwrite", "-bigtiff", str(f), str(out)], a.reps, a.max_load, a.wait, clean))
            if others:
                record("export", "bioio save", fid, measure([py, __file__, "_py", "export", kind, str(f), str(out)], a.reps, a.max_load, a.wait, clean))

    if "scaling" in sections:
        for fid, name, compression in SCALING_FILES:
            f = corpus / name
            if not selected(fid) or not f.exists():
                continue
            out = work / "out.ome.tiff"

            def clean() -> None:
                if out.exists():
                    out.unlink()

            for n in a.threads:
                reps = a.reps if f.stat().st_size < (1 << 30) else min(a.reps, 3)
                record("scaling", f"threads={n}", fid, measure([bin_path, "--threads", str(n), "export", str(f), "-o", str(out), "--overwrite", "--compression", compression, "--quiet"], reps, a.max_load, a.wait, clean))
    shutil.rmtree(work, ignore_errors=True)


# --------------------------------------------------------------------------------------------
# report
# --------------------------------------------------------------------------------------------


def _fmt_s(s: float | None) -> str:
    if s is None:
        return "—"
    if s < 1:
        return f"{s * 1000:.0f} ms" if s >= 0.01 else f"{s * 1000:.1f} ms"
    return f"{s:.2f} s"


def _cell(r: dict | None, inner: bool = False) -> str:
    if not r:
        return "n/a"
    if "median_s" not in r:
        return "failed"
    s = f"{_fmt_s(r['median_s'])} ({_fmt_s(r['min_s'])}–{_fmt_s(r['max_s'])})"
    if inner and "work_median_s" in r:
        s += f"; in-process {_fmt_s(r['work_median_s'])}"
    s += f"; {r['peak_rss_bytes'] / (1 << 20):.0f} MiB"
    if r.get("provisional"):
        s += " †"
    return s


def cmd_report(a: argparse.Namespace) -> None:
    data = load_results(Path(a.out))
    env, res = data["environment"], data["results"]
    lines = ["Environment: " + ", ".join(f"{k} {v}" for k, v in env.items()), ""]
    loads = [r["load_1m_before"] for r in res.values() if "load_1m_before" in r]
    if loads:
        lines.append(f"1-minute load average before each measurement: {min(loads):.2f}–{max(loads):.2f}"
                     f" (threshold {env.get('max_load')}); † = measured above the threshold (provisional).")
        lines.append("")

    def get(section: str, tool: str, fid: str) -> dict | None:
        return res.get(f"{section}|{tool}|{fid}")

    lines += ["Start-up (no file): median (min–max); peak RSS", "", "| Process | Time |", "|---|---|"]
    for tool in ["openreadout", "bio-formats", "python", "python+bioio"]:
        if get("startup", tool, "-"):
            lines.append(f"| {tool} | {_cell(get('startup', tool, '-'))} |")
    lines += ["", "Metadata (`info`): median (min–max) wall time per process; peak RSS", "",
              "| File | OpenReadout `info` | Bio-Formats `showinf -nopix` | Python reader |", "|---|---|---|---|"]
    for fid, name, kind in FILES:
        py_tool = "bioio" if kind.startswith("image") else {"fcs": "flowio", "abf": "pyabf", "mzml": "pyteomics"}.get(kind)
        row = [get("info", "openreadout", fid), get("info", "bio-formats showinf", fid), get("info", py_tool, fid) if py_tool else None]
        if not any(row):
            continue
        py_cell = _cell(row[2], inner=True) + (f" ({py_tool})" if row[2] else "")
        lines.append(f"| {fid} (`{name}`) | {_cell(row[0])} | {_cell(row[1])} | {py_cell} |")
    lines += ["", "Export to OME-TIFF: median (min–max) wall time per process; peak RSS", "",
              "| File | OpenReadout (none) | OpenReadout (deflate) | `bfconvert` | bioio `save` |", "|---|---|---|---|---|"]
    for fid, name, kind in FILES:
        row = [get("export", t, fid) for t in ["openreadout (none)", "openreadout (deflate)", "bfconvert", "bioio save"]]
        if not any(row):
            continue
        lines.append(f"| {fid} | {_cell(row[0])} | {_cell(row[1])} | {_cell(row[2])} | {_cell(row[3], inner=True)} |")
    scal = sorted({k.split("|")[2] for k in res if k.startswith("scaling|")})
    if scal:
        threads = sorted({int(k.split("|")[1].split("=")[1]) for k in res if k.startswith("scaling|")})
        lines += ["", "`--threads` scaling of `export` (compression in the id): median wall time (speed-up vs 1 thread); peak RSS", "",
                  "| File | " + " | ".join(f"{n} thread{'s' if n > 1 else ''}" for n in threads) + " |",
                  "|---|" + "---|" * len(threads)]
        for fid in scal:
            base = (get("scaling", "threads=1", fid) or {}).get("median_s")
            cells = []
            for n in threads:
                r = get("scaling", f"threads={n}", fid)
                if not r or "median_s" not in r:
                    cells.append("—")
                    continue
                sp = f" ({base / r['median_s']:.2f}x)" if base else ""
                cells.append(f"{_fmt_s(r['median_s'])}{sp}; {r['peak_rss_bytes'] / (1 << 20):.0f} MiB" + (" †" if r.get("provisional") else ""))
            lines.append(f"| {fid} | " + " | ".join(cells) + " |")
    if a.baseline:
        base = load_results(Path(a.baseline))["results"]
        lines += ["", f"OpenReadout before ({base_label(a.baseline)}) and after: median wall time; peak RSS", "",
                  "| Measurement | Before | After | Speed-up |", "|---|---|---|---|"]
        for key in sorted(res):
            section, tool, fid = key.split("|")
            if not (tool.startswith("openreadout") or section == "scaling") or key not in base:
                continue
            b, r = base[key], res[key]
            if "median_s" not in b or "median_s" not in r:
                continue
            lines.append(f"| {section} {tool} {fid} | {_fmt_s(b['median_s'])}; {b['peak_rss_bytes'] / (1 << 20):.0f} MiB"
                         f" | {_fmt_s(r['median_s'])}; {r['peak_rss_bytes'] / (1 << 20):.0f} MiB | {b['median_s'] / r['median_s']:.2f}x |")
    print("\n".join(lines))


def base_label(path: str) -> str:
    env = load_results(Path(path)).get("environment", {})
    return env.get("label") or env.get("openreadout", "baseline")


def main() -> None:
    if len(sys.argv) > 1 and sys.argv[1] == "_py":
        py_helper(sys.argv[2:])
        return
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run", help="measure and merge into the results JSON")
    r.add_argument("--bin", default=str(ROOT / "target" / "release" / "openreadout"))
    r.add_argument("--corpus", default=os.environ.get("OPENREADOUT_CORPUS_DIR", str(ROOT / "corpus" / "files")))
    r.add_argument("--out", default=str(DEFAULT_OUT))
    r.add_argument("--reps", type=int, default=5)
    r.add_argument("--max-load", type=float, default=3.0)
    r.add_argument("--wait", type=float, default=1800.0, help="seconds to wait for a quiet machine per measurement")
    r.add_argument("--section", nargs="+", default=["startup", "info", "export", "scaling"],
                   choices=["startup", "info", "export", "scaling"])
    r.add_argument("--only", nargs="*", help="file ids (substring match)")
    r.add_argument("--threads", nargs="+", type=int, default=[1, 2, 4, 8])
    r.add_argument("--label", help="free-text label stored with the environment (e.g. the commit measured)")
    r.add_argument("--ours-only", action="store_true", help="skip Bio-Formats and the Python readers (e.g. to measure an older openreadout build)")
    r.add_argument("--tmp", default=None, help="directory for export outputs (default: system temp)")
    r.set_defaults(func=cmd_run)
    p = sub.add_parser("report", help="print Markdown tables from the results JSON")
    p.add_argument("--out", default=str(DEFAULT_OUT))
    p.add_argument("--baseline", help="results JSON of an older build (run with --ours-only) to compare against")
    p.set_defaults(func=cmd_report)
    a = ap.parse_args()
    a.func(a)


if __name__ == "__main__":
    main()
