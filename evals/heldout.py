"""Held-out questions: files from sources no OpenReadout reader was developed on.

The `dev` and `test` questions ask about the corpus the readers were built against, so they cannot
show whether the readers generalize. These questions ask about the held-out corpus instead
(`tier = "heldout"` in corpus/manifest.toml; policy in docs/benchmark/heldout.md): other depositors,
instrument models, software versions and years. Every question gets `split = "heldout"`, lands in
`questions/heldout.jsonl`, and runs only with `run.py --split heldout`.

Answers come from three places, never from OpenReadout:
  - `oracle:` the held-out ground truth `corpus/oracle/heldout/<id>.json`, written by oracle/gen.py
    with the same third-party readers as the rest of the corpus;
  - `facts-heldout:` `evals/facts/heldout.json`, which this module computes in the oracle venv
    (analysis-tier values from the data with third-party readers, cross-checked by a second reader
    where one exists; and values the depositor states in the repository record or in an export);
  - `manifest:` a fact recorded in corpus/manifest.toml (the format of a file).

    oracle/.venv/bin/python evals/heldout.py           # recompute evals/facts/heldout.json (needs the held-out files)
    oracle/.venv/bin/python evals/heldout.py --check   # exit 1 if the committed facts drift from the files
    cd evals && uv run python generate.py              # then regenerate the questions (heldout.jsonl included)

Like analysis.py, this module imports only the standard library and `generate` at the top (the
readers are imported inside the functions that use them), so generate.py runs without them.
"""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
import generate as g

ROOT = Path(__file__).resolve().parent.parent
ORACLE_DIR = ROOT / "corpus" / "oracle" / "heldout"
OUT = Path(__file__).resolve().parent / "facts" / "heldout.json"
SPLIT = "heldout"


# ---------------------------------------------------------------- facts (oracle venv only)


@dataclass
class Fact:
    corpus_id: str
    name: str
    how: str
    compute: Callable[[], tuple[Any, str]]
    cross: Callable[[], tuple[Any, str]] | None = None
    role: str = "heldout"
    rel: float = 1e-5  # agreement required between the two readers


def hpath(corpus_id: str, role: str = "heldout") -> Path:
    """A held-out file on disk (`role` "input" is read as "heldout", so analysis.py's readers work)."""
    import facts as fx

    p = fx.corpus_dir() / fx.manifest_file(corpus_id, "heldout" if role == "input" else role)
    if not p.exists():
        raise SystemExit(f"{corpus_id}: {p} is missing; fetch it first (cargo xtask corpus fetch --tier heldout)")
    return p


def rd(*packages: str) -> str:
    from importlib.metadata import version

    return " + ".join(f"{p} {version(p)}" for p in packages)


# microscopy


def nd2_data(cid: str):
    import nd2

    with nd2.ND2File(hpath(cid)) as h:
        return h.asarray(), list(h.sizes), [c.channel.name for c in (h.metadata.channels or [])]


def nd2_rgb_mean(cid: str, sample: int):
    a, axes, _ = nd2_data(cid)
    assert axes == ["Y", "X", "S"], axes
    return a[..., sample].mean(dtype="float64"), rd("nd2")


def ometiff_rgb_mean(cid: str, sample: int):
    import tifffile

    with tifffile.TiffFile(hpath(cid, "oracle-export")) as t:
        s = t.series[0]
        a = s.asarray()
        assert s.axes == "YXS", s.axes
    return a[..., sample].mean(dtype="float64"), rd("tifffile") + " on the depositor's OME-TIFF export"


def nd2_brightest_channel(cid: str):
    a, axes, names = nd2_data(cid)
    assert axes == ["C", "Y", "X"], axes
    means = a.reshape(a.shape[0], -1).mean(axis=1, dtype="float64")
    return names[int(means.argmax())], rd("nd2")


def nd2_brightest_z(cid: str, channel: int):
    a, axes, _ = nd2_data(cid)
    assert axes == ["Z", "C", "Y", "X"], axes
    means = a[:, channel].reshape(a.shape[0], -1).mean(axis=1, dtype="float64")
    return int(means.argmax()) + 1, rd("nd2")


def czi_brightest_t(cid: str):
    import czifile

    with czifile.CziFile(hpath(cid)) as c:
        s = c.scenes[0]
        a, dims = s.asarray(), tuple(s.dims)
    assert dims == ("T", "Z", "X"), dims
    means = a.reshape(a.shape[0], -1).mean(axis=1, dtype="float64")
    return int(means.argmax()) + 1, rd("czifile")


def lif_frame_means(cid: str):
    import liffile

    with liffile.LifFile(hpath(cid)) as f:
        im = f.images[0]
        assert tuple(im.dims) == ("T", "Y", "X"), im.dims
        a = im.asarray()
    return a.reshape(a.shape[0], -1).mean(axis=1, dtype="float64")


def lif_dimmest_t(cid: str):
    m = lif_frame_means(cid)
    return int(m.argmin()) + 1, rd("liffile")


def lif_dimmest_t_cross(cid: str):
    import numpy as np
    from readlif.reader import LifFile

    im = LifFile(str(hpath(cid))).get_image(0)
    m = [np.asarray(im.get_frame(z=0, t=t, c=0)).mean(dtype="float64") for t in range(im.dims.t)]
    return int(np.argmin(m)) + 1, rd("readlif") + " (black box)"


def lif_brightest_series(cid: str, channel: int, cross: bool = False):
    import analysis as a

    means, reader = (a.lif_series_means_cross if cross else a.lif_series_means)(cid, channel)
    return max(means, key=means.get), reader


# electron microscopy


def mrc_max(cid: str, numpy: bool = False):
    import analysis as a

    d = (a.mrc_numpy if numpy else a.mrc_data)(cid)
    return float(d.max()), a.NUMPY_MRC if numpy else rd("mrcfile")


def mrc_mean(cid: str, numpy: bool = False):
    import analysis as a

    d = (a.mrc_numpy if numpy else a.mrc_data)(cid)
    return d.mean(dtype="float64"), a.NUMPY_MRC if numpy else rd("mrcfile")


# flow cytometry


def fcs_raw_median(cid: str, param: str, fcsparser: bool = False):
    """Median of one parameter's values as stored (no $PnE/$PnG scaling), flowio or fcsparser."""
    import numpy as np

    if fcsparser:
        import analysis as a

        arr, names, _ = a.fcs_events(cid, fcsparser=True)
        return float(np.median(arr[:, names.index(param)])), rd("fcsparser")
    import flowio

    f = flowio.FlowData(str(hpath(cid)))
    names = [f.text[f"p{i}n"] for i in range(1, f.channel_count + 1)]
    arr = np.asarray(f.as_array(preprocess=False), dtype="float64")
    return float(np.median(arr[:, names.index(param)])), rd("flowio")


# electrophysiology


def abf_most_negative_sweep(cid: str, neo: bool = False):
    import analysis as a

    sw, _ = (a.abf_sweeps_neo if neo else a.abf_sweeps)(cid, 0)
    mins = [float(y.min()) for y in sw]
    return mins.index(min(mins)) + 1, rd("neo") + " AxonRawIO" if neo else rd("pyabf")


def abf_min(cid: str, neo: bool = False):
    import numpy as np

    import analysis as a

    sw, _ = (a.abf_sweeps_neo if neo else a.abf_sweeps)(cid, 0)
    return float(np.concatenate(sw).min()), rd("neo") + " AxonRawIO" if neo else rd("pyabf")


# mass spectrometry


def ms_spectra(cid: str, role: str = "heldout") -> list[dict]:
    """Every spectrum of an mzML/mzXML (pyteomics): level, time in minutes, precursor, arrays."""
    import numpy as np
    from pyteomics import mzml, mzxml

    p = hpath(cid, role)
    out = []
    if p.suffix.lower() == ".mzxml":
        with mzxml.MzXML(str(p)) as r:
            for s in r:
                prec = s.get("precursorMz") or [{}]
                out.append(
                    {
                        "level": int(s["msLevel"]),
                        "rt_min": float(s["retentionTime"]),  # pyteomics gives minutes
                        "precursor": float(prec[0]["precursorMz"]) if "precursorMz" in prec[0] else None,
                        "mz": np.asarray(s["m/z array"], dtype="float64"),
                        "i": np.asarray(s["intensity array"], dtype="float64"),
                    }
                )
        return out
    with mzml.MzML(str(p)) as r:
        for s in r:
            sc = (s.get("scanList") or {}).get("scan", [{}])[0]
            t = sc.get("scan start time")
            unit = getattr(t, "unit_info", "minute") if t is not None else None
            out.append(
                {
                    "level": s.get("ms level", 1),
                    "rt_min": None if t is None else float(t) / 60.0 if unit == "second" else float(t),
                    "precursor": None,
                    "mz": np.asarray(s["m/z array"], dtype="float64"),
                    "i": np.asarray(s["intensity array"], dtype="float64"),
                    "tic": float(s["total ion current"]) if "total ion current" in s else None,
                }
            )
    return out


def ms_tic_apex(cid: str, role: str = "heldout", from_cvparam: bool = False):
    sp = [s for s in ms_spectra(cid, role) if s["level"] == 1]
    best = max(sp, key=lambda s: s["tic"] if from_cvparam else s["i"].sum())
    how = "`total ion current` cvParams" if from_cvparam else "summed intensity arrays"
    return round(best["rt_min"], 4), f"{rd('pyteomics')} ({how})"


def ms_distinct_precursors(cid: str):
    sp = ms_spectra(cid, "oracle-export")
    return len({round(s["precursor"], 2) for s in sp if s["level"] == 2}), rd("pyteomics") + " on the depositor's mzXML"


def ms_base_peak_mz(cid: str):
    (s,) = ms_spectra(cid)
    return round(float(s["mz"][s["i"].argmax()]), 4), rd("pyteomics")


# chromatography


def chemstation_file(cid: str):
    """(minutes, values) of one ChemStation .ch file (rainbow-api, LGPL, run as a black box)."""
    import numpy as np
    from rainbow.agilent import chemstation as cs

    p = hpath(cid)
    if p.is_dir():
        p = next(p.glob("*.ch"))
    df = cs.parse_file(str(p))
    x = np.asarray(df.xlabels, dtype="float64")
    return x, np.asarray(df.data, dtype="float64")[: len(x), 0]


def chemstation_signal_nm(cid: str):
    import re

    from rainbow.agilent import chemstation as cs

    signal = (cs.parse_file(str(hpath(cid))).metadata or {}).get("signal", "")
    return float(re.search(r"Sig=(\d+(?:\.\d+)?)", signal).group(1)), rd("rainbow-api") + f" (black box): {signal!r}"


def chemstation_apex(cid: str):
    x, y = chemstation_file(cid)
    return float(x[int(y.argmax())]), rd("rainbow-api") + " (black box)"


def xy_export_apex(cid: str):
    """The depositor's own .xy text export of the same run (time in min, detector counts)."""
    import numpy as np

    a = np.loadtxt(hpath(cid, "oracle-export"))
    return float(a[int(a[:, 1].argmax()), 0]), "numpy on the depositor's .xy export"


def gcms_tic_apex(cid: str):
    import numpy as np
    import scipy.io
    import scipy.io.netcdf

    scipy.io.netcdf.NetCDFFile = scipy.io.netcdf_file  # Aston imports a name newer SciPy dropped
    from aston.tracefile import TraceFile

    tr = TraceFile(str(hpath(cid))).total_trace()
    t = np.asarray(tr.index, dtype="float64")
    return float(t[int(np.asarray(tr.values, dtype="float64").ravel().argmax())]), rd("aston")


def gcms_tic_apex_rainbow(cid: str):
    import numpy as np
    from rainbow.agilent import chemstation as cs

    df = cs.parse_ms(str(hpath(cid)))
    x = np.asarray(df.xlabels, dtype="float64")
    return float(x[int(np.asarray(df.data, dtype="float64").sum(axis=1).argmax())]), rd("rainbow-api") + " (black box)"


def andi_largest_area_peak(cid: str):
    from scipy.io import netcdf_file

    with netcdf_file(hpath(cid), "r", mmap=False) as f:
        v = f.variables
        area = v["peak_area"][:].astype("float64")
        rt = v["peak_retention_time"][:].astype("float64")
        unit = f.retention_unit.decode().lower()
    t = float(rt[int(area.argmax())])
    return round(t / 60.0 if unit.startswith("sec") else t, 4), rd("scipy") + " netcdf_file (the vendor's peak table)"


# NMR


def bruker_tallest_ppm(cid: str, raw: bool = False):
    import analysis as a

    return a.pdata_ppm(cid, raw=raw)


# plates


def plate_wells(cid: str, wavelength: float | None = None) -> dict[str, float]:
    """{well: value} of the first read of a plate export (allotropy ASM), optionally one wavelength."""
    sys.path.insert(0, str(ROOT / "oracle"))
    import plate

    vendor = fx_manifest_value(cid, "plate_vendor")
    asm = plate.allotropy_asm(hpath(cid), vendor)
    out: dict[str, float] = {}
    for doc in asm["plate reader aggregate document"]["plate reader document"]:
        for m in doc["measurement aggregate document"]["measurement document"]:
            if "error aggregate document" in m:
                continue
            wl = None
            for dc in m.get("device control aggregate document", {}).get("device control document", []):
                if isinstance(dc.get("detector wavelength setting"), dict):
                    wl = float(dc["detector wavelength setting"]["value"])
            if wavelength is not None and wl != wavelength:
                continue
            for k, v in m.items():
                if plate.mode_of(k) and isinstance(v, dict) and "value" in v:
                    well = m["sample document"]["location identifier"]
                    out.setdefault(well, float(v["value"]))  # first read wins
    return out


