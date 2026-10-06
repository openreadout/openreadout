#!/usr/bin/env python3
"""Time and peak memory of `info`, `preview`, `stats` and `export` on the largest corpus files.

Each case runs the binary once to warm the page cache, then `--reps` more times under
`/usr/bin/time -l` (macOS) or `/usr/bin/time -v` (Linux). The median wall time and the largest
peak RSS are reported, with the 1-minute load average after the case. Outputs go to a temporary
directory that is emptied before every run. Results and the table are in
docs/benchmark/large-files.md.

    python3 bench/large_files.py run BINARY [--reps 3] [--only REGEX] [--out results.json]
    python3 bench/large_files.py report BEFORE.json AFTER.json    # Markdown table
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any, Dict, List

ROOT = Path(__file__).resolve().parent.parent
CORPUS = Path(os.environ.get("OPENREADOUT_CORPUS_DIR", ROOT / "corpus" / "files"))

TIMS = (
    "JesusCantoral_sol791-34-25_13_CA4_B05.10_DDA-PASEF_SG_0.8-1.5_Aurora25cm_30min_"
    "Slot2-13_1_9-12-2025_8334.d"
)

# (case id, size, operation, arguments); {C} is the corpus directory, {OUT} the output directory.
CASES: List[tuple[str, str, str, str]] = [
    ("czi-youngmouse", "3.7 GB", "info", "info {C}/zenodo10577621-Young-mouse.czi --json"),
    (
        "czi-youngmouse",
        "3.7 GB",
        "preview",
        "preview {C}/zenodo10577621-Young-mouse.czi -o {OUT}/p.png",
    ),
    (
        "czi-youngmouse",
        "3.7 GB",
        "stats --level 4",
        "stats {C}/zenodo10577621-Young-mouse.czi --level 4 --json",
    ),
    ("czi-kidney", "395 MB", "info", "info {C}/zenodo10577621-Kidney-RAC-3color.czi --json"),
    (
        "czi-kidney",
        "395 MB",
        "preview",
        "preview {C}/zenodo10577621-Kidney-RAC-3color.czi -o {OUT}/p.png",
    ),
    ("czi-kidney", "395 MB", "stats", "stats {C}/zenodo10577621-Kidney-RAC-3color.czi --json"),
    (
        "czi-kidney",
        "395 MB",
        "export ome-tiff",
        "export {C}/zenodo10577621-Kidney-RAC-3color.czi --to ome-tiff -o {OUT}/o.ome.tiff",
    ),
    (
        "czi-kidney",
        "395 MB",
        "export ome-zarr",
        "export {C}/zenodo10577621-Kidney-RAC-3color.czi --to ome-zarr -o {OUT}/o.ome.zarr",
    ),
    ("lif-amr1", "3.0 GB", "info", "info {C}/ome-imagesc-110520-AMR1.lif --json"),
    ("lif-amr1", "3.0 GB", "preview", "preview {C}/ome-imagesc-110520-AMR1.lif -o {OUT}/p.png"),
    (
        "lif-amr1",
        "3.0 GB",
        "stats --image 0",
        "stats {C}/ome-imagesc-110520-AMR1.lif --image 0 --json",
    ),
    ("lif-amr1", "3.0 GB", "stats (27 images)", "stats {C}/ome-imagesc-110520-AMR1.lif --json"),
    (
        "lif-amr1",
        "3.0 GB",
        "export ome-tiff --image 0",
        "export {C}/ome-imagesc-110520-AMR1.lif --image 0 --to ome-tiff -o {OUT}/o.ome.tiff",
    ),
    (
        "lif-amr1",
        "3.0 GB",
        "export ome-zarr (27 images)",
        "export {C}/ome-imagesc-110520-AMR1.lif --to ome-zarr -o {OUT}/o.ome.zarr",
    ),
    ("nd2-sla2", "605 MB", "info", "info {C}/zenodo14231228-Sla2-WT-18-roi.nd2 --json"),
    (
        "nd2-sla2",
        "605 MB",
        "preview",
        "preview {C}/zenodo14231228-Sla2-WT-18-roi.nd2 -o {OUT}/p.png",
    ),
    ("nd2-sla2", "605 MB", "stats", "stats {C}/zenodo14231228-Sla2-WT-18-roi.nd2 --json"),
    (
        "nd2-sla2",
        "605 MB",
        "export ome-tiff",
        "export {C}/zenodo14231228-Sla2-WT-18-roi.nd2 --to ome-tiff -o {OUT}/o.ome.tiff",
    ),
    (
        "nd2-sla2",
        "605 MB",
        "export ome-zarr",
        "export {C}/zenodo14231228-Sla2-WT-18-roi.nd2 --to ome-zarr -o {OUT}/o.ome.zarr",
    ),
    ("oir-stitch", "1.1 GB", "info", "info {C}/zenodo13680725-stitch-a01-g001.oir --json"),
    (
        "oir-stitch",
        "1.1 GB",
        "preview",
        "preview {C}/zenodo13680725-stitch-a01-g001.oir -o {OUT}/p.png",
    ),
    ("oir-stitch", "1.1 GB", "stats", "stats {C}/zenodo13680725-stitch-a01-g001.oir --json"),
    (
        "oir-stitch",
        "1.1 GB",
        "export ome-tiff",
        "export {C}/zenodo13680725-stitch-a01-g001.oir --to ome-tiff -o {OUT}/o.ome.tiff",
    ),
    (
        "mrxs-bmp",
        "1.8 GB",
        "info",
        "info {C}/openslide-mirax2-2-4-bmp-zip/Mirax2.2-4-BMP.mrxs --json",
    ),
    (
        "mrxs-bmp",
        "1.8 GB",
        "preview",
        "preview {C}/openslide-mirax2-2-4-bmp-zip/Mirax2.2-4-BMP.mrxs -o {OUT}/p.png",
    ),
    (
        "mrxs-bmp",
        "1.8 GB",
        "stats --level 5",
        "stats {C}/openslide-mirax2-2-4-bmp-zip/Mirax2.2-4-BMP.mrxs --level 5 --json",
    ),
    (
        "vsi-vs200",
        "1.5 GB",
        "info",
        "info {C}/zenodo8161864-vs200/Image_L2410_Part3_1_H+E_20X.vsi --json",
    ),
    (
        "vsi-vs200",
        "1.5 GB",
        "preview",
        "preview {C}/zenodo8161864-vs200/Image_L2410_Part3_1_H+E_20X.vsi -o {OUT}/p.png",
    ),
    ("svs-cmu1", "177 MB", "info", "info {C}/openslide-aperio-CMU-1.svs --json"),
    ("svs-cmu1", "177 MB", "preview", "preview {C}/openslide-aperio-CMU-1.svs -o {OUT}/p.png"),
    (
        "svs-cmu1",
        "177 MB",
        "stats --level 2",
        "stats {C}/openslide-aperio-CMU-1.svs --level 2 --json",
    ),
    ("qptiff", "422 MB", "info", "info {C}/ome-qptiff-HandEcompressed_Scan1.qptiff --json"),
    (
        "qptiff",
        "422 MB",
        "preview",
        "preview {C}/ome-qptiff-HandEcompressed_Scan1.qptiff -o {OUT}/p.png",
    ),
    ("raw-eclipse", "868 MB", "info", "info {C}/msv99508-eclipse-faims-ptrc-f03.raw --json"),
    (
        "raw-eclipse",
        "868 MB",
        "export mzml",
        "export {C}/msv99508-eclipse-faims-ptrc-f03.raw --to mzml -o {OUT}/o.mzML",
    ),
    (
        "raw-eclipse",
        "868 MB",
        "export parquet",
        "export {C}/msv99508-eclipse-faims-ptrc-f03.raw --to parquet -o {OUT}/o.parquet",
    ),
    ("mzml-ascend", "1.2 GB", "info", "info {C}/pxd059315-ascend-etd-wkl-1.mzML --json"),
    (
        "mzml-ascend",
        "1.2 GB",
        "export parquet",
        "export {C}/pxd059315-ascend-etd-wkl-1.mzML --to parquet -o {OUT}/o.parquet",
    ),
    ("tims-8334", "140 MB", "info", "info {C}/" + TIMS + " --json"),
    ("tims-8334", "140 MB", "export mzml", "export {C}/" + TIMS + " --to mzml -o {OUT}/o.mzML"),
    ("plx-bisver", "1.3 GB", "info", "info {C}/zenodo11586428-bisver170808100-mrg.plx --json"),
    (
        "plx-bisver",
        "1.3 GB",
        "export csv",
        "export {C}/zenodo11586428-bisver170808100-mrg.plx --to csv -o {OUT}/o.csv",
    ),
    (
        "plx-bisver",
        "1.3 GB",
        "export parquet",
        "export {C}/zenodo11586428-bisver170808100-mrg.plx --to parquet -o {OUT}/o.parquet",
    ),
    (
        "plx-bisver",
        "1.3 GB",
        "export nwb",
        "export {C}/zenodo11586428-bisver170808100-mrg.plx --to nwb -o {OUT}/o.nwb",
    ),
    ("sglx-pt03", "1.1 GB", "info", "info {C}/zenodo5899237/Pt03.imec0.lf.bin --json"),
    (
        "sglx-pt03",
        "1.1 GB",
        "export csv",
        "export {C}/zenodo5899237/Pt03.imec0.lf.bin --to csv -o {OUT}/o.csv",
    ),
    (
        "sglx-pt03",
        "1.1 GB",
        "export parquet",
        "export {C}/zenodo5899237/Pt03.imec0.lf.bin --to parquet -o {OUT}/o.parquet",
    ),
    ("wid-crm2", "403 MB", "info", "info {C}/zenodo4944335-35d-crm2-scan1-crr.wid --json"),
    (
        "fcs-facsdiscover",
        "18 MB",
        "info",
        "info {C}/zenodo19221995-facsdiscover-zam36-yfp.fcs --json",
    ),
    (
        "fcs-facsdiscover",
        "18 MB",
        "export parquet",
        "export {C}/zenodo19221995-facsdiscover-zam36-yfp.fcs --to parquet -o {OUT}/o.parquet",
    ),
]


def peak_rss_mib(stderr: str) -> float:
    """Peak RSS from `/usr/bin/time` output: bytes on macOS (-l), KiB on Linux (-v)."""
    m = re.search(r"(\d+)\s+maximum resident set size", stderr)
    if m:
        return int(m.group(1)) / 2**20
    m = re.search(r"Maximum resident set size \(kbytes\): (\d+)", stderr)
    return int(m.group(1)) / 2**10 if m else float("nan")


def run(binary: str, reps: int, only: str | None, out: Path) -> None:
    flag = "-l" if sys.platform == "darwin" else "-v"
    results: List[Dict[str, Any]] = []
    with tempfile.TemporaryDirectory() as tmp:
        outdir = Path(tmp) / "out"
        for case, size, op, args in CASES:
            if only and not re.search(only, f"{case} {op}"):
                continue
            argv = args.format(C=CORPUS, OUT=outdir).split()
            walls: List[float] = []
            rss: List[float] = []
            code = 0
            err = ""
            for i in range(reps + 1):
                shutil.rmtree(outdir, ignore_errors=True)
                outdir.mkdir()
                t = time.monotonic()
                p = subprocess.run(
                    ["/usr/bin/time", flag, binary, *argv],
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.PIPE,
                    text=True,
                    check=False,
                )
                wall = time.monotonic() - t
                code = p.returncode
                if i == 0:
                    continue  # warms the page cache
                walls.append(wall)
                rss.append(peak_rss_mib(p.stderr))
                if code != 0:
                    err = next((ln for ln in p.stderr.splitlines() if ln.strip()), "")
            r = {
                "case": case,
                "size": size,
                "op": op,
                "wall_s": statistics.median(walls),
                "peak_rss_mib": max(rss),
                "exit": code,
                "load": os.getloadavg()[0],
                "error": err,
            }
            results.append(r)
            print(
                f"{case:18} {op:30} {r['wall_s']:8.3f} s {r['peak_rss_mib']:8.1f} MiB "
                f"exit {code} load {r['load']:.1f}",
                flush=True,
            )
    out.write_text(json.dumps(results, indent=1) + "\n")


def report(before: Path, after: Path) -> None:
    b = {(r["case"], r["op"]): r for r in json.loads(before.read_text())}
    a = json.loads(after.read_text())

    def cell(r: Dict[str, Any]) -> str:
        if r["exit"] != 0:
            return f"refused; {r['peak_rss_mib']:.0f} MiB"
        w = r["wall_s"]
        t = f"{w:.2f} s" if w < 10 else f"{w:.0f} s"
        return f"{t}; {r['peak_rss_mib']:.0f} MiB"

    print("| file | size | operation | before | after |")
    print("| --- | --- | --- | --- | --- |")
    for r in a:
        old = b.get((r["case"], r["op"]))
        print(
            f"| {r['case']} | {r['size']} | `{r['op']}` | {cell(old) if old else '-'} | {cell(r)} |"
        )


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run")
    r.add_argument("binary")
    r.add_argument("--reps", type=int, default=3)
    r.add_argument("--only")
    r.add_argument("--out", type=Path, default=Path("large_files.json"))
    rep = sub.add_parser("report")
    rep.add_argument("before", type=Path)
    rep.add_argument("after", type=Path)
    a = ap.parse_args()
    if a.cmd == "run":
        run(a.binary, a.reps, a.only, a.out)
    else:
        report(a.before, a.after)


if __name__ == "__main__":
    main()
