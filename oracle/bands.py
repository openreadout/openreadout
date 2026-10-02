"""Spectral band and region oracles: `corpus/oracle/bands/<id>.json` for the corpus test
`crates/openreadout-corpus-tests/tests/bands.rs` (`openreadout analyze peaks --x-range` on spectra).

Every spectrum is read with a third-party reader run as a black box, never with OpenReadout:
brukeropus (MIT) for Bruker OPUS, SpectroChemPy (CeCILL-B) for Thermo OMNIC, renishawWiRE (MIT)
for Renishaw WiRE, specio (BSD-3) for PerkinElmer .sp, jcamp (MIT) for JCAMP-DX and nmrglue
(BSD-3) for Bruker processed NMR spectra. Where a dataset ships the vendor's own export of the
same spectrum (OMNIC CSV), the areas are also computed from that export, a second independent
source.

Regions are integrated with NumPy exactly as band areas are usually scripted: the samples with
lo <= x <= hi (x ascending), `numpy.trapezoid(y - b, x)` with b the straight line through the
first and last of them (`linear`) or 0 (`none`); the maximum is the sample of largest y - b
(smallest for transmittance/reflectance, whose bands are minima) and the centroid the
(y - b)-weighted mean x. Bands: the most prominent maxima of the raw signal by
`scipy.signal.find_peaks` (prominence), which OpenReadout's detector must find within
1.5 samples.

Vendor-computed values: TopSpin's integrals of the regions in `pdata/1/intrng` (integrals.txt,
bias and slope 0, normalised to region 1) for the Bruker NMR spectrum.

    oracle/.venv/bin/python oracle/bands.py [--check]
"""

from __future__ import annotations

import argparse
import collections
import collections.abc
import contextlib
import io
import json
import os
import re
import sys
import warnings
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "corpus" / "oracle" / "bands"
CORPUS = Path(os.environ.get("OPENREADOUT_CORPUS_DIR", ROOT / "corpus" / "files"))


def manifest_file(cid: str, role: str = "input") -> Path:
    text = (ROOT / "corpus" / "manifest.toml").read_text(encoding="utf-8")
    for block in text.split("[[file]]"):
        if re.search(r'^id = "' + re.escape(cid) + r'"$', block, re.M) and re.search(
            r'^role = "' + role + '"$', block, re.M
        ):
            m = re.search(r'^filename = "([^"]+)"$', block, re.M)
            if m:
                return CORPUS / m.group(1)
    raise SystemExit(f"{cid} ({role}): not in corpus/manifest.toml")


def version(mod: str) -> str:
    from importlib.metadata import version as v

    try:
        return f"{mod} {v(mod)}"
    except Exception:
        return mod


# ------------------------------------------------------------------ readers (black boxes)


def quiet():
    warnings.simplefilter("ignore")
    return contextlib.ExitStack()


def read_opus(p: Path, key: str):
    from brukeropus import OPUSFile

    warnings.simplefilter("ignore")
    d = getattr(OPUSFile(str(p)), key)
    return np.asarray(d.x, float), np.asarray(d.y, float), version("brukeropus") + f" (block {key})"


def read_omnic(p: Path):
    import spectrochempy as scp

    warnings.simplefilter("ignore")
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        d = scp.read_omnic(str(p))
    return np.asarray(d.x.data, float), np.asarray(d.data[0], float), version("spectrochempy") + " read_omnic"


def read_export(p: Path):
    """A vendor text export: two numeric columns (comma, tab or space separated)."""
    rows = []
    for line in p.read_text(errors="replace").splitlines():
        parts = [q for q in re.split(r"[,\t ;]+", line.strip()) if q]
        if len(parts) >= 2:
            try:
                rows.append((float(parts[0]), float(parts[1])))
            except ValueError:
                continue
    a = np.asarray(rows)
    return a[:, 0], a[:, 1], "the vendor's export " + p.name


def read_wdf(p: Path):
    from renishawWiRE import WDFReader

    warnings.simplefilter("ignore")
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        r = WDFReader(str(p))
    y = np.asarray(r.spectra, float).reshape(-1, r.point_per_spectrum)[0]
    return np.asarray(r.xdata, float), y, version("renishawWiRE")