def fx_manifest_value(cid: str, key: str):
    import tomllib

    with (ROOT / "corpus" / "manifest.toml").open("rb") as fh:
        for e in tomllib.load(fh)["file"]:
            if e["id"] == cid and e.get("role") == "heldout":
                return e[key]
    raise KeyError(cid)


def plate_top_well(cid: str, wavelength: float | None = None):
    vals = plate_wells(cid, wavelength)
    top, second = sorted(vals.values())[-1], sorted(vals.values())[-2]
    assert top > second, "tie for the top well"
    return max(vals, key=vals.get), rd("allotropy")


# vibrational spectroscopy (held-out additions 2026-09-24)


def _quiet():
    import contextlib
    import io
    import warnings

    warnings.simplefilter("ignore")
    stack = contextlib.ExitStack()
    stack.enter_context(contextlib.redirect_stdout(io.StringIO()))
    stack.enter_context(contextlib.redirect_stderr(io.StringIO()))
    return stack


def band(x, y, lo: float, hi: float, kind: str = "max", margin: float = 0.01) -> float:
    """x of the highest (kind "max") or lowest ("min") y within [lo, hi]; stops unless the extreme
    beats every point more than 10 x-units away by `margin` (relative), so the answer is one band."""
    import numpy as np

    x, y = np.asarray(x, dtype="float64"), np.asarray(y, dtype="float64")
    m = (x >= lo) & (x <= hi)
    xs, ys = x[m], y[m] if kind == "max" else -y[m]
    i = int(ys.argmax())
    far = np.abs(xs - xs[i]) > 10
    if far.any():
        rest = ys[far].max()
        assert ys[i] - rest > margin * abs(ys[i]), f"no single band: {ys[i]} vs {rest} at {xs[far][ys[far].argmax()]}"
    return float(xs[i])


def text_xy(p):
    """(x, y) of a vendor text export (OPUS .dpt, OMNIC CSV, WiRE TXT, Spectrum CSV): the first two
    numeric columns of every line that has them."""
    import numpy as np

    rows = []
    for line in Path(p).read_text(errors="replace").splitlines():
        parts = line.replace(";", " ").replace(",", " ").replace("\t", " ").split()
        try:
            rows.append((float(parts[0]), float(parts[1])))
        except (IndexError, ValueError):
            continue
    a = np.asarray(rows)
    return a[:, 0], a[:, 1]


def opus_block_xy(cid: str, block: str):
    from brukeropus import OPUSFile

    with _quiet():
        d = getattr(OPUSFile(str(hpath(cid))), block)
    return d.x, d.y


def opus_extreme(cid: str, block: str, lo: float, hi: float, kind: str):
    x, y = opus_block_xy(cid, block)
    return band(x, y, lo, hi, kind), rd("brukeropus") + f" (block `{block}`)"


def value_at(x, y, at: float) -> float:
    """y linearly interpolated at x = `at` (x in either order)."""
    import numpy as np

    x, y = np.asarray(x, dtype="float64"), np.asarray(y, dtype="float64")
    o = np.argsort(x)
    return float(np.interp(at, x[o], y[o]))


def opus_value_at(cid: str, block: str, at: float):
    x, y = opus_block_xy(cid, block)
    return value_at(x, y, at), rd("brukeropus") + f" (block `{block}`, linear interpolation)"


def export_value_at(cid: str, at: float):
    x, y = text_xy(hpath(cid, "oracle-export"))
    return value_at(x, y, at), "the vendor's text export " + hpath(cid, "oracle-export").name


def export_extreme(cid: str, lo: float, hi: float, kind: str):
    x, y = text_xy(hpath(cid, "oracle-export"))
    return band(x, y, lo, hi, kind), "the vendor's text export " + hpath(cid, "oracle-export").name


def omnic_data(cid: str):
    import numpy as np
    import spectrochempy as scp

    with _quiet():
        d = scp.read_omnic(str(hpath(cid)))
    return np.asarray(d.x.data, dtype="float64"), np.asarray(d.data, dtype="float64")


def omnic_extreme(cid: str, lo: float, hi: float, kind: str):
    x, y = omnic_data(cid)
    return band(x, y[0], lo, hi, kind), rd("spectrochempy")


def omnic_group_argmax(cid: str, lo: float, hi: float):
    """Spectrum (counting from 1, file order) whose maximum within [lo, hi] is the highest."""
    x, y = omnic_data(cid)
    m = (x >= lo) & (x <= hi)
    peaks = y[:, m].max(axis=1)
    order = peaks.argsort()
    assert peaks[order[-1]] > 1.02 * peaks[order[-2]], peaks
    return int(order[-1]) + 1, rd("spectrochempy")


def wdf_spectra(cid: str):
    import numpy as np
    from renishawWiRE import WDFReader

    with _quiet():
        r = WDFReader(str(hpath(cid)))
    return np.asarray(r.xdata, dtype="float64"), np.asarray(r.spectra, dtype="float64").reshape(
        -1, r.point_per_spectrum
    )


def wdf_mean_band(cid: str, cross: bool = False):
    """Raman shift of the tallest point of the mean of every spectrum of the file."""
    import numpy as np

    if cross:
        import spectrochempy as scp

        with _quiet():
            d = scp.read_wdf(str(hpath(cid)))
        x = np.asarray(d.x.data, dtype="float64")
        y = np.asarray(d.data, dtype="float64").reshape(-1, x.size).mean(axis=0)
        return band(x, y, 0, 1e5, "max", margin=0.0), rd("spectrochempy") + " read_wdf"
    x, s = wdf_spectra(cid)
    return band(x, s.mean(axis=0), 0, 1e5, "max", margin=0.0), rd("renishawWiRE")


def wdf_first_band(cid: str):
    x, s = wdf_spectra(cid)
    return band(x, s[0], 0, 1e5, "max"), rd("renishawWiRE")


def pesp_xy(cid: str):
    import collections
    import collections.abc

    import numpy as np

    for n in ("Iterable", "Mapping", "Sequence", "MutableMapping"):
        if not hasattr(collections, n):
            setattr(collections, n, getattr(collections.abc, n))
    from specio import specread

    s = specread(str(hpath(cid)))
    return np.asarray(s.wavelength, dtype="float64"), np.asarray(s.amplitudes, dtype="float64").ravel()


def pesp_extreme(cid: str, lo: float, hi: float, kind: str):
    x, y = pesp_xy(cid)
    return band(x, y, lo, hi, kind), rd("specio")


# qPCR (held-out additions 2026-09-24)


def xls_results(cid: str) -> list[dict]:
    """Rows of the `Results` sheet of a QuantStudio / ViiA 7 .xls export (xlrd, BSD-3)."""
    import xlrd

    sh = xlrd.open_workbook(str(hpath(cid, "oracle-export"))).sheet_by_name("Results")
    hdr = next(r for r in range(sh.nrows) if sh.row_values(r)[:2] == ["Well", "Well Position"])
    names = sh.row_values(hdr)
    return [dict(zip(names, sh.row_values(r), strict=False)) for r in range(hdr + 1, sh.nrows) if sh.row_values(r)[1]]


def eds_records(cid: str) -> list[dict]:
    """Per-well vendor results the oracle read from the .eds (analysis_result.txt etc.)."""
    return load_oracle(cid)["records"]


def eds_highest_mean_ct(cid: str, from_export: bool = False):
    import statistics
    from collections import defaultdict

    by: dict[str, list[float]] = defaultdict(list)
    if from_export:
        for r in xls_results(cid):
            if isinstance(r["CT"], float):
                by[r["Target Name"]].append(r["CT"])
        src = "xlrd on the vendor's .xls export (Results sheet, CT)"
    else:
        cycles = load_oracle(cid)["cycles"]
        for r in eds_records(cid):
            if r["cq"] is not None and not r["cq_undetermined"] and r["cq"] < cycles:
                by[r["target"]].append(r["cq"])
        src = "the vendor's analysis_result.txt inside the .eds (oracle/qpcr.py)"
    means = sorted(((statistics.mean(v), t) for t, v in by.items()), reverse=True)
    assert means[0][0] - means[1][0] > 1.0, means[:2]
    return means[0][1], src


def eds_well_tm(cid: str, well: str, from_export: bool = False):
    if from_export:
        r = next(r for r in xls_results(cid) if r["Well Position"] == well)
        return float(r["Tm1"]), "xlrd on the vendor's .xls export (Results sheet, Tm1)"
    r = next(r for r in eds_records(cid) if r["well"] == well)
    assert len(r["tm"]) == 1, r["tm"]
    return float(r["tm"][0]), "the vendor's meltcuve_result.txt inside the .eds (oracle/qpcr.py)"


def eds_undetermined(cid: str, target: str, from_export: bool = False):
    if from_export:
        rows = [r for r in xls_results(cid) if r["Target Name"] == target]
        return sum(r["CT"] == "Undetermined" for r in rows), "xlrd on the vendor's .xls export (CT = Undetermined)"
    cycles = load_oracle(cid)["cycles"]
    rows = [r for r in eds_records(cid) if r["target"] == target]
    n = sum(r["cq"] is None or r["cq_undetermined"] or r["cq"] >= cycles for r in rows)
    return n, "the vendor's analysis_result.txt inside the .eds (no Ct below the last cycle)"


def eds_rq(cid: str, sample: str, target: str, from_export: bool = False):
    if from_export:
        r = next(
            r
            for r in xls_results(cid)
            if r["Sample Name"] == sample and r["Target Name"] == target and isinstance(r["RQ"], float)
        )
        return float(r["RQ"]), "xlrd on the vendor's .xls export (Results sheet, RQ)"
    r = next(
        r for r in eds_records(cid) if r["sample"] == sample and r["target"] == target and r["vendor_rq"] is not None
    )
    return float(r["vendor_rq"]), "the vendor's analysis_result.txt inside the .eds (RQ)"


def rex_export_ct(cid: str) -> dict[str, list[float]]:
    """{tube name: [Ct, ...]} of a Rotor-Gene Q 'Excel Analysed Data Export' CSV."""
    import csv

    rows = list(csv.reader(hpath(cid, "oracle-export").read_text(encoding="latin-1").splitlines()))
    hdr = next(i for i, r in enumerate(rows) if r[:3] == ["No.", "Color", "Name"])
    out: dict[str, list[float]] = {}
    for r in rows[hdr + 1 :]:
        if len(r) > 4 and r[0].isdigit() and r[4].strip():
            out.setdefault(r[2], []).append(float(r[4]))
    return out


def rex_extreme_ct_gene(cid: str, kind: str, exclude: tuple[str, ...] = ()):
    import statistics

    means = sorted((statistics.mean(v), k) for k, v in rex_export_ct(cid).items() if k not in exclude)
    lo, hi = means[0], means[-1]
    if kind == "min":
        assert means[1][0] - lo[0] > 1.0, means[:2]
        return lo[1], "the Rotor-Gene Q software's analysed-data CSV export (Ct, threshold 0.1)"
    assert hi[0] - means[-2][0] > 1.0, means[-2:]
    return hi[1], "the Rotor-Gene Q software's analysed-data CSV export (Ct, threshold 0.1)"


def rdml_highest_mean_cq(cid: str):
    import statistics
    from collections import defaultdict

    by: dict[str, list[float]] = defaultdict(list)
    for r in load_oracle(cid)["records"]:
        if r["cq"] is not None and not r["cq_undetermined"] and r["cq"] > 0:
            by[r["target"]].append(r["cq"])
    means = sorted(((statistics.mean(v), t) for t, v in by.items()), reverse=True)
    assert means[0][0] - means[1][0] > 1.0, means[:2]
    return means[0][
        1
    ], "rdmlpython (the RDML consortium's reader, via oracle/qpcr.py): Cq values the CFX Manager export stores"


def rdml_highest_mean_cq_xml(cid: str):
    """The same from the RDML XML with ElementTree (every <data><cq> of every <react>)."""
    import statistics
    import xml.etree.ElementTree as ET
    import zipfile
    from collections import defaultdict

    p = hpath(cid)
    if zipfile.is_zipfile(p):  # an .rdml file is usually a zip holding rdml_data.xml
        with zipfile.ZipFile(p) as z:
            root = ET.fromstring(z.read(next(n for n in z.namelist() if n.lower().endswith(".xml"))))
    else:
        root = ET.parse(p).getroot()
    loc = lambda t: t.rsplit("}", 1)[-1]  # noqa: E731
    by: dict[str, list[float]] = defaultdict(list)
    for el in root.iter():
        if loc(el.tag) != "data":
            continue
        tgt = next((c.attrib.get("id") for c in el if loc(c.tag) == "tar"), None)
        cq = next((c.text for c in el if loc(c.tag) == "cq"), None)
        if tgt and cq and float(cq) > 0:
            by[tgt].append(float(cq))
    means = sorted(((statistics.mean(v), t) for t, v in by.items()), reverse=True)
    return means[0][1], "ElementTree over the RDML XML"


