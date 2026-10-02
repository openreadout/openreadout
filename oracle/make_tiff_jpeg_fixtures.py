#!/usr/bin/env python
"""Regenerate the JPEG-in-TIFF colour fixtures (ids `synthetic-tiff-jpeg-*`) in corpus/files/.

Usage:  cd oracle && uv run python make_tiff_jpeg_fixtures.py [--out DIR] [--oracle]
        (Bio-Formats: BFTOOLS_DIR, oracle/bftools/bftools, or the main checkout's copy, as in gen.py;
         the Bio-Formats 6.7.0 fixtures need BFTOOLS_670_DIR = an unzipped bftools.zip of 6.7.0 from
         https://downloads.openmicroscopy.org/bio-formats/6.7.0/artifacts/bftools.zip)

Why these files exist (docs/provenance/tiff.md, 2026-09-24 "JPEG colour"): Bio-Formats 6.x writes
JPEG-compressed RGB tiles as JFIF streams, i.e. Y, Cb, Cr components with 2x2 chroma subsampling,
while tagging the page photometric RGB. Bio-Formats 8.x tags the same streams photometric YCbCr.
A decoder that takes photometric RGB to mean "the components are R, G, B" returns Y, Cb, Cr as
colours (mean ~160 instead of ~220 on this tissue image). The fixtures pin both writers, the
separate-planes variants, and tifffile-written pages with and without JFIF.

Source pixels: a 1300 x 1000 crop of level 0 of OpenSlide's CMU-1-Small-Region.svs (CC0-1.0,
corpus id `openslide-aperio-cmu-1-small-region`), decoded by tifffile + imagecodecs. Bio-Formats
(GPL) is run as a black box on a PNG (interleaved RGB in -> interleaved JPEG out) and on an
uncompressed tifffile TIFF (-> one JPEG stream per sample plane). Bio-Formats writes fresh UUIDs,
so the manifest has no sha256 for these files; the oracle JSON (tifffile + imagecodecs, BSD) pins
the pixels.

`--oracle` also runs gen.py on every fixture.
"""
from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path

import imagecodecs
import numpy as np
import tifffile

ROOT = Path(__file__).resolve().parents[1]
SOURCE = "openslide-aperio-CMU-1-Small-Region.svs"


def _bftools(tool: str, env: str | None = None) -> str:
    here = Path(__file__).resolve().parent
    cands = []
    if env and os.environ.get(env):
        cands.append(Path(os.environ[env]))
    if env is None:
        if os.environ.get("BFTOOLS_DIR"):
            cands.append(Path(os.environ["BFTOOLS_DIR"]))
        cands.append(here / "bftools" / "bftools")
        for parent in here.parents:
            if parent.name == "worktrees" and parent.parent.name == ".claude":
                cands.append(parent.parent.parent / "oracle" / "bftools" / "bftools")
    for c in cands:
        if (c / tool).exists():
            return str(c / tool)
    raise SystemExit(f"bftools not found for {env or 'BFTOOLS_DIR'}")


def _bfconvert(tool: str, src: Path, dst: Path, *args: str) -> None:
    if dst.exists():
        dst.unlink()
    r = subprocess.run([tool, "-no-upgrade", "-overwrite", "-compression", "JPEG", *args, str(src), str(dst)],
                       capture_output=True, text=True)
    if r.returncode != 0 or not dst.exists():
        raise SystemExit(f"bfconvert failed for {dst.name}: {r.stdout[-500:]} {r.stderr[-500:]}")


PYRAMID = ["-tilex", "256", "-tiley", "256", "-pyramid-resolutions", "3", "-pyramid-scale", "2"]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=str(ROOT / "corpus" / "files"))
    ap.add_argument("--oracle", action="store_true")
    a = ap.parse_args()
    out = Path(a.out)
    src = out / SOURCE
    if not src.exists():
        sys.exit(f"{src} missing: cargo xtask corpus fetch --only openslide-aperio-cmu-1-small-region")
    rgb = np.ascontiguousarray(tifffile.imread(src)[:1000, :1300])
    work = out / ".synthetic-tiff-jpeg-work"
    work.mkdir(exist_ok=True)
    png = work / "source.png"
    png.write_bytes(imagecodecs.png_encode(rgb))
    flat = work / "source.tif"
    tifffile.imwrite(flat, rgb, photometric="rgb")

    made = []
    bf670 = _bftools("bfconvert", "BFTOOLS_670_DIR")
    bf8 = _bftools("bfconvert")
    for tool, tag in ((bf670, "bf670"), (bf8, "bf850")):
        # interleaved RGB in -> one 3-component JFIF stream per tile (6.7.0: photometric RGB, 8.x: YCbCr)
        p = out / f"synthetic-tiff-jpeg-{tag}-rgb-tiled-pyramid.ome.tif"
        _bfconvert(tool, png, p, *PYRAMID)
        made.append(p)
        p = out / f"synthetic-tiff-jpeg-{tag}-rgb-strips.ome.tif"
        _bfconvert(tool, png, p)
        made.append(p)
        # uncompressed TIFF in -> one 1-component stream per sample plane (6.7.0: RGB, planar 2;
        # 8.x: YCbCr, planar 2, which neither tifffile nor we decode)
        p = out / f"synthetic-tiff-jpeg-{tag}-planar-tiled.ome.tif"
        _bfconvert(tool, flat, p, "-tilex", "256", "-tiley", "256")
        made.append(p)

    # tifffile + imagecodecs (libjpeg-turbo): its default for RGB input is photometric YCbCr with
    # 2x2-subsampled JFIF streams; `outcolorspace="RGB"` writes photometric RGB streams coded as
    # R, G, B (Adobe transform 0, component ids 'R','G','B'; no JFIF).
    p = out / "synthetic-tiff-jpeg-tifffile-ycbcr.tif"
    tifffile.imwrite(p, rgb, photometric="rgb", tile=(256, 256), compression="jpeg",
                     compressionargs={"level": 90})
    made.append(p)
    p = out / "synthetic-tiff-jpeg-tifffile-rgb-ascoded.tif"
    tifffile.imwrite(p, rgb, photometric="rgb", tile=(256, 256), compression="jpeg",
                     compressionargs={"level": 90, "outcolorspace": "RGB"})
    made.append(p)
    for p in made:
        print(p.name, p.stat().st_size)
    if a.oracle:
        env = {**os.environ, "OPENREADOUT_CORPUS_DIR": str(out)}
        subprocess.run([sys.executable, str(ROOT / "oracle" / "gen.py"), *map(str, made)], check=True, env=env)


if __name__ == "__main__":
    main()