def read_pesp(p: Path):
    for n in ("Iterable", "Mapping", "Sequence", "MutableMapping"):
        if not hasattr(collections, n):
            setattr(collections, n, getattr(collections.abc, n))
    from specio import specread

    s = specread(str(p))
    return np.asarray(s.wavelength, float), np.asarray(s.amplitudes, float).ravel(), version("specio")


def read_jcamp(p: Path):
    import jcamp

    read = jcamp.jcamp_readfile if hasattr(jcamp, "jcamp_readfile") else jcamp.readfile
    d = read(str(p))
    return np.asarray(d["x"], float), np.asarray(d["y"], float), version("jcamp")


def read_bruker_pdata(p: Path):
    import nmrglue as ng

    warnings.simplefilter("ignore")
    dic, data = ng.bruker.read_pdata(str(p / "pdata" / "1"), scale_data=True)
    udic = ng.bruker.guess_udic(dic, data)
    uc = ng.fileiobase.uc_from_udic(udic)
    return np.asarray(uc.ppm_scale(), float), np.asarray(data, float), version("nmrglue") + " read_pdata"


# ------------------------------------------------------------------ reference arithmetic


def ascending(x, y):
    o = np.argsort(x, kind="stable")
    x, y = x[o], y[o]
    m = np.isfinite(x) & np.isfinite(y)
    return x[m], y[m]


def region(x, y, lo, hi, baseline: str, minima: bool) -> dict:
    m = (x >= lo) & (x <= hi)
    xs, ys = x[m], y[m]
    b = np.interp(xs, [xs[0], xs[-1]], [ys[0], ys[-1]]) if baseline == "linear" else np.zeros_like(ys)
    d = ys - b
    sign = -1.0 if minima else 1.0
    k = int(np.argmax(sign * d))
    w = float(np.trapezoid(sign * d, xs))
    out = {
        "points": int(m.sum()),
        "x_first": float(xs[0]),
        "x_last": float(xs[-1]),
        "area": float(np.trapezoid(d, xs)),
        "area_no_baseline": float(np.trapezoid(ys, xs)),
        "max_x": float(xs[k]),
        "max_height": float(sign * d[k]),
    }
    if w > 0:
        out["centroid"] = float(np.trapezoid(xs * sign * d, xs) / w)
    return out


def prominent(x, y, minima: bool, n: int = 5) -> list[float]:
    from scipy.signal import find_peaks

    s = -y if minima else y
    idx, props = find_peaks(s, prominence=0)
    order = np.argsort(-props["prominences"])[:n]
    return sorted(float(x[idx[i]]) for i in order)


# ------------------------------------------------------------------ cases

OPUS_BITUMEN = [[980, 1060], [1350, 1525], [1650, 1800], [2800, 3000]]

CASES = [
    # id, reader, trace, regions, minima, export id (second source) or None
    ("opus-bitumen-unaged-2", lambda p: read_opus(p, "a"), 0, OPUS_BITUMEN, False, None),
    ("opus-bitumen-1h-180c-2", lambda p: read_opus(p, "a"), 0, OPUS_BITUMEN, False, None),
    ("opus-bitumen-5h-120c-3", lambda p: read_opus(p, "a"), 0, OPUS_BITUMEN, False, None),
    ("opus-orange-peach-juice", lambda p: read_opus(p, "r"), 0, [[950, 1150], [1550, 1700]], True, None),
    ("omnic-toffolo-atr-paraffin", read_omnic, 0, [[2800, 3000], [1400, 1500], [700, 740]], False, "omnic-toffolo-atr-paraffin-csv"),
    (
        "omnic-toffolo-atr-calcite-spar-brazil",
        read_omnic,
        0,
        [[1300, 1550], [860, 890], [700, 720]],
        False,
        "omnic-toffolo-atr-calcite-spar-brazil-csv",
    ),
    (
        "omnic-toffolo-atr-quartz-alpha-synthetic-nist",
        read_omnic,
        0,
        [[1000, 1250], [760, 820]],
        False,
        "omnic-toffolo-atr-quartz-alpha-synthetic-nist-csv",
    ),
    (
        "omnic-toffolo-atr-cellulose-thermo-scientific",
        read_omnic,
        0,
        [[950, 1200], [3000, 3600]],
        False,
        "omnic-toffolo-atr-cellulose-thermo-scientific-csv",
    ),
    ("omnic-toffolo-atr-bone-modern", read_omnic, 0, [[900, 1200], [1350, 1500]], False, "omnic-toffolo-atr-bone-modern-csv"),
    ("wdf-pywdf-sp", read_wdf, 0, [[1300, 1400], [1550, 1650]], False, None),
    ("pesp-orange-single", read_pesp, 0, [[1000, 1100], [2800, 3000]], True, None),
    ("jcamp-lancashire-dupinc1", read_jcamp, 0, [[300, 350], [380, 420]], False, None),
    ("jcamp-isas-specfile", read_jcamp, 0, [[2800, 3000], [1400, 1500]], True, None),
    ("nmrxiv-s846-50", read_bruker_pdata, 1, [], False, None),
]