# high-content screening plates (held-out additions 2026-09-24)


def harmony_named_mean(cid: str, row: int, col: int, channel: int, field: int | None = None):
    """Pooled mean (tifffile) of the Harmony plane files `rRRcCCfFFp01-chN...` of one well and channel
    number, all fields present (or one), found by the Harmony file-name convention."""
    import analysis as a

    folder = hpath(cid)
    f = f"{field:02d}" if field else "??"
    files = sorted(folder.glob(f"Images/r{row:02d}c{col:02d}f{f}p01-ch{channel}sk1fk1fl1.tiff"))
    assert files, (cid, row, col, channel, field)
    return a._tif_mean(files), rd("tifffile")


def harmony_brighter_well(cid: str, wells: list[tuple[int, int]], channel: int):
    means = {f"{chr(64 + r)}{c:02d}": harmony_named_mean(cid, r, c, channel)[0] for r, c in wells}
    top, second = sorted(means.values())[-1], sorted(means.values())[-2]
    assert top > 1.02 * second, means
    return max(means, key=means.get), rd("tifffile")


def harmony_brightest_field(cid: str, row: int, col: int, channel_name: str, fields: int):
    import analysis as a

    means = {f: a.hcs_harmony_mean(cid, row, col, f, channel_name)[0] for f in range(1, fields + 1)}
    top, second = sorted(means.values())[-1], sorted(means.values())[-2]
    assert top > 1.02 * second, means
    return max(means, key=means.get), rd("tifffile") + " (planes named by Index.idx.xml)"


def ix_brightest_site(cid: str, plate: str, well: str, wave: int, sites: int):
    import analysis as a

    folder = hpath(cid)
    means = {s: a._tif_mean([folder / f"{plate}_{well}_s{s}_w{wave}.TIF"]) for s in range(1, sites + 1)}
    top, second = sorted(means.values())[-1], sorted(means.values())[-2]
    assert top > 1.02 * second, means
    return max(means, key=means.get), rd("tifffile") + " (MetaXpress file names: _<well>_s<site>_w<wavelength>)"


# newer analysis commands (held-out additions 2026-09-24)


def lcd_export(cid: str) -> dict:
    """Chromatograms and peak tables of the depositor's LabSolutions ASCII export (oracle/gen_heldout.py)."""
    sys.path.insert(0, str(ROOT / "oracle"))
    import gen_heldout

    return gen_heldout.labsolutions_export(hpath(cid, "oracle-export"))["shimadzu_export"]


def lcd_export_trace(cid: str, name: str):
    """(minutes, values) of one [LC Chromatogram(name)] block of the export."""
    import numpy as np

    rows, on = [], False
    for line in hpath(cid, "oracle-export").read_text(encoding="latin-1").splitlines():
        if line.startswith("["):
            on = line.strip() == f"[LC Chromatogram({name})]"
            continue
        parts = line.split("\t")
        if on and len(parts) >= 2:
            try:
                rows.append((float(parts[0]), float(parts[1])))
            except ValueError:
                continue
    a = np.asarray(rows)
    return a[:, 0], a[:, 1]


def lcd_tallest_rt(cid: str, table: str, from_trace: bool = False):
    if from_trace:
        x, y = lcd_export_trace(cid, table)
        return round(float(x[int(y.argmax())]), 3), "argmax of the export's chromatogram points"
    peaks = sorted(lcd_export(cid)["peak_tables"][table], key=lambda p: -p["height"])
    assert peaks[0]["height"] > 1.3 * peaks[1]["height"], peaks[:2]
    return peaks[0]["rt_min"], "the vendor's peak table (LabSolutions ASCII export)"


def lcd_nth_area_rt(cid: str, table: str, n: int):
    peaks = sorted(lcd_export(cid)["peak_tables"][table], key=lambda p: -p["area"])
    assert peaks[n - 1]["area"] > 1.3 * peaks[n]["area"], peaks[: n + 1]
    return peaks[n - 1]["rt_min"], "the vendor's peak table (LabSolutions ASCII export)"


def gz_spectra(cid: str) -> list[dict]:
    """MS level, retention time (min), TIC cvParam and intensity sum of every spectrum of a
    gzip-compressed mzML, pyteomics on the decompressed stream."""
    import gzip

    import numpy as np
    from pyteomics import mzml

    out = []
    with gzip.open(hpath(cid), "rb") as fh, mzml.MzML(fh, use_index=False) as r:
        for s in r:
            sc = (s.get("scanList") or {}).get("scan", [{}])[0]
            t = sc.get("scan start time")
            unit = getattr(t, "unit_info", "minute")
            out.append(
                {
                    "level": s.get("ms level", 1),
                    "rt_min": float(t) / 60.0 if unit == "second" else float(t),
                    "tic": float(s["total ion current"]) if "total ion current" in s else None,
                    "sum": float(np.asarray(s["intensity array"], dtype="float64").sum()),
                }
            )
    return out


def gz_ms1_tic_apex(cid: str, from_cvparam: bool = False):
    sp = [s for s in gz_spectra(cid) if s["level"] == 1]
    best = max(sp, key=lambda s: s["tic"] if from_cvparam else s["sum"])
    how = "`total ion current` cvParams" if from_cvparam else "summed intensity arrays"
    return round(best["rt_min"], 4), f"{rd('pyteomics')} on the decompressed mzML ({how})"


def mzmlb_first_precursor(cid: str):
    from pyteomics import mzmlb

    with mzmlb.MzMLb(str(hpath(cid))) as r:
        s = next(iter(r))
        ion = s["precursorList"]["precursor"][0]["selectedIonList"]["selectedIon"][0]
    return round(float(ion["selected ion m/z"]), 4), rd("pyteomics", "h5py") + " (pyteomics.mzmlb)"


def gen5_sample_conc(cid: str, wells: list[str], refit: bool = False):
    """Mean concentration of a sample's wells: the vendor's Conc block of a Gen5 export, or (refit)
    a NumPy least-squares line through the blank-subtracted standards (x = concentration)."""
    import numpy as np

    lines = hpath(cid).read_text(encoding="latin-1").splitlines()

    def block(title: str) -> dict[str, str]:
        i = next(k for k, ln in enumerate(lines) if ln.strip() == title)
        out = {}
        for ln in lines[i + 2 : i + 10]:
            cells = ln.split("\t")
            for c, v in enumerate(cells[1:13], start=1):
                out[f"{cells[0]}{c}"] = v
        return out

    if not refit:
        conc = block("Conc")
        return float(np.mean([float(conc[w]) for w in wells])), "the vendor's Conc block (Gen5 export)"
    od = block("Blank 562")
    std = {"2": 0.0, "3": 10.0, "4": 25.0, "5": 50.0, "6": 100.0}  # layout: STD1-5 in columns 2-6, rows A-B
    x = [std[w[1:]] for w in od if w[0] in "AB" and w[1:] in std]
    y = [float(od[w]) for w in od if w[0] in "AB" and w[1:] in std]
    slope, intercept = np.polyfit(x, y, 1)
    vals = [(float(od[w]) - intercept) / slope for w in wells]
    return float(np.mean(vals)), "NumPy linear fit of the blank-subtracted standards (duplicates), back-calculated"


def abf_depositor_rheobase(cid: str, recording: str):
    """The current step the depositor wrote down as the first to evoke spikes (index spreadsheet)."""
    import re

    import openpyxl

    wb = openpyxl.load_workbook(hpath(cid, "oracle-export"), read_only=True)
    for ws in wb:
        for row in ws.iter_rows(values_only=True):
            cells = [str(c) for c in row if c is not None]
            for k, c in enumerate(cells[:-1]):
                if c == recording and "spikes at" in cells[k + 1]:
                    return float(
                        re.search(r"spikes at (\d+)", cells[k + 1]).group(1)
                    ), "the depositor's index spreadsheet"
    raise KeyError(recording)


def abf_last_sweep_ap_count_neo(cid: str, threshold: float = 0.0):
    import numpy as np

    import analysis as a

    sw, _ = a.abf_sweeps_neo(cid, 0)
    y = sw[-1]
    return int(np.count_nonzero((y[:-1] < threshold) & (y[1:] >= threshold))), rd("neo") + " AxonRawIO + numpy"


def svs_region_mean(cid: str, x0: int, y0: int, w: int, h: int, sample: int, openslide: bool = False):
    """Mean of one colour sample (0 red) of a full-resolution region, tifffile + imagecodecs or OpenSlide."""
    import numpy as np

    if openslide:
        import openslide as osl

        s = osl.OpenSlide(str(hpath(cid)))
        a = np.asarray(s.read_region((x0, y0), 0, (w, h)))[..., :3]
        return a[..., sample].mean(dtype="float64"), rd("openslide-python") + " (OpenSlide, black box)"
    import tifffile

    with tifffile.TiffFile(hpath(cid)) as tf:
        a = tf.pages[0].asarray()
    return a[y0 : y0 + h, x0 : x0 + w, sample].mean(dtype="float64"), rd("tifffile", "imagecodecs")


def bruker_tallest_ppm_window(cid: str, lo: float, hi: float, raw: bool = False):
    import analysis as a

    return a.pdata_ppm(cid, lo, hi, raw=raw)


C = {
    "xzt": "ho-zenodo19047136-xzt-lsm800",
    "axioscan": "ho-zenodo17736625-axioscan-12scenes",
    "airyscan": "ho-zenodo6848342-airyscan-lsm880",
    "lsm780": "ho-zenodo579617-lsm780-zen2011",
    "rbc": "ho-figshare28719716-rbc-timelapse",
    "c2plus": "ho-zenodo20453349-c2plus-zstack",
    "rgb": "ho-zenodo12734440-dsfi3-rgb",
    "nsparc": "ho-figshare29205017-ax-nsparc",
    "sp8": "ho-zenodo18851060-sp8-bio407",
    "stellaris": "ho-zenodo18174107-stellaris5",
    "sp5": "ho-zenodo16316945-sp5-resonant-tl",
    "prostate": "ho-zenodo18302140-prostate5-bf670",
    "ndpi": "ho-zenodo18302140-ndpi-fluo",
    "zenexport": "ho-zenodo10222721-zen-export",
    "oir": "ho-zenodo5114678-fv3000-oir",
    "oib": "ho-zenodo14205552-fv1000-oib",
    "svs": "ho-zenodo21842475-aperio-svs",
    "ims": "ho-zenodo14675120-dragonfly-ims",
    "zvi": "ho-figshare18420278-zvi",
    "cryosparc": "ho-zenodo15755070-cryosparc-map",
    "tomo": "ho-zenodo10837519-imod-tomo",
    "dm3": "ho-zenodo18299303-ed-diff",
    "eels": "ho-zenodo19451991-eels-si",
    "ser": "ho-zenodo1486742-ser-stemdiff",
    "velox": "ho-zenodo11098177-velox-4det",
    "attune": "ho-zenodo7352402-attune-nxt",
    "moflo": "ho-zenodo21975568-moflo-summit",
    "novocyte": "ho-zenodo15350040-novocyte-penteon",
    "melody": "ho-zenodo5174952-facsmelody",
    "abf1": "ho-zenodo20758248-abf1-gapfree-vc",
    "iclamp": "ho-zenodo8356786-abf2-iclamp",
    "episodic": "ho-zenodo4988993-abf2-vc-episodic",
    "nwb": "ho-dandi000293-uhn-icephys",
    "bruker1h": "ho-zenodo22673699-1h",
    "hsqc1200": "ho-zenodo14929539-hsqc1200",
    "hsqc900": "ho-zenodo14929539-hsqc900",
    "jeol1h": "ho-zenodo22230007-1h",
    "jeolhsqc": "ho-zenodo22230007-hsqc",
    "varian": "ho-zenodo22709194-1h",
    "ir": "ho-zenodo7849381-atr-ir",
    "fid": "ho-figshare29988259-101f0101",
    "gcms": "ho-figshare28050092-yucca-gcms",
    "dad": "ho-figshare25897444-dad1a",
    "andi": "ho-zenodo18592075-aso-uv",
    "gen5": "ho-bnext-cytation5-pierce660",
    "gen5kin": "ho-bnext-cytation3-vio",
    "softmax": "ho-minikel-softmax-elisa021",
    "icontrol": "ho-barrick-icontrol-cr082824",
    "exploris": "ho-zenodo11284462-exploris-wash3",
    "ltqxl": "ho-pxd010395-ltqxl-dt28",
    "tsq": "ho-pxd036704-tsq-srm-c10",
    "prm": "ho-pxd065795-qehf-prm",
    "tt5600": "ho-zenodo20729183-tt5600-mzr",
    "maldi": "ho-zenodo3746000-maldiquant",
    # held-out additions 2026-09-24: spectroscopy, qPCR, screening plates, newer analysis commands
    "opus70": "ho-zenodo3986032-opus-vertex70",
    "opusalpha": "ho-zenodo13928180-opus-alpha",
    "spa": "ho-zenodo17691644-omnic-spa",
    "spg": "ho-zenodo7777291-omnic-spg",
    "wdfmap": "ho-zenodo21018471-wdf-map",
    "wdf1": "ho-zenodo13353392-wdf-single",
    "pesp": "ho-zenodo13752773-pesp-frontier",
    "qs6": "ho-figshare32881001-qs6flex-eds",
    "viia7": "ho-figshare26362456-viia7-eds",
    "rex": "ho-zenodo6754439-rotorgene-rex",
    "rdml": "ho-gh-ramiromagno-rdml-rpa",
    "harmony6": "ho-cpg0002-phenix-harmony6",
    "harmony5": "ho-cpg0036-medina-harmony5",
    "ixbia": "ho-biad2152-imagexpress-210224",
    "cv8000": "ho-cpg0036-imtm-cv8000",
    "lcd": "ho-gh-actolonen-lcd-std25",
    "mzmlgz": "ho-pxd042958-ltq-orbitrap-mzmlgz",
    "mzmlb": "ho-gh-pwiz-mzmlb-narrow",
    "gen5bca": "ho-gh-kaiaragaki-mop-gen5-bca",
    "fid600": "ho-zenodo14988141-1h-d2o",
    "abfsteps": "ho-figshare12613811-abf-steps",
    "svsjp2k": "ho-zenodo17362964-svs-jp2k",
    # bench instruments (2026-09-26): ÄKTA/UNICORN, Image Lab, JASCO
    "unicornho": "ho-gh-artiums-unicorn-histrap",
    "scnho": "ho-zenodo16611302-scn-biochimlab",
    "jwsho": "ho-zenodo22832447-jws-ftir-atr",
    "seahorseho": "ho-zenodo8277227-seahorse-taz",
}

