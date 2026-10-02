#!/usr/bin/env python
"""Regenerate the synthetic mzMLb and gzip fixtures in corpus/files/.

Usage:  cd oracle && uv run python make_mzml_container_fixtures.py [--out DIR] [--oracle]

Sources: `mzdata-small.mzML` (mzdata test data, Apache-2.0; 48 LTQ-FT spectra and a TIC
chromatogram) and `pyteomics-test-mzxml.mzXML` (pyteomics test data, Apache-2.0; 2 scans),
written back by third-party code run as a black box:

* `synthetic-mzmlb-zlib.mzMLb`: psims' `MzMLbWriter` (Apache-2.0, h5py) with HDF5 deflate
  (level 4) on every dataset, the spectra and chromatogram of mzdata-small.
* `synthetic-mzmlb-blosc.mzMLb`: the same with the Blosc filter (32001, LZ4 codec, byte
  shuffle; hdf5plugin, MIT).
* `synthetic-mzmlb-numpress.mzMLb`: deflate, and MS-Numpress linear (m/z) / short logged float
  (intensity) arrays stored as byte datasets.
* `synthetic-mzml-gz-members.mzML.gz`: mzdata-small.mzML compressed by Python's `gzip` module as
  two concatenated members (split in the middle of a spectrum), as block-gzip tools write them.
* `synthetic-mzxml-gz.mzXML.gz`: pyteomics-test-mzxml.mzXML compressed by Python's `gzip` module
  (level 9).
* `synthetic-mzml-gz-truncated.mzML.gz` (role corrupt): mzdata-small.mzML gzip-compressed as one
  member and cut to its first 70 % (the file ends inside the deflate stream).

`--oracle` runs gen.py on the readable fixtures (pyteomics' `mzmlb.MzMLb` for mzMLb, pyteomics'
MzML/MzXML on a `gzip`-decompressed temporary copy for the .gz files).
"""
from __future__ import annotations

import argparse
import gzip
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))


def _mzmlb(dst: Path, compression: str, arrays: tuple[str, str, str]) -> None:
    """Write mzdata-small's spectra with psims' mzMLb writer (reusing make_mzml_fixtures'
    document layout: `_write` opens `IndexedMzMLWriter`, which is swapped for `MzMLbWriter`)."""
    import psims.mzml.writer as w
    from psims.mzmlb.writer import MzMLbWriter

    import make_mzml_fixtures as mk

    def factory(fh, close=True):
        fh.close()
        return MzMLbWriter(str(dst), close=True, h5_compression=compression)

    saved = w.IndexedMzMLWriter
    w.IndexedMzMLWriter = factory
    try:
        spectra, chroms = mk._spectra(OUT / mk.SOURCE)
        mk._write(dst, spectra, chroms, *arrays)
    finally:
        w.IndexedMzMLWriter = saved


OUT: Path | None = None


def main() -> None:
    global OUT
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=str(ROOT / "corpus" / "files"))
    ap.add_argument("--oracle", action="store_true")
    a = ap.parse_args()
    OUT = Path(a.out)
    src = OUT / "mzdata-small.mzML"
    mzxml = OUT / "pyteomics-test-mzxml.mzXML"
    for p in (src, mzxml):
        if not p.exists():
            sys.exit(f"{p} missing: cargo xtask corpus fetch")

    _mzmlb(OUT / "synthetic-mzmlb-zlib.mzMLb", "gzip", ("zlib", "zlib", "zlib"))
    _mzmlb(OUT / "synthetic-mzmlb-blosc.mzMLb", "blosc:lz4", ("zlib", "zlib", "zlib"))
    _mzmlb(OUT / "synthetic-mzmlb-numpress.mzMLb", "gzip",
           ("MS-Numpress linear prediction compression",
            "MS-Numpress short logged float compression",
            "MS-Numpress positive integer compression"))

    raw = src.read_bytes()
    cut = raw.index(b"<spectrum ", len(raw) // 2) + 40  # inside a spectrum element
    (OUT / "synthetic-mzml-gz-members.mzML.gz").write_bytes(
        gzip.compress(raw[:cut], 6, mtime=0) + gzip.compress(raw[cut:], 6, mtime=0))
    (OUT / "synthetic-mzxml-gz.mzXML.gz").write_bytes(gzip.compress(mzxml.read_bytes(), 9, mtime=0))
    (OUT / "synthetic-mzml-gz-truncated.mzML.gz").write_bytes(
        gzip.compress(raw, 6, mtime=0)[: len(gzip.compress(raw, 6, mtime=0)) * 7 // 10])
    for p in sorted(list(OUT.glob("synthetic-mzmlb-*.mzMLb")) + list(OUT.glob("synthetic-mz*-gz*.gz"))):
        print("wrote", p.name, p.stat().st_size)

    if a.oracle:
        gen = HERE / "gen.py"
        for name, fid in (
            ("synthetic-mzmlb-zlib.mzMLb", "synthetic-mzmlb-zlib"),
            ("synthetic-mzmlb-blosc.mzMLb", "synthetic-mzmlb-blosc"),
            ("synthetic-mzmlb-numpress.mzMLb", "synthetic-mzmlb-numpress"),
            ("synthetic-mzml-gz-members.mzML.gz", "synthetic-mzml-gz-members"),
            ("synthetic-mzxml-gz.mzXML.gz", "synthetic-mzxml-gz"),
        ):
            # psims' writer keeps its HDF5 handles until this process exits: do not lock
            env = {**os.environ, "HDF5_USE_FILE_LOCKING": "FALSE"}
            subprocess.run([sys.executable, str(gen), "--id", fid, str(OUT / name)], check=True, env=env)


if __name__ == "__main__":
    main()