def topspin_regions(cid: str) -> list[dict]:
    """TopSpin's integration regions (intrng, ppm) and integrals (integrals.txt)."""
    base = manifest_file(cid) / "pdata" / "1"
    bounds = []
    for line in (base / "intrng").read_text().splitlines():
        m = re.match(r"\s*([-\d.eE+]+)\s+([-\d.eE+]+)\s+([-\d.eE+]+)\s+([-\d.eE+]+)", line)
        if m:
            bounds.append((float(m.group(1)), float(m.group(2)), float(m.group(3)), float(m.group(4))))
    values = []
    for line in (base / "integrals.txt").read_text().splitlines():
        m = re.match(r"\s*(\d+)\s+([-\d.]+)\s+([-\d.]+)\s+([-\d.eE+]+)\s*$", line)
        if m:
            values.append(float(m.group(4)))
    assert len(bounds) == len(values), (bounds, values)
    return [
        {"from": min(a, b), "to": max(a, b), "bias": bias, "slope": slope, "integral": v}
        for (a, b, bias, slope), v in zip(bounds, values, strict=True)
    ]


# Where a reader's axis is not the file's to full precision, the comparison allows for it.
TOLERANCE = {
    read_omnic: {
        "x": 6e-4,
        "area_rel": 5e-5,
        "why": "SpectroChemPy rounds the wavenumber axis to 3 decimals (steps of 0.48 cm-1)",
    },
    read_bruker_pdata: {
        "x": 2e-6,
        "area_rel": 1e-6,
        "why": "nmrglue's ppm scale sits 8e-7 ppm (0.3 % of a step) from OFFSET - i*SW_p/SF/SI",
    },
}


def build(cid, reader, trace, regions, minima, export) -> dict:
    p = manifest_file(cid)
    x, y, src = reader(p)
    x, y = ascending(x, y)
    doc = {
        "id": cid,
        "trace": trace,
        "reader": src,
        "tolerance": TOLERANCE.get(reader, {"x": 1e-9, "area_rel": 1e-6, "why": "same samples"}),
        "points": len(x),
        "bands_are_minima": minima,
        "prominent_bands": prominent(x, y, minima),
        "median_step": float(np.median(np.diff(x))),
        "regions": [],
    }
    vendor = []
    if cid == "nmrxiv-s846-50":
        vendor = topspin_regions(cid)
        regions = [[v["from"], v["to"]] for v in vendor]
        doc["vendor"] = {
            "source": "TopSpin 3.5 pdata/1/integrals.txt (vendor-computed; bias 0, slope 0; normalised to region 1 = 2)",
            "integrals": [v["integral"] for v in vendor],
        }
    for lo, hi in regions:
        r = {"range": [lo, hi]}
        for base in ("linear", "none"):
            r[base] = region(x, y, lo, hi, base, minima)
        doc["regions"].append(r)
    if export:
        ex, ey, esrc = read_export(manifest_file(export, "oracle-export"))
        ex, ey = ascending(ex, ey)
        doc["export"] = {
            "source": esrc,
            "regions": [{"range": [lo, hi], "linear": region(ex, ey, lo, hi, "linear", minima)} for lo, hi in regions],
        }
    return doc


def render(doc: dict) -> str:
    return json.dumps(doc, indent=1) + "\n"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true", help="exit 1 if a committed oracle differs")
    args = ap.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    bad = 0
    for case in CASES:
        doc = build(*case)
        path = OUT / f"{case[0]}.json"
        text = render(doc)
        if args.check:
            if not path.exists() or path.read_text() != text:
                print(f"DIFFERS {path.relative_to(ROOT)}")
                bad += 1
            continue
        path.write_text(text)
        print(f"wrote {path.relative_to(ROOT)}: {len(doc['regions'])} regions, {doc['points']} points")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