FACTS: list[Fact] = [
    Fact(
        C["rgb"],
        "mean_red",
        "mean of the red sample of the 2048×2880 RGB image, as NIS-Elements' own OME-TIFF "
        "export orders the samples (R, G, B); the nd2 package returns the samples in stored order (B, G, R): "
        "its third sample is the same array",
        lambda: ometiff_rgb_mean(C["rgb"], 0),
        lambda: nd2_rgb_mean(C["rgb"], 2),
    ),
    Fact(
        C["nsparc"],
        "brightest_channel",
        "name of the channel with the highest mean over its 1024×1024 plane",
        lambda: nd2_brightest_channel(C["nsparc"]),
    ),
    Fact(
        C["sp5"],
        "dimmest_t",
        "frame (counting from 1) with the lowest mean of the 1500-frame 128×128 xyt series",
        lambda: lif_dimmest_t(C["sp5"]),
        lambda: lif_dimmest_t_cross(C["sp5"]),
    ),
    Fact(
        C["sp8"],
        "brightest_series_c1",
        "series name with the highest mean in channel index 0 (DAPI)",
        lambda: lif_brightest_series(C["sp8"], 0),
        lambda: lif_brightest_series(C["sp8"], 0, cross=True),
    ),
    Fact(
        C["cryosparc"],
        "max_density",
        "maximum voxel value of the float32 map",
        lambda: mrc_max(C["cryosparc"]),
        lambda: mrc_max(C["cryosparc"], numpy=True),
    ),
    Fact(
        C["tomo"],
        "mean_voxel",
        "mean of every int16 voxel of the tomogram",
        lambda: mrc_mean(C["tomo"]),
        lambda: mrc_mean(C["tomo"], numpy=True),
    ),
    Fact(
        C["attune"],
        "median_bl1_a",
        "median of BL1-A ($P4N, CX3CR1-FITC-A) over all events, values as stored",
        lambda: fcs_raw_median(C["attune"], "BL1-A"),
        lambda: fcs_raw_median(C["attune"], "BL1-A", fcsparser=True),
    ),
    Fact(
        C["melody"],
        "median_ssc_a",
        "median of SSC-A over all events, values as stored",
        lambda: fcs_raw_median(C["melody"], "SSC-A"),
        lambda: fcs_raw_median(C["melody"], "SSC-A", fcsparser=True),
    ),
    Fact(
        C["novocyte"],
        "median_b525_a",
        "median of B525-A (AF488 - Puromycin-A) over all events, values as stored",
        lambda: fcs_raw_median(C["novocyte"], "B525-A"),
        lambda: fcs_raw_median(C["novocyte"], "B525-A", fcsparser=True),
    ),
    Fact(
        C["iclamp"],
        "peak_mv",
        "maximum of channel 0 (IN 0, mV) in the only sweep",
        lambda: __import__("analysis").abf_peak(C["iclamp"], 1, 0),
        lambda: __import__("analysis").abf_peak(C["iclamp"], 1, 0, neo=True),
    ),
    Fact(
        C["episodic"],
        "most_negative_sweep",
        "sweep (counting from 1) whose minimum current is the most negative",
        lambda: abf_most_negative_sweep(C["episodic"]),
        lambda: abf_most_negative_sweep(C["episodic"], neo=True),
    ),
    Fact(
        C["abf1"],
        "min_pa",
        "most negative current (pA) of the whole 90 s gap-free recording",
        lambda: abf_min(C["abf1"]),
        lambda: abf_min(C["abf1"], neo=True),
    ),
    Fact(
        C["bruker1h"],
        "tallest_ppm",
        "chemical shift of the highest point of pdata/1/1r (ppm = OFFSET - i·SW_p/SF/SI)",
        lambda: bruker_tallest_ppm(C["bruker1h"]),
        lambda: bruker_tallest_ppm(C["bruker1h"], raw=True),
    ),
    Fact(
        C["exploris"],
        "tic_apex_min",
        "retention time (min) of the MS1 scan with the largest summed intensity, depositor's mzML",
        lambda: ms_tic_apex(C["exploris"], "oracle-export"),
        lambda: ms_tic_apex(C["exploris"], "oracle-export", from_cvparam=True),
    ),
    Fact(
        C["tt5600"],
        "tic_apex_min",
        "retention time (min) of the spectrum with the largest summed intensity",
        lambda: ms_tic_apex(C["tt5600"]),
        lambda: ms_tic_apex(C["tt5600"], from_cvparam=True),
    ),
    Fact(
        C["maldi"],
        "base_peak_mz",
        "m/z of the most intense point of the only spectrum (profile)",
        lambda: ms_base_peak_mz(C["maldi"]),
    ),
    Fact(
        C["tsq"],
        "distinct_precursors",
        "number of distinct SRM precursor m/z values (rounded to 0.01) over all scans",
        lambda: ms_distinct_precursors(C["tsq"]),
    ),
    Fact(
        C["prm"],
        "distinct_precursors",
        "number of distinct PRM precursor m/z values (rounded to 0.01) over all MS2 scans",
        lambda: ms_distinct_precursors(C["prm"]),
    ),
    Fact(
        C["fid"],
        "apex_min",
        "retention time (min) of the highest FID1A value",
        lambda: chemstation_apex(C["fid"]),
        lambda: xy_export_apex(C["fid"]),
        rel=1e-4,
    ),
    Fact(C["dad"], "apex_min", "retention time (min) of the highest DAD1A value", lambda: chemstation_apex(C["dad"])),
    Fact(
        C["dad"],
        "signal_nm",
        "the `Sig=` wavelength of the signal description rainbow-api reads from the file header",
        lambda: chemstation_signal_nm(C["dad"]),
    ),
    Fact(
        C["gcms"],
        "tic_apex_min",
        "retention time (min) of the scan with the largest total ion current",
        lambda: gcms_tic_apex(C["gcms"]),
        lambda: gcms_tic_apex_rainbow(C["gcms"]),
    ),
    Fact(
        C["andi"],
        "largest_peak_min",
        "retention time (min) of the peak with the largest area in the file's own peak table",
        lambda: andi_largest_area_peak(C["andi"]),
    ),
    Fact(
        C["softmax"],
        "top_well_450",
        "well with the highest absorbance at 450 nm",
        lambda: plate_top_well(C["softmax"], 450.0),
    ),
    Fact(
        C["gen5"], "top_well", "well with the highest value of the first 660 nm read", lambda: plate_top_well(C["gen5"])
    ),
    # ---- held-out additions 2026-09-24: vibrational spectroscopy
    Fact(
        C["opus70"],
        "t_at_1000",
        "transmittance of the transmittance block at 1000 cm-1 (linear interpolation between the two nearest points)",
        lambda: opus_value_at(C["opus70"], "t", 1000.0),
        lambda: export_value_at(C["opus70"], 1000.0),
    ),
    Fact(
        C["opusalpha"],
        "min_result_600_1800",
        "wavenumber (cm-1) of the lowest point of the result (ARIT) spectrum between 600 and 1800 cm-1",
        lambda: opus_extreme(C["opusalpha"], "arit", 600, 1800, "min"),
        lambda: export_extreme(C["opusalpha"], 600, 1800, "min"),
    ),
    Fact(
        C["spa"],
        "min_t",
        "wavenumber (cm-1) of the lowest %T of the spectrum (450-4000 cm-1)",
        lambda: omnic_extreme(C["spa"], 450, 4000, "min"),
        lambda: export_extreme(C["spa"], 450, 4000, "min"),
        rel=1e-4,
    ),
    Fact(
        C["spg"],
        "max_amide1_spectrum",
        "spectrum of the group with the highest absorbance maximum between 1600 and 1700 cm-1",
        lambda: omnic_group_argmax(C["spg"], 1600, 1700),
    ),
    Fact(
        C["wdfmap"],
        "mean_spectrum_band",
        "Raman shift (cm-1) of the tallest point of the mean of all 2601 map spectra",
        lambda: wdf_mean_band(C["wdfmap"]),
        lambda: wdf_mean_band(C["wdfmap"], cross=True),
        rel=1e-4,
    ),
    Fact(
        C["wdf1"],
        "tallest_band",
        "Raman shift (cm-1) of the tallest point of the spectrum",
        lambda: wdf_first_band(C["wdf1"]),
        lambda: export_extreme(C["wdf1"], 0, 1e5, "max"),
        rel=1e-4,
    ),
    Fact(
        C["pesp"],
        "min_t",
        "wavenumber (cm-1) of the lowest %T of the spectrum (600-4000 cm-1)",
        lambda: pesp_extreme(C["pesp"], 600, 4000, "min"),
        lambda: export_extreme(C["pesp"], 600, 4000, "min"),
        rel=1e-4,
    ),
    # ---- held-out additions 2026-09-24: qPCR (vendor-computed results)
    Fact(
        C["qs6"],
        "highest_mean_ct_target",
        "target with the highest mean Ct over its wells that have a Ct (vendor results)",
        lambda: eds_highest_mean_ct(C["qs6"]),
        lambda: eds_highest_mean_ct(C["qs6"], from_export=True),
    ),
    Fact(
        C["qs6"],
        "tm_a10",
        "melting temperature (Tm1, deg C) the software reported for well A10 (target 3-Sphk1)",
        lambda: eds_well_tm(C["qs6"], "A10"),
        lambda: eds_well_tm(C["qs6"], "A10", from_export=True),
    ),
    Fact(
        C["viia7"],
        "cb1_undetermined",
        "number of CB1 wells (the no-template control included) whose Ct is Undetermined",
        lambda: eds_undetermined(C["viia7"], "CB1"),
        lambda: eds_undetermined(C["viia7"], "CB1", from_export=True),
    ),
    Fact(
        C["viia7"],
        "rq_cb1_ssc9_high",
        "vendor RQ (2^-ddCt; reference gene S18, calibrator sample 'SHSY5Y (S4A)') of CB1 in sample 'SSC-9 high'",
        lambda: eds_rq(C["viia7"], "SSC-9 high", "CB1"),
        lambda: eds_rq(C["viia7"], "SSC-9 high", "CB1", from_export=True),
    ),
    Fact(
        C["rex"],
        "earliest_gene",
        "tube name (gene) with the lowest mean Ct in Cycling A.Green, 18s RNA left out (vendor analysed-data export)",
        lambda: rex_extreme_ct_gene(C["rex"], "min", exclude=("18s RNA",)),
    ),
    Fact(
        C["rex"],
        "latest_gene",
        "tube name (gene) with the highest mean Ct in Cycling A.Green (vendor export; NTCs have none)",
        lambda: rex_extreme_ct_gene(C["rex"], "max"),
    ),
    Fact(
        C["rdml"],
        "highest_mean_cq_target",
        "target with the highest mean Cq over its reactions that have one",
        lambda: rdml_highest_mean_cq(C["rdml"]),
        lambda: rdml_highest_mean_cq_xml(C["rdml"]),
    ),
    # ---- held-out additions 2026-09-24: high-content screening plates
    Fact(
        C["harmony6"],
        "brighter_well_phalloidin",
        "of wells A01 and A02 (the wells in the copy), the one with the higher pooled mean of channel 4 "
        "(WG-Phalloidin) over its fields",
        lambda: harmony_brighter_well(C["harmony6"], [(1, 1), (1, 2)], 4),
    ),
    Fact(
        C["harmony5"],
        "brightest_field_mito_a01",
        "field (counting from 1) of well A01 with the highest mean in the MitoTracker Deep Red channel",
        lambda: harmony_brightest_field(C["harmony5"], 1, 1, "MitoTracker Deep Red", 9),
    ),
    Fact(
        C["ixbia"],
        "brightest_site_dapi_b02",
        "site (counting from 1) of well B02 with the highest mean in wavelength 1 (DAPI)",
        lambda: ix_brightest_site(C["ixbia"], "210224", "B02", 1, 4),
    ),
    Fact(
        C["cv8000"],
        "a01_mito_mean",
        "pooled mean of every plane file of well A01 in the channel whose .mes target is 'Mitotracker deep red'",
        lambda: __import__("analysis").hcs_cv_well_mean(C["cv8000"], 1, 1, "Mitotracker deep red"),
    ),
    # ---- held-out additions 2026-09-24: newer analysis commands
    Fact(
        C["lcd"],
        "rid_tallest_rt",
        "retention time (min) of the tallest peak of the refractive-index detector (Detector B)",
        lambda: lcd_tallest_rt(C["lcd"], "Detector B"),
        lambda: lcd_tallest_rt(C["lcd"], "Detector B-Ch1", from_trace=True),
        rel=1e-3,
    ),
    Fact(
        C["lcd"],
        "uv2_second_area_rt",
        "retention time (min) of the second-largest peak by area in UV channel 2 (Detector A-Ch2), vendor integration",
        lambda: lcd_nth_area_rt(C["lcd"], "Detector A-Ch2", 2),
    ),
    Fact(
        C["mzmlgz"],
        "ms1_tic_apex_min",
        "retention time (min) of the MS1 spectrum with the largest summed intensity",
        lambda: gz_ms1_tic_apex(C["mzmlgz"]),
        lambda: gz_ms1_tic_apex(C["mzmlgz"], from_cvparam=True),
    ),
    Fact(
        C["mzmlb"],
        "first_precursor_mz",
        "selected-ion m/z of the first spectrum's precursor",
        lambda: mzmlb_first_precursor(C["mzmlb"]),
    ),
    Fact(
        C["gen5bca"],
        "spl3_conc",
        "mean concentration of sample SPL3 (wells A9, B9) from the linear standard curve",
        lambda: gen5_sample_conc(C["gen5bca"], ["A9", "B9"]),
        lambda: gen5_sample_conc(C["gen5bca"], ["A9", "B9"], refit=True),
        rel=0.005,
    ),
    Fact(
        C["fid600"],
        "tallest_ppm_6_10",
        "chemical shift of the highest point of TopSpin's processed pdata/1/1r between 6 and 10 ppm",
        lambda: bruker_tallest_ppm_window(C["fid600"], 6.0, 10.0),
        lambda: bruker_tallest_ppm_window(C["fid600"], 6.0, 10.0, raw=True),
    ),
    Fact(
        C["abfsteps"],
        "rheobase_pa",
        "smallest current step (pA) whose step window holds an upward crossing of 0 mV",
        lambda: __import__("analysis").abf_rheobase_numpy(C["abfsteps"]),
        lambda: abf_depositor_rheobase(C["abfsteps"], "19n26010"),
    ),
    Fact(
        C["abfsteps"],
        "last_sweep_ap_count",
        "upward crossings of 0 mV in the last sweep (the largest step)",
        lambda: __import__("analysis").abf_ap_count_numpy(C["abfsteps"], 22),
        lambda: abf_last_sweep_ap_count_neo(C["abfsteps"]),
    ),
    Fact(
        C["svsjp2k"],
        "level0_mean",
        "mean of every R, G, B sample of the full-resolution level (JPEG 2000 tiles)",
        lambda: __import__("analysis").svs_level0_mean(C["svsjp2k"]),
        lambda: __import__("analysis").svs_level0_mean_cross(C["svsjp2k"]),
        rel=2e-3,
    ),
    Fact(
        C["svsjp2k"],
        "region_red_mean",
        "mean of the red sample of the full-resolution region x 2000-2511, y 2000-2511",
        lambda: svs_region_mean(C["svsjp2k"], 2000, 2000, 512, 512, 0),
        lambda: svs_region_mean(C["svsjp2k"], 2000, 2000, 512, 512, 0, openslide=True),
        rel=2e-3,
    ),
]


