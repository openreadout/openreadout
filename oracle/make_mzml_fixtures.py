#!/usr/bin/env python
"""Regenerate the synthetic mzML fixtures in corpus/files/ (ids start with `synthetic-mzml-`).

Usage:  cd oracle && uv run python make_mzml_fixtures.py [--out DIR] [--oracle]

The source is `mzdata-small.mzML` (mzdata test data, Apache-2.0; 48 LTQ-FT spectra, MS1 profile
and MS2 centroid, one TIC chromatogram), read with pyteomics and written back with psims
(Apache-2.0), both run as black boxes:

* `synthetic-mzml-numpress`: indexed mzML; m/z arrays `MS-Numpress linear prediction
  compression`, intensity arrays `MS-Numpress short logged float compression`, chromatogram
  intensity `MS-Numpress positive integer compression` (psims encodes with pynumpress, the
  Python binding of the reference MSNumpress implementation). pyteomics decodes it with the
  same library for the oracle.
* `synthetic-mzml-zstd`: the same spectra with `zstd compression` (MS:1003780) arrays. pyteomics
  cannot decode zstd, so the oracle is pyteomics' reading of the zlib twin written in the same
  run with identical metadata (`synthetic-mzml-zstd-twin.mzML`, kept next to it), re-labelled;
  zstd and zlib are both lossless.
* `synthetic-mzml-plain`: the `<mzML>` element of mzdata-small cut out of its `<indexedmzML>`
  wrapper byte for byte (no offset index: exercises the scanning fallback).
* `synthetic-mzml-stale-index` (role corrupt): mzdata-small with an XML comment inserted after
  the header, so every index offset is off by the comment's length and the SHA-1 no longer
  matches. Readers must fall back to scanning; `check` must report the stale index.
* `synthetic-mzml-truncated` (role corrupt): the first 60 % of mzdata-small's bytes.

`--oracle` runs gen.py on the three readable fixtures afterwards.
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
SOURCE = "mzdata-small.mzML"


def _spectra(src: Path):
    from pyteomics import mzml
    with mzml.MzML(str(src), decode_binary=True) as f:
        spectra = list(f)
    with mzml.MzML(str(src), decode_binary=True) as f:
        chroms = list(f.iterfind("chromatogram"))
    return spectra, chroms


def _write(dst: Path, spectra, chroms, compression_mz, compression_int, compression_chrom, indexed=True):
    from psims.mzml.writer import IndexedMzMLWriter, PlainMzMLWriter
    cls = IndexedMzMLWriter if indexed else PlainMzMLWriter
    enc = {"m/z array": np.float64, "intensity array": np.float64, "time array": np.float64}
    with cls(open(dst, "wb"), close=True) as out:
        out.controlled_vocabularies()
        out.file_description(["MS1 spectrum", "MSn spectrum"])
        out.software_list([{"id": "psims", "version": "1", "params": ["python-psims"]}])
        source = out.Source(1, ["electrospray ionization"])
        analyzer = out.Analyzer(2, ["fourier transform ion cyclotron resonance mass spectrometer"])
        detector = out.Detector(3, ["inductive detector"])
        out.instrument_configuration_list([
            out.InstrumentConfiguration(id="IC1", component_list=[source, analyzer, detector], params=["LTQ FT"])
        ])
        out.data_processing_list([
            out.DataProcessing([out.ProcessingMethod(order=1, software_reference="psims", params=["Conversion to mzML"])], id="DP1")
        ])
        with out.run(id="synthetic", instrument_configuration="IC1"):
            with out.spectrum_list(count=len(spectra)):
                for sp in spectra:
                    scan = sp["scanList"]["scan"][0]
                    rt = float(scan["scan start time"])  # minutes in the source
                    params = [{"ms level": int(sp["ms level"])}]
                    if "total ion current" in sp:
                        params.append({"total ion current": float(sp["total ion current"])})
                    prec = None
                    pl = sp.get("precursorList", {}).get("precursor", [])
                    if pl:
                        si = pl[-1]["selectedIonList"]["selectedIon"][0]
                        prec = {
                            "mz": float(si["selected ion m/z"]),
                            "intensity": float(si.get("peak intensity", 0.0)),
                            "charge": int(si["charge state"]) if "charge state" in si else None,
                            "activation": ["collision-induced dissociation", {"collision energy": 35.0}],
                        }
                    out.write_spectrum(
                        sp["m/z array"], sp["intensity array"], id=sp["id"],
                        polarity="positive scan" if "positive scan" in sp else "negative scan",
                        centroided="centroid spectrum" in sp,
                        scan_start_time=rt,
                        params=params,
                        precursor_information=prec,
                        encoding=enc,
                        compression={"m/z array": compression_mz, "intensity array": compression_int},
                    )
            with out.chromatogram_list(count=len(chroms)):
                for c in chroms:
                    out.write_chromatogram(
                        c["time array"], np.rint(c["intensity array"]), id=c["id"],
                        chromatogram_type="total ion current chromatogram",
                        encoding=enc,
                        compression={"time array": "zlib", "intensity array": compression_chrom},
                    )


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=str(ROOT / "corpus" / "files"))
    ap.add_argument("--oracle", action="store_true")
    a = ap.parse_args()
    out = Path(a.out)
    src = out / SOURCE
    if not src.exists():
        sys.exit(f"{src} missing: cargo xtask corpus fetch --only mzdata-small")
    spectra, chroms = _spectra(src)

    npz = out / "synthetic-mzml-numpress.mzML"
    _write(npz, spectra, chroms,
           "MS-Numpress linear prediction compression",
           "MS-Numpress short logged float compression",
           "MS-Numpress positive integer compression")
    zst = out / "synthetic-mzml-zstd.mzML"
    twin = out / "synthetic-mzml-zstd-twin.mzML"
    _write(zst, spectra, chroms, "zstd", "zstd", "zstd")
    _write(twin, spectra, chroms, "zlib", "zlib", "zlib")

    raw = src.read_bytes()
    a0 = raw.index(b"<mzML")
    a1 = raw.index(b"</mzML>") + len(b"</mzML>")
    (out / "synthetic-mzml-plain.mzML").write_bytes(b'<?xml version="1.0" encoding="utf-8"?>\n' + raw[a0:a1] + b"\n")

    cut = raw.index(b"<run")
    (out / "synthetic-mzml-stale-index.mzML").write_bytes(raw[:cut] + b"<!-- inserted after the index was written -->\n" + raw[cut:])
    (out / "synthetic-mzml-truncated.mzML").write_bytes(raw[: len(raw) * 6 // 10])
    for p in sorted(out.glob("synthetic-mzml-*.mzML")):
        print("wrote", p.name, p.stat().st_size)

    if a.oracle:
        gen = Path(__file__).resolve().parent / "gen.py"
        for name in ("synthetic-mzml-numpress.mzML", "synthetic-mzml-plain.mzML", "synthetic-mzml-zstd-twin.mzML"):
            subprocess.run([sys.executable, str(gen), str(out / name)], check=True)
        # the zstd fixture's ground truth is its zlib twin's
        od = ROOT / "corpus" / "oracle"
        t = json.loads((od / "synthetic-mzml-zstd-twin.json").read_text())
        t["id"] = "synthetic-mzml-zstd"
        t["file"] = zst.name
        t["size"] = zst.stat().st_size
        t.pop("sha256", None)
        t["reader"] += " on the zlib twin synthetic-mzml-zstd-twin.mzML (pyteomics cannot decode zstd)"
        (od / "synthetic-mzml-zstd.json").write_text(json.dumps(t, separators=(",", ":")) + "\n")
        (od / "synthetic-mzml-zstd-twin.json").unlink()


if __name__ == "__main__":
    main()