def compute() -> dict:
    import analysis as a
    import facts as fx

    dev_path = a.path
    a.path = hpath  # analysis.py's readers, on held-out files (restored after: other callers share the module)
    try:
        return _compute(a, fx)
    finally:
        a.path = dev_path


def _compute(a, fx) -> dict:
    out: dict = {}
    for f in FACTS:
        value, reader = f.compute()
        value = a.tidy(value)
        rec: dict[str, Any] = {"value": value, "reader": reader, "how": f.how}
        if f.cross:
            other, other_reader = f.cross()
            if not a.agree(value, a.tidy(other), rel=f.rel):
                raise SystemExit(f"{f.corpus_id} {f.name}: {reader} says {value!r}, {other_reader} says {other!r}")
            rec["cross_check"] = f"{other_reader}: agrees"
        entry = out.setdefault(
            f.corpus_id,
            {"file": fx.manifest_file(f.corpus_id, f.role), "extractor": "evals/heldout.py", "facts": {}},
        )
        entry["facts"][f.name] = rec
        print(f"{f.corpus_id} {f.name} = {value!r}", file=sys.stderr)
    return out


def render(data: dict) -> str:
    return json.dumps(data, indent=1, ensure_ascii=False, sort_keys=True) + "\n"


# ---------------------------------------------------------------- questions

HINT_RAW = "a number (raw values as stored in the file)"


def o_img(o: dict, key: str, image: int = 0):
    return o["images"][image][key]


def o_um(o: dict, axis: str = "x", image: int = 0) -> float:
    return o["images"][image]["physical_size_um"][axis]


def nmr_extra(o: dict, key: str, trace: int = 0):
    return o["traces"][trace]["parameters"]["extra"][key]


def acqus(o: dict, key: str):
    return o["traces"][0]["parameters"]["acqus"][key]


def spec(qid, cid, category, question, answer, source, **kw) -> g.Spec:
    return g.Spec(qid, C[cid], category, question, answer, source, **kw)


def fact(name: str, build: Callable[[Any], dict]) -> Callable[[dict, dict], dict]:
    return lambda f, m: build(f[name])


SPECS: list[g.Spec] = [
    # ------------------------------------------------------------ microscopy: lookups
    spec(
        "ho-mic-czi-xzt-timepoints",
        "xzt",
        "dimensions",
        "This confocal line-scan acquisition was repeated over time. How many time points does it contain?",
        lambda o, m: g.integer(o_img(o, "size_t")),
        "oracle: /images/0/size_t (czifile)",
    ),
    spec(
        "ho-mic-czi-axioscan-scenes",
        "axioscan",
        "dimensions",
        "This slide-scanner file holds several scenes (separately scanned regions). How many?",
        lambda o, m: g.integer(len(o["images"])),
        "oracle: len(/images) (czifile: one image per scene)",
    ),
    spec(
        "ho-mic-czi-airyscan-pixel",
        "airyscan",
        "pixel-size",
        "What is the pixel size of this Airyscan image?",
        lambda o, m: g.number(o_um(o), "µm", rel=0.01),
        "oracle: /images/0/physical_size_um/x (czifile); the depositor states 44 nm",
        answer_hint="a number with its unit",
    ),
    spec(
        "ho-mic-czi-lsm780-channels",
        "lsm780",
        "channels",
        "How many channels were recorded in this image?",
        lambda o, m: g.integer(o_img(o, "size_c")),
        "oracle: /images/0/size_c (czifile)",
    ),
    spec(
        "ho-mic-nd2-rbc-timepoints",
        "rbc",
        "dimensions",
        "How many frames (time points) are in this time-lapse movie?",
        lambda o, m: g.integer(o_img(o, "size_t")),
        "oracle: /images/0/size_t (nd2)",
    ),
    spec(
        "ho-mic-nd2-c2plus-channels",
        "c2plus",
        "channels",
        "What are the names of the channels in this confocal stack?",
        lambda o, m: g.items(o_img(o, "channel_names")),
        "oracle: /images/0/channel_names (nd2)",
        answer_hint="the channel names, comma-separated",
    ),
    spec(
        "ho-mic-nd2-rgb-pixel",
        "rgb",
        "pixel-size",
        "This is a colour camera image of an H&E section. What is its pixel size in micrometres?",
        lambda o, m: g.number(o_um(o), "µm", rel=0.01),
        "oracle: /images/0/physical_size_um/x (nd2)",
        answer_hint="a number with its unit",
    ),
    spec(
        "ho-mic-lif-stellaris-pixel",
        "stellaris",
        "pixel-size",
        "What is the pixel size of the first image in this file, in micrometres?",
        lambda o, m: g.number(o_um(o), "µm", rel=0.01),
        "oracle: /images/0/physical_size_um/x (liffile)",
        answer_hint="a number with its unit",
    ),
    spec(
        "ho-mic-ometiff-prostate-size",
        "prostate",
        "dimensions",
        "What are the width and height of the full-resolution image, in pixels?",
        lambda o, m: g.items([str(o_img(o, "size_x")), str(o_img(o, "size_y"))]),
        "oracle: /images/0/size_x and size_y (tifffile)",
        answer_hint="width × height in pixels",
    ),
    spec(
        "ho-mic-ndpi-pixel",
        "ndpi",
        "pixel-size",
        "This is a fluorescence slide scan. What is its resolution in microns per pixel at full resolution?",
        lambda o, m: g.number(o_um(o), "µm", rel=0.01),
        "oracle: /images/0/physical_size_um/x (tifffile)",
        answer_hint="a number with its unit",
    ),
    spec(
        "ho-mic-ometiff-zen-pixel",
        "zenexport",
        "pixel-size",
        "What is the pixel size of this super-resolution image?",
        lambda o, m: g.number(o_um(o), "µm", rel=0.01),
        "oracle: /images/0/physical_size_um/x (tifffile)",
        answer_hint="a number with its unit",
    ),
    spec(
        "ho-mic-oir-date",
        "oir",
        "acquisition-time",
        "On what date was this image acquired?",
        lambda o, m: g.date(o["datetime"][:10], tolerance_days=1),
        "oracle: /datetime (oirfile; UTC)",
    ),
    spec(
        "ho-mic-oib-zstep",
        "oib",
        "pixel-size",
        "What was the z-step between the optical sections of this stack?",
        lambda o, m: g.number(o_um(o, "z"), "µm", rel=0.02),
        "oracle: /images/0/physical_size_um/z (Bio-Formats; oiffile agrees)",
        answer_hint="a number with its unit",
    ),
    spec(
        "ho-mic-svs-mpp",
        "svs",
        "pixel-size",
        "This is a scanned slide. What is its resolution in microns per pixel?",
        lambda o, m: g.number(o_um(o), "µm", rel=0.01),
        "oracle: /images/0/physical_size_um/x (tifffile)",
        answer_hint="a number with its unit",
    ),
    spec(
        "ho-mic-ims-channel",
        "ims",
        "channels",
        "What is the name of the channel in this image?",
        lambda o, m: g.string(o["images"][0]["channel_names"][0], ["488 GFP CF40 Zyla"]),
        "oracle: /images/0/channel_names/0 (h5py)",
    ),
    spec(
        "ho-mic-zvi-channels",
        "zvi",
        "channels",
        "How many fluorescence channels were recorded in this image?",
        lambda o, m: g.integer(o_img(o, "size_c")),
        "oracle: /images/0/size_c (Bio-Formats)",
    ),
    # ------------------------------------------------------------ microscopy: analysis
    spec(
        "ho-ana-mic-nd2-rgb-red-mean",
        "rgb",
        "analysis",
        "What is the mean value of the red channel of this RGB image (0-255 scale)?",
        fact("mean_red", lambda v: g.number(v, None, rel=0.005)),
        "facts-heldout: mean_red (tifffile on NIS-Elements' OME-TIFF export; nd2 agrees in its B,G,R sample order)",
        answer_hint=HINT_RAW,
    ),
    spec(
        "ho-ana-mic-nd2-nsparc-brightest",
        "nsparc",
        "analysis",
        "Which channel has the highest mean intensity? Answer with the channel name.",
        fact(
            "brightest_channel",
            lambda v: g.string(v, reject=[n for n in ("DAPI", "AF488", "AF555", "AF647") if n != v]),
        ),
        "facts-heldout: brightest_channel (nd2)",
        answer_hint="the channel name",
    ),
    spec(
        "ho-ana-mic-lif-sp8-brightest-series",
        "sp8",
        "analysis",
        "Which image series has the highest mean intensity in the first channel (DAPI)? Answer with the series name.",
        fact("brightest_series_c1", lambda v: g.string(v)),
        "facts-heldout: brightest_series_c1 (liffile; readlif agrees)",
        answer_hint="the series name",
    ),
    spec(
        "ho-ana-mic-lif-sp5-dimmest",
        "sp5",
        "analysis",
        "Which frame of the time series has the lowest mean intensity (counting frames from 1)?",
        fact("dimmest_t", g.integer),
        "facts-heldout: dimmest_t (liffile; readlif agrees)",
        answer_hint="the frame number",
    ),
    # ------------------------------------------------------------ electron microscopy
    spec(
        "ho-em-mrc-cryosparc-voxel",
        "cryosparc",
        "pixel-size",
        "What is the voxel size of this cryo-EM map, in ångström?",
        lambda o, m: g.number(o["images"][0]["mrcfile_voxel_size_angstrom"][0], "Å", rel=0.005),
        "oracle: /images/0/mrcfile_voxel_size_angstrom/0 (mrcfile)",
        answer_hint="a number with its unit",
    ),
    spec(
        "ho-em-mrc-tomo-slices",
        "tomo",
        "dimensions",
        "How many slices (sections along z) does this tomogram have?",
        lambda o, m: g.integer(o_img(o, "size_z")),
        "oracle: /images/0/size_z (mrcfile)",
    ),
    spec(
        "ho-em-dm3-diff-size",
        "dm3",
        "dimensions",
        "What is the size of this diffraction pattern in pixels (width × height)?",
        lambda o, m: g.items([str(o_img(o, "size_x")), str(o_img(o, "size_y"))]),
        "oracle: /images/0/size_x and size_y (dm3_lib)",
        answer_hint="width × height in pixels",
    ),
    spec(
        "ho-em-dm4-eels-channels",
        "eels",
        "dimensions",
        "This is an EELS spectrum image. How many energy channels does each spectrum have?",
        lambda o, m: g.integer(o_img(o, "size_t")),
        "oracle: /images/0/size_t (dm3_lib: energy axis)",
    ),
    spec(
        "ho-em-ser-magnification",
        "ser",
        "instrument",
        "At what nominal magnification was this recorded?",
        lambda o, m: g.number(o["emi"]["Magnification [x]"], None, rel=0.001),
        "oracle: /emi/Magnification [x] (ncempy reading the .emi next to the .ser)",
        answer_hint="the magnification",
        stage_as="sample_1.ser",
        extra=[("ho-zenodo1486742-emi-stemdiff", "sample.emi")],
    ),
    spec(
        "ho-em-emd-detectors",
        "velox",
        "channels",
        "How many detector images (e.g. HAADF, BF) were recorded simultaneously in this file?",
        lambda o, m: g.integer(len(o["images"])),
        "oracle: len(/images) (h5py: Data/Image groups)",
    ),
    spec(
        "ho-ana-em-mrc-cryosparc-max",
        "cryosparc",
        "analysis",
        "What is the highest density value in this map (raw values as stored)?",
        fact("max_density", lambda v: g.number(v, None, rel=0.001)),
        "facts-heldout: max_density (mrcfile; NumPy on the MRC2014 layout agrees)",
        answer_hint=HINT_RAW,
    ),
    spec(
        "ho-ana-em-mrc-tomo-mean",
        "tomo",
        "analysis",
        "What is the mean voxel value of the whole tomogram (raw values as stored)?",
        fact("mean_voxel", lambda v: g.number(v, None, rel=0.005)),
        "facts-heldout: mean_voxel (mrcfile; NumPy on the MRC2014 layout agrees)",
        answer_hint=HINT_RAW,
    ),
    # ------------------------------------------------------------ flow cytometry
    spec(
        "ho-flow-fcs-attune-cd8",
        "attune",
        "channels",
        "Which detector channel (parameter name, e.g. XX-A) carries the CD8 stain?",
        lambda o, m: g.string(
            o["tables"][0]["parameter_names"][o["tables"][0]["parameter_labels"].index("CD8-PerCP-Cy5.5-A")]
        ),
        "oracle: /tables/0/parameter_names at the index of parameter_labels 'CD8-PerCP-Cy5.5-A' (flowio)",
        answer_hint="the parameter name",
    ),
    spec(
        "ho-flow-fcs-moflo-parameters",
        "moflo",
        "counts",
        "How many parameters (columns) does each event have?",
        lambda o, m: g.integer(o["tables"][0]["parameter_count"]),
        "oracle: /tables/0/parameter_count (flowio)",
    ),
    spec(
        "ho-flow-fcs-novocyte-events",
        "novocyte",
        "counts",
        "How many events are in this compensation control?",
        lambda o, m: g.integer(o["tables"][0]["event_count"]),
        "oracle: /tables/0/event_count (flowio; fcsparser agrees)",
    ),
    spec(
        "ho-ana-flow-fcs-attune-median",
        "attune",
        "analysis",
        "What is the median of the BL1-A parameter (CX3CR1-FITC) over all events, raw values as stored?",
        fact("median_bl1_a", lambda v: g.number(v, None, rel=0.01)),
        "facts-heldout: median_bl1_a (flowio; fcsparser agrees)",
        answer_hint=HINT_RAW,
    ),
    spec(
        "ho-ana-flow-fcs-novocyte-median",
        "novocyte",
        "analysis",
        "What is the median of the B525-A parameter (AF488 - Puromycin) over all events, raw values as stored?",
        fact("median_b525_a", lambda v: g.number(v, None, rel=0.01)),
        "facts-heldout: median_b525_a (flowio; fcsparser agrees)",
        answer_hint=HINT_RAW,
    ),
    spec(
        "ho-ana-flow-fcs-melody-median",
        "melody",
        "analysis",
        "What is the median side-scatter area (SSC-A) over all events, raw values as stored?",
        fact("median_ssc_a", lambda v: g.number(v, None, rel=0.01)),
        "facts-heldout: median_ssc_a (flowio; fcsparser agrees)",
        answer_hint=HINT_RAW,
    ),
    # ------------------------------------------------------------ electrophysiology
    spec(
        "ho-ephys-abf2-iclamp-units",
        "iclamp",
        "channels",
        "What are the units of the two recorded input channels?",
        lambda o, m: g.items(
            o["traces"][0]["channel_units"],
            {"mV": ["millivolt", "millivolts"], "pA": ["picoamp", "picoampere", "picoamperes"]},
        ),
        "oracle: /traces/0/channel_units (pyabf)",
        answer_hint="the two units, comma-separated",
    ),
    spec(
        "ho-ephys-abf2-episodic-sweeps",
        "episodic",
        "counts",
        "How many sweeps does this recording contain?",
        lambda o, m: g.integer(o["traces"][0]["sweep_count"]),
        "oracle: /traces/0/sweep_count (pyabf)",
    ),
    spec(
        "ho-ephys-nwb-date",
        "nwb",
        "acquisition-time",
        "When did this recording session start (date)?",
        lambda o, m: g.date(o["session"]["session_start_time"][:10], tolerance_days=1),
        "oracle: /session/session_start_time (h5py)",
    ),
    spec(
        "ho-ana-ephys-abf2-iclamp-peak",
        "iclamp",
        "analysis",
        "What is the most positive membrane potential reached in the voltage channel (IN 0), in mV?",
        fact("peak_mv", lambda v: g.number(v, "mV", abs_=0.05)),
        "facts-heldout: peak_mv (pyabf; neo AxonRawIO agrees)",
        answer_hint="a number with its unit",
    ),
    spec(
        "ho-ana-ephys-abf2-episodic-sweep",
        "episodic",
        "analysis",
        "Which sweep (counting from 1) contains the largest inward current, i.e. the most negative value?",
        fact("most_negative_sweep", g.integer),
        "facts-heldout: most_negative_sweep (pyabf; neo AxonRawIO agrees)",
        answer_hint="the sweep number",
    ),
    spec(
        "ho-ana-ephys-abf1-min",
        "abf1",
        "analysis",
        "What is the most negative current in the whole recording, in pA?",
        fact("min_pa", lambda v: g.number(v, "pA", abs_=0.05)),
        "facts-heldout: min_pa (pyabf; neo AxonRawIO agrees)",
        answer_hint="a number with its unit",
    ),
    # ------------------------------------------------------------ NMR
    spec(
        "ho-nmr-bruker-hsqc1200-pulprog",
        "hsqc1200",
        "method",
        "Which pulse program was used for this experiment?",
        lambda o, m: g.string(acqus(o, "PULPROG")),
        "oracle: /traces/0/parameters/acqus/PULPROG (nmrglue)",
        answer_hint="the pulse program name",
    ),
    spec(
        "ho-nmr-bruker-hsqc900-solvent",
        "hsqc900",
        "instrument",
        "What solvent was the sample in?",
        lambda o, m: g.string(acqus(o, "SOLVENT"), ["methanol-d4", "CD3OD", "deuterated methanol", "methanol"]),
        "oracle: /traces/0/parameters/acqus/SOLVENT (nmrglue)",
        answer_hint="the solvent",
    ),
    spec(
        "ho-nmr-jeol-1h-frequency",
        "jeol1h",
        "instrument",
        "What was the spectrometer frequency for this 1H spectrum, in MHz?",
        lambda o, m: g.number(nmr_extra(o, "spectrometer_frequency_mhz"), "MHz", rel=0.001),
        "oracle: /traces/0/parameters/extra/spectrometer_frequency_mhz (nmrglue jeol)",
        answer_hint="a number with its unit",
    ),
    spec(
        "ho-nmr-jeol-hsqc-pulprog",
        "jeolhsqc",
        "method",
        "Which pulse sequence (experiment file) was used?",
        lambda o, m: g.string(nmr_extra(o, "pulse_program"), [nmr_extra(o, "pulse_program").rsplit(".", 1)[0]]),
        "oracle: /traces/0/parameters/extra/pulse_program (nmrglue jeol)",
        answer_hint="the pulse sequence name",
    ),
    spec(
        "ho-nmr-varian-scans",
        "varian",
        "counts",
        "How many transients (scans) were averaged in this FID?",
        lambda o, m: g.integer(nmr_extra(o, "scans")),
        "oracle: /traces/0/parameters/extra/scans (nmrglue varian, procpar nt)",
    ),
    spec(
        "ho-spec-jcamp-ir-points",
        "ir",
        "counts",
        "How many data points does this infrared spectrum have?",
        lambda o, m: g.integer(o["traces"][0]["sample_count"]),
        "oracle: /traces/0/sample_count (jcamp)",
    ),
    spec(
        "ho-ana-nmr-bruker-1h-tallest",
        "bruker1h",
        "analysis",
        "In the processed 1H spectrum (processing number 1), at what chemical shift is the tallest peak?",
        fact("tallest_ppm", lambda v: g.number(v, None, abs_=0.01)),
        "facts-heldout: tallest_ppm (nmrglue; NumPy on 1r agrees)",
        answer_hint="the chemical shift in ppm",
    ),
    # ------------------------------------------------------------ mass spectrometry
    spec(
        "ho-ms-raw-ltqxl-first-precursor",
        "ltqxl",
        "values",
        "What is the precursor m/z of the first MS/MS scan in this run?",
        lambda o, m: g.number(
            next(s["precursor_mz"] for s in o["spectra"]["scans"] if s["ms_level"] == 2), None, abs_=0.01
        ),
        "oracle: first ms_level 2 scan's precursor_mz (pyteomics on the depositor's mzXML)",
        answer_hint="the m/z",
    ),
    spec(
        "ho-ms-raw-tsq-scans",
        "tsq",
        "counts",
        "How many scans does this triple-quadrupole SRM run contain?",
        lambda o, m: g.integer(o["spectra"]["scan_count"]),
        "oracle: /spectra/scan_count (pyteomics on the depositor's mzXML)",
    ),
    spec(
        "ho-ms-raw-prm-collision",
        "prm",
        "method",
        "What normalized collision energy was used for the fragmentation scans?",
        lambda o, m: g.number(o["spectra"]["scans"][0]["collision_energy"], None, abs_=0.5),
        "oracle: /spectra/scans/0/collision_energy (pyteomics on the depositor's mzXML)",
        answer_hint="the collision energy",
    ),
    spec(
        "ho-ana-ms-raw-exploris-tic",
        "exploris",
        "analysis",
        "At what retention time (minutes) does the total ion chromatogram of this run reach its maximum?",
        fact("tic_apex_min", lambda v: g.number(v, "min", abs_=0.1)),
        "facts-heldout: tic_apex_min (pyteomics on the depositor's mzML; TIC cvParams agree)",
        answer_hint="a number with its unit",
    ),
    spec(
        "ho-ana-ms-mzml-tt5600-tic",
        "tt5600",
        "analysis",
        "At what retention time (minutes) does the total ion chromatogram reach its maximum?",
        fact("tic_apex_min", lambda v: g.number(v, "min", abs_=0.02)),
        "facts-heldout: tic_apex_min (pyteomics; TIC cvParams agree)",
        answer_hint="a number with its unit",
    ),
    spec(
        "ho-ana-ms-mzml-maldi-basepeak",
        "maldi",
        "analysis",
        "What is the m/z of the most intense point in this MALDI spectrum?",
        fact("base_peak_mz", lambda v: g.number(v, None, abs_=0.05)),
        "facts-heldout: base_peak_mz (pyteomics)",
        answer_hint="the m/z",
    ),
    spec(
        "ho-ana-ms-raw-tsq-precursors",
        "tsq",
        "analysis",
        "How many distinct precursor m/z values (targets) were monitored in this SRM run?",
        fact("distinct_precursors", g.integer),
        "facts-heldout: distinct_precursors (pyteomics on the depositor's mzXML)",
        answer_hint="the number of precursors",
    ),
    spec(
        "ho-ana-ms-raw-prm-precursors",
        "prm",
        "analysis",
        "How many distinct precursor m/z values were targeted in this PRM run?",
        fact("distinct_precursors", g.integer),
        "facts-heldout: distinct_precursors (pyteomics on the depositor's mzXML)",
        answer_hint="the number of precursors",
    ),
    # ------------------------------------------------------------ chromatography
    spec(
        "ho-chrom-cs-dad-wavelength",
        "dad",
        "channels",
        "At what detection wavelength was this diode-array signal recorded?",
        fact("signal_nm", lambda v: g.number(v, "nm", abs_=0.5)),
        "facts-heldout: signal_nm (rainbow-api: the `Sig=` wavelength of the file's signal description)",
        answer_hint="a number with its unit",
    ),
    spec(
        "ho-ana-chrom-cs-fid-apex",
        "fid",
        "analysis",
        "At what retention time (minutes) is the highest point of this GC-FID chromatogram?",
        fact("apex_min", lambda v: g.number(v, "min", abs_=0.01)),
        "facts-heldout: apex_min (rainbow-api; the depositor's .xy export agrees)",
        answer_hint="a number with its unit",
    ),
    spec(
        "ho-ana-chrom-cs-dad-apex",
        "dad",
        "analysis",
        "At what retention time (minutes) does this UV chromatogram reach its maximum?",
        fact("apex_min", lambda v: g.number(v, "min", abs_=0.01)),
        "facts-heldout: apex_min (rainbow-api)",
        answer_hint="a number with its unit",
    ),
    spec(
        "ho-ana-chrom-cs-gcms-tic",
        "gcms",
        "analysis",
        "At what retention time (minutes) does the total ion chromatogram reach its maximum?",
        fact("tic_apex_min", lambda v: g.number(v, "min", abs_=0.02)),
        "facts-heldout: tic_apex_min (Aston; rainbow-api agrees)",
        answer_hint="a number with its unit",
    ),
    spec(
        "ho-ana-chrom-andi-largest",
        "andi",
        "analysis",
        "Which peak has the largest area? Give its retention time in minutes.",
        fact("largest_peak_min", lambda v: g.number(v, "min", abs_=0.02)),
        "facts-heldout: largest_peak_min (scipy netCDF: the vendor's peak table)",
        answer_hint="a number with its unit",
    ),
    # ------------------------------------------------------------ plates
    spec(
        "ho-ana-plate-softmax-top",
        "softmax",
        "analysis",
        "Which well has the highest absorbance at 450 nm?",
        fact("top_well_450", lambda v: g.string(v)),
        "facts-heldout: top_well_450 (allotropy)",
        answer_hint="the well, e.g. C7",
    ),
    spec(
        "ho-ana-plate-gen5-top",
        "gen5",
        "analysis",
        "Which well gave the highest absorbance in the first 660 nm read?",
        fact("top_well", lambda v: g.string(v)),
        "facts-heldout: top_well (allotropy)",
        answer_hint="the well, e.g. C7",
    ),
    # ------------------------------------------------------------ vibrational spectroscopy (added 2026-09-24)
    spec(
        "ho-spec-opus-vertex70-instrument",
        "opus70",
        "instrument",
        "Which FT-IR spectrometer model recorded this spectrum?",
        lambda o, m: g.string(
            o["traces"][0]["parameters"]["extra"]["instrument"], ["Vertex 70", "Bruker VERTEX 70", "Bruker Vertex 70"]
        ),
        "oracle: /traces/0/parameters/extra/instrument (brukeropus)",
        answer_hint="the instrument model",
    ),
    spec(
        "ho-ana-spec-opus-vertex70-band",
        "opus70",
        "analysis",
        "What is the transmittance of this sample at 1000 cm-1, as a fraction (0-1) the way the file stores it?",
        fact("t_at_1000", lambda v: g.number(v, None, rel=0.01)),
        "facts-heldout: t_at_1000 (brukeropus; the depositor's OPUS .dpt export agrees)",
        answer_hint="a number",
    ),
    spec(
        "ho-ana-spec-opus-alpha-band",
        "opusalpha",
        "analysis",
        "In the result spectrum of this ATR measurement (the processed spectrum, not the single channels), at "
        "what wavenumber between 600 and 1800 cm-1 is the value lowest (the strongest absorption band)?",
        fact("min_result_600_1800", lambda v: g.number(v, None, abs_=3.0)),
        "facts-heldout: min_result_600_1800 (brukeropus; the depositor's OPUS .dpt export agrees)",
        answer_hint="the wavenumber in cm-1",
    ),
    spec(
        "ho-ana-spec-omnic-spa-band",
        "spa",
        "analysis",
        "At what wavenumber is the strongest absorption band of this spectrum (its lowest %T)?",
        fact("min_t", lambda v: g.number(v, None, abs_=3.0)),
        "facts-heldout: min_t (SpectroChemPy; the depositor's OMNIC CSV export agrees)",
        answer_hint="the wavenumber in cm-1",
    ),
    spec(
        "ho-spec-omnic-spg-count",
        "spg",
        "counts",
        "How many spectra does this OMNIC spectral group file contain?",
        lambda o, m: g.integer(o["traces"][0]["sweep_count"]),
        "oracle: /traces/0/sweep_count (SpectroChemPy)",
    ),
    spec(
        "ho-ana-spec-omnic-spg-amide",
        "spg",
        "analysis",
        "Which spectrum of this group (counting from 1, in file order) has the highest absorbance maximum "
        "between 1600 and 1700 cm-1 (the amide I region)?",
        fact("max_amide1_spectrum", g.integer),
        "facts-heldout: max_amide1_spectrum (SpectroChemPy)",
        answer_hint="the spectrum number",
    ),
    spec(
        "ho-spec-wdf-map-shape",
        "wdfmap",
        "dimensions",
        "This is a Raman map. How many points does the map have along x and along y?",
        lambda o, m: g.items([str(o_img(o, "size_x")), str(o_img(o, "size_y"))]),
        "oracle: /images/0/size_x and size_y (renishawWiRE map shape)",
        answer_hint="points along x × points along y",
    ),
    spec(
        "ho-ana-spec-wdf-map-mean-band",
        "wdfmap",
        "analysis",
        "Average all spectra of this Raman map. At what Raman shift is the tallest point of the average spectrum?",
        fact("mean_spectrum_band", lambda v: g.number(v, None, abs_=4.0)),
        "facts-heldout: mean_spectrum_band (renishawWiRE; SpectroChemPy read_wdf agrees)",
        answer_hint="the Raman shift in cm-1",
    ),
    spec(
        "ho-ana-spec-wdf-single-band",
        "wdf1",
        "analysis",
        "At what Raman shift is the strongest band of this spectrum?",
        fact("tallest_band", lambda v: g.number(v, None, abs_=2.0)),
        "facts-heldout: tallest_band (renishawWiRE; the depositor's WiRE text export agrees)",
        answer_hint="the Raman shift in cm-1",
    ),
    spec(
        "ho-spec-pesp-detector",
        "pesp",
        "instrument",
        "Which detector was used to record this FT-IR spectrum?",
        lambda o, m: g.string(o["traces"][0]["parameters"]["extra"]["detector"], ["MCT detector", "HgCdTe"]),
        "oracle: /traces/0/parameters/extra/detector (specio)",
        answer_hint="the detector",
    ),
    spec(
        "ho-ana-spec-pesp-band",
        "pesp",
        "analysis",
        "At what wavenumber is the strongest absorption band of this spectrum (its lowest %T)?",
        fact("min_t", lambda v: g.number(v, None, abs_=3.0)),
        "facts-heldout: min_t (specio; the depositor's CSV export agrees)",
        answer_hint="the wavenumber in cm-1",
    ),
    # ------------------------------------------------------------ qPCR (added 2026-09-24)
    spec(
        "ho-qpcr-eds-qs6-tm",
        "qs6",
        "values",
        "What melting temperature (Tm) did the instrument software report for well A10?",
        fact("tm_a10", lambda v: g.number(v, "°C", abs_=0.1)),
        "facts-heldout: tm_a10 (the vendor's melt results inside the .eds; the vendor's .xls export agrees)",
        answer_hint="a number with its unit",
    ),
    spec(
        "ho-ana-qpcr-eds-qs6-highest-ct",
        "qs6",
        "analysis",
        "Several assays share this plate. Which target has the highest mean Ct over its wells that have a Ct?",
        fact("highest_mean_ct_target", lambda v: g.string(v)),
        "facts-heldout: highest_mean_ct_target (vendor Ct inside the .eds; the vendor's .xls export agrees)",
        answer_hint="the target name",
    ),
    spec(
        "ho-qpcr-eds-viia7-undetermined",
        "viia7",
        "counts",
        "How many wells of the CB1 assay (the no-template control included) gave no Ct (Undetermined)?",
        fact("cb1_undetermined", g.integer),
        "facts-heldout: cb1_undetermined (vendor results inside the .eds; the vendor's .xls export agrees)",
        answer_hint="the number of wells",
    ),
    spec(
        "ho-ana-qpcr-eds-viia7-rq",
        "viia7",
        "analysis",
        "Using S18 as the reference gene and sample 'SHSY5Y (S4A)' as the calibrator, what is the relative "
        "quantity (RQ = 2^-ΔΔCt) of CB1 in sample 'SSC-9 high'? Use the mean Ct of the wells that have a Ct.",
        fact("rq_cb1_ssc9_high", lambda v: g.number(v, None, rel=0.01)),
        "facts-heldout: rq_cb1_ssc9_high (the vendor's RQ inside the .eds; the vendor's .xls export agrees)",
        answer_hint="a number",
    ),
    spec(
        "ho-ana-qpcr-rex-earliest",
        "rex",
        "analysis",
        "The tubes are named after the gene they measure. Leaving out 18s RNA, which gene amplifies earliest "
        "(lowest mean Ct in the Green cycling channel)?",
        fact("earliest_gene", lambda v: g.string(v)),
        "facts-heldout: earliest_gene (the Rotor-Gene Q software's analysed-data export; not given to the agent)",
        answer_hint="the gene name",
    ),
    spec(
        "ho-ana-qpcr-rex-latest",
        "rex",
        "analysis",
        "The tubes are named after the gene they measure. Which gene amplifies latest (highest mean Ct in the "
        "Green cycling channel), not counting the no-template controls?",
        fact("latest_gene", lambda v: g.string(v)),
        "facts-heldout: latest_gene (the Rotor-Gene Q software's analysed-data export; not given to the agent)",
        answer_hint="the gene name",
    ),
    spec(
        "ho-qpcr-rdml-a1-cq",
        "rdml",
        "values",
        "What Cq value is stored for well A1?",
        lambda o, m: g.number(next(r["cq"] for r in o["records"] if r["row"] == 1 and r["col"] == 1), None, abs_=0.01),
        "oracle: /records (row 1, column 1) cq (rdmlpython)",
        answer_hint="the Cq",
    ),
    spec(
        "ho-ana-qpcr-rdml-highest-cq",
        "rdml",
        "analysis",
        "Which target has the highest mean Cq over its reactions that have a Cq?",
        fact("highest_mean_cq_target", lambda v: g.string(v)),
        "facts-heldout: highest_mean_cq_target (rdmlpython; ElementTree over the XML agrees)",
        answer_hint="the target name",
    ),
    # ------------------------------------------------------------ high-content screening (added 2026-09-24)
    spec(
        "ho-hcs-harmony6-channels",
        "harmony6",
        "channels",
        "This folder is a high-content screening plate exported from the imager. Which channels were acquired? "
        "Give the channel names.",
        lambda o, m: g.items(o["hcs"]["bioformats"]["channels"]),
        "oracle: /hcs/bioformats/channels (Bio-Formats 8.5, black box)",
        stage_as="sample_plate",
        answer_hint="the channel names, comma-separated",
    ),
    spec(
        "ho-ana-hcs-harmony6-brighter-well",
        "harmony6",
        "analysis",
        "This copy of the plate holds the images of wells A01 and A02 only. Which of the two wells has the higher "
        "mean intensity in the WG-Phalloidin channel, pooled over all its fields?",
        fact(
            "brighter_well_phalloidin",
            lambda v: g.string(v, [v.replace("0", "", 1)], reject=["A01" if v == "A02" else "A02"]),
        ),
        "facts-heldout: brighter_well_phalloidin (tifffile on the plane files)",
        stage_as="sample_plate",
        answer_hint="the well, e.g. B03",
    ),
    spec(
        "ho-hcs-harmony5-pixel",
        "harmony5",
        "pixel-size",
        "This folder is a high-content screening plate. What is the pixel size of its images, in micrometres?",
        lambda o, m: g.number(o["hcs"]["pixel_size_um"][0], "µm", rel=0.005),
        "oracle: /hcs/pixel_size_um/0 (Index.idx.xml parsed with the Python standard library; Bio-Formats agrees)",
        stage_as="sample_plate",
        answer_hint="a number with its unit",
    ),
    spec(
        "ho-ana-hcs-harmony5-field",
        "harmony5",
        "analysis",
        "This copy of the plate holds well A01 only. Which field of view of A01 (counting from 1, as the plate "
        "index numbers them) has the highest mean intensity in the MitoTracker Deep Red channel?",
        fact("brightest_field_mito_a01", g.integer),
        "facts-heldout: brightest_field_mito_a01 (tifffile on the planes Index.idx.xml names)",
        stage_as="sample_plate",
        answer_hint="the field number",
    ),
    spec(
        "ho-hcs-ix-sites",
        "ixbia",
        "counts",
        "This folder is an ImageXpress plate (only some wells were copied). How many sites (fields of view) "
        "were imaged per well?",
        lambda o, m: g.integer(max(len(w["images"]) for w in o["hcs"]["wells"])),
        "oracle: /hcs/wells/*/images, largest count (the .HTD parsed as text)",
        stage_as="sample_plate",
        answer_hint="the number of sites",
    ),
    spec(
        "ho-ana-hcs-ix-site",
        "ixbia",
        "analysis",
        "In well B02, which site (counting from 1) has the highest mean intensity in the DAPI channel?",
        fact("brightest_site_dapi_b02", g.integer),
        "facts-heldout: brightest_site_dapi_b02 (tifffile)",
        stage_as="sample_plate",
        answer_hint="the site number",
    ),
    spec(
        "ho-hcs-cv8000-g07-fields",
        "cv8000",
        "integrity",
        "This folder is a Yokogawa CellVoyager measurement. The instrument could not image some fields "
        "(autofocus errors). How many fields of view were actually imaged in well G07?",
        lambda o, m: g.integer(next(len(w["images"]) for w in o["hcs"]["wells"] if w["well"] == "G07")),
        "oracle: /hcs/wells (G07) images (MeasurementData.mlf parsed with the Python standard library)",
        stage_as="sample_plate",
        answer_hint="the number of fields",
    ),
    spec(
        "ho-ana-hcs-cv8000-mito-mean",
        "cv8000",
        "analysis",
        "What is the mean intensity of well A01 in the Mitotracker deep red channel, pooled over all its "
        "fields (raw values as stored)?",
        fact("a01_mito_mean", lambda v: g.number(v, None, rel=0.005)),
        "facts-heldout: a01_mito_mean (tifffile on the plane files the .mlf lists)",
        stage_as="sample_plate",
        answer_hint=HINT_RAW,
    ),
    # ------------------------------------------------------------ newer analysis commands (added 2026-09-24)
    spec(
        "ho-ana-chrom-lcd-rid-tallest",
        "lcd",
        "analysis",
        "This HPLC run recorded a UV detector (two channels) and a refractive-index detector. In the "
        "refractive-index chromatogram, at what retention time (minutes) is the tallest peak?",
        fact("rid_tallest_rt", lambda v: g.number(v, "min", abs_=0.05)),
        "facts-heldout: rid_tallest_rt (the vendor's peak table in the depositor's LabSolutions export; "
        "the export's chromatogram points agree)",
        answer_hint="a number with its unit",
    ),
    spec(
        "ho-ana-chrom-lcd-uv-second-peak",
        "lcd",
        "analysis",
        "Integrate the peaks of the second UV channel of detector A. What is the retention time (minutes) of "
        "the second-largest peak by area?",
        fact("uv2_second_area_rt", lambda v: g.number(v, "min", abs_=0.05)),
        "facts-heldout: uv2_second_area_rt (the vendor's integration in the depositor's LabSolutions export; "
        "not given to the agent)",
        answer_hint="a number with its unit",
    ),
    spec(
        "ho-ms-mzmlgz-ms2-count",
        "mzmlgz",
        "counts",
        "How many MS/MS (MS2) spectra does this compressed mzML file contain?",
        lambda o, m: g.integer(sum(1 for s in o["spectra"]["scans"] if s["ms_level"] == 2)),
        "oracle: /spectra/scans with ms_level 2 (pyteomics)",
    ),
    spec(
        "ho-ana-ms-mzmlgz-tic",
        "mzmlgz",
        "analysis",
        "At what retention time (minutes) does the MS1 total ion chromatogram of this run reach its maximum?",
        fact("ms1_tic_apex_min", lambda v: g.number(v, "min", abs_=0.1)),
        "facts-heldout: ms1_tic_apex_min (pyteomics; the TIC cvParams agree)",
        answer_hint="a number with its unit",
    ),
    spec(
        "ho-ms-mzmlb-first-precursor",
        "mzmlb",
        "values",
        "What is the precursor m/z of the first spectrum in this mzMLb file?",
        fact("first_precursor_mz", lambda v: g.number(v, None, abs_=0.01)),
        "facts-heldout: first_precursor_mz (pyteomics.mzmlb)",
        answer_hint="the m/z",
    ),
    spec(
        "ho-ana-plate-gen5-bca-conc",
        "gen5bca",
        "analysis",
        "This is a BCA protein assay (standards STD1-STD5 at 0, 10, 25, 50 and 100 in duplicate, blank wells "
        "BLK). Using a linear standard curve on the blank-subtracted absorbance, what is the mean concentration "
        "of sample SPL3 (in the same units as the standards)?",
        fact("spl3_conc", lambda v: g.number(v, None, rel=0.01)),
        "facts-heldout: spl3_conc (the vendor's Conc results in the export; a NumPy linear refit agrees)",
        answer_hint="a number",
    ),
    spec(
        "ho-ana-nmr-bruker-fid-600",
        "fid600",
        "analysis",
        "This Bruker experiment holds only the raw 1H FID (no processed spectrum). Process it into a spectrum "
        "(Fourier transform, phase, ppm referencing). Between 6 and 10 ppm, at what shift is the tallest peak?",
        fact("tallest_ppm_6_10", lambda v: g.number(v, None, abs_=0.01)),
        "facts-heldout: tallest_ppm_6_10 (TopSpin's own pdata/1/1r of the same FID, read with nmrglue; NumPy agrees)",
        prepare={"remove": ["pdata"]},
        answer_hint="the chemical shift in ppm",
    ),
    spec(
        "ho-ana-ephys-abf-rheobase",
        "abfsteps",
        "analysis",
        "This is a current-clamp recording with a series of current steps. What is the rheobase: the smallest "
        "current step that evokes at least one action potential (in pA)?",
        fact("rheobase_pa", lambda v: g.number(v, "pA", abs_=1.0)),
        "facts-heldout: rheobase_pa (pyABF step levels and 0 mV crossings; the depositor's notes say 220 pA)",
        answer_hint="a number with its unit",
    ),
    spec(
        "ho-ana-ephys-abf-last-sweep-aps",
        "abfsteps",
        "analysis",
        "How many action potentials does the cell fire during the largest current step (the last sweep)?",
        fact("last_sweep_ap_count", g.integer),
        "facts-heldout: last_sweep_ap_count (pyABF + NumPy 0 mV crossings; neo AxonRawIO agrees)",
        answer_hint="the number of action potentials",
    ),
    spec(
        "ho-ana-mic-svs-jp2k-mean",
        "svsjp2k",
        "analysis",
        "What is the mean pixel value of the full-resolution image of this slide, over all three colour "
        "channels (0-255 scale)?",
        fact("level0_mean", lambda v: g.number(v, None, rel=0.005)),
        "facts-heldout: level0_mean (tifffile + imagecodecs; OpenSlide agrees)",
        answer_hint=HINT_RAW,
    ),
    spec(
        "ho-ana-mic-svs-jp2k-region",
        "svsjp2k",
        "analysis",
        "At full resolution, take the 512 x 512 pixel region whose top-left corner is at x = 2000, y = 2000 "
        "(pixels, counting from 0). What is the mean of its red channel (0-255 scale)?",
        fact("region_red_mean", lambda v: g.number(v, None, rel=0.005)),
        "facts-heldout: region_red_mean (tifffile + imagecodecs; OpenSlide read_region agrees)",
        answer_hint=HINT_RAW,
    ),
]


def load_oracle(corpus_id: str) -> dict:
    return g.share.oracle_json.load(ORACLE_DIR / f"{corpus_id}.json")  # <id>.json, or <id>.json.gz over 1 MiB


_PREVIEW_FACTS: dict | None = None  # --only: facts computed in this run, not read from OUT


def load_facts() -> dict:
    if _PREVIEW_FACTS is not None:
        return _PREVIEW_FACTS
    with OUT.open() as fh:
        return json.load(fh)


# ------------------------------------------------------------ bench instruments (2026-09-26)
SPECS += [
    spec(
        "ho-bench-akta-fractions",
        "unicornho",
        "counts",
        "How many fractions were collected during this ÄKTA purification run (not counting waste)?",
        lambda o, m: g.integer(sum(1 for e in o["events"]["Fraction"] if e[2].strip().lower() != "waste")),
        "oracle: /events/Fraction without Waste marks (UNICORN's event curve in the result export)",
    ),
    spec(
        "ho-bench-scn-width",
        "scnho",
        "dimensions",
        "How wide is this gel/blot image, in pixels?",
        lambda o, m: g.integer(o_img(o, "size_x")),
        "oracle: /images/0/size_x (Bio-Formats)",
    ),
    spec(
        "ho-bench-seahorse-measurements",
        "seahorseho",
        "counts",
        "How many measurement cycles did this Seahorse XF run have?",
        lambda o, m: g.integer(o["measurements"]),
        "oracle: /measurements (rate spans in the assay XML; "
        "seahorse_oracle.py checks them against the Measure commands)",
    ),
    spec(
        "ho-bench-jws-points",
        "jwsho",
        "counts",
        "How many data points does this FT-IR spectrum have?",
        lambda o, m: g.integer(int(o["export"]["header"]["NPOINTS"])),
        "oracle: /export/header/NPOINTS (JASCO Spectra Manager's text export of the same file)",
    ),
]


# ------------------------------------------------------------ held-out draw C (2026-09-26, evals/heldout_draw_c.py)
sys.modules.setdefault("heldout", sys.modules[__name__])  # heldout_draw_c's helpers use this module's readers
import heldout_draw_c  # noqa: E402

C.update(heldout_draw_c.C)
FACTS += heldout_draw_c.facts(Fact, sys.modules[__name__])
SPECS += heldout_draw_c.specs(spec, fact, g, sys.modules[__name__])

# ------------------------------------------------------------ held-out draw D (2026-10-06, evals/heldout_draw_d.py)
import heldout_draw_d  # noqa: E402

C.update(heldout_draw_d.C)
FACTS += heldout_draw_d.facts(Fact, sys.modules[__name__])
SPECS += heldout_draw_d.specs(spec, fact, g, sys.modules[__name__])


def build_question(manifest: dict, spec: g.Spec) -> dict:
    """generate.build_question, reading the held-out oracle and facts instead."""
    m = g.manifest_entry(manifest, spec.corpus_id)
    if m.get("tier") != SPLIT:
        raise SystemExit(f"{spec.qid}: {spec.corpus_id} is not a held-out file")
    kind = spec.source.split(":", 1)[0]
    if kind == "facts-heldout":
        entry = load_facts()[spec.corpus_id]
        values = {k: v["value"] for k, v in entry["facts"].items()}
        source = f"evals/facts/heldout.json [{spec.corpus_id}] ({entry['extractor']}; {entry['file']}) — {spec.source}"
    elif kind == "oracle":
        values = load_oracle(spec.corpus_id)
        source = f"corpus/oracle/heldout/{spec.corpus_id}.json ({values.get('reader', 'oracle')}) — {spec.source}"
    elif kind == "manifest":
        values = {}
        source = f"corpus/manifest.toml [{spec.corpus_id}] — {spec.source}"
    else:
        raise SystemExit(f"{spec.qid}: unknown source kind {kind!r}")
    answer = spec.answer(values, m)
    q: dict[str, Any] = {
        "id": spec.qid,
        "family": g.family_of(m["format"]),
        "format": m["format"],
        "category": spec.category,
        "file": g.file_ref(manifest, spec.corpus_id, spec.stage_as),
        "question": spec.question,
        "answer_hint": spec.answer_hint or g.hint_for(answer),
        "answer": answer,
        "source": source,
    }
    if spec.extra:
        q["extra_files"] = [g.file_ref(manifest, cid, sa) for cid, sa in spec.extra]
    q["split"] = SPLIT
    if m.get("exposed"):
        # developed on before the draw reserved the record (docs/benchmark/heldout.md): asked,
        # scored, but left out of the generalization numbers (stats.py)
        q["exposed"] = True
    return q


def build_all(manifest: dict) -> list[dict]:
    return [build_question(manifest, s) for s in SPECS]


def preview(only: str) -> int:
    """Compute the facts of the ids containing `only` and print the questions about them."""
    global FACTS, _PREVIEW_FACTS

    FACTS = [f for f in FACTS if only in f.corpus_id]
    _PREVIEW_FACTS = compute()
    manifest = g.load_manifest()
    for s in SPECS:
        if only in s.corpus_id:
            print(json.dumps(build_question(manifest, s), ensure_ascii=False))
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true", help="exit 1 if evals/facts/heldout.json is out of date")
    ap.add_argument(
        "--only",
        metavar="TEXT",
        help="compute only the facts of held-out ids containing TEXT and print them with the questions "
        "about those ids; writes nothing (for checking new facts and questions)",
    )
    args = ap.parse_args()
    if args.only:
        return preview(args.only)
    text = render(compute())
    if args.check:
        if not OUT.exists() or OUT.read_text(encoding="utf-8") != text:
            print("evals/facts/heldout.json is out of date; run evals/heldout.py", file=sys.stderr)
            return 1
        print("evals/facts/heldout.json is up to date")
        return 0
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(text, encoding="utf-8")
    print(f"wrote {OUT.relative_to(ROOT)}: {len(FACTS)} facts")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
