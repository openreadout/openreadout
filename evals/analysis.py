"""Analysis tier: questions whose answers need the data itself (pixels, sweeps, events, spectra,
chromatograms, processed NMR spectra, plate reads) plus a small computation.

"Which z-slice is brightest?", "how many events are GFP-positive?", "at what retention time does the
ion chromatogram of m/z 146.1176 peak?": a scientist's questions that no header field answers. The
answers are computed here with third-party readers run in the oracle venv, never with OpenReadout:
czifile, nd2, liffile, mrcfile, h5py, pyabf, neo, flowio, pyteomics (on the depositor's own mzML of a
Thermo run: the agent gets the `.raw`), scipy's netCDF reader, aston, nmrglue and allotropy (all
BSD/MIT/Apache), and rainbow-api (LGPL, run as a black box). Where a second
independent reader exists it is run too (pylibCZIrw and readlif as black boxes, fcsparser, neo's
AxonRawIO, a plain NumPy read of the MRC, NCS and TopSpin `1r` layouts from their public
descriptions, rainbow-api for Agilent MS data) and must agree, or the script stops.

The values go to `evals/facts/analysis.json` (committed), with the corpus file, reader and version,
the computation in words and the cross-check; `generate.py` reads them like the experiment facts, so
the questions regenerate without the corpus or the oracle libraries. The question specs live here
too (`ANALYSIS_SPECS`; this module imports only the standard library and `generate` at the top;
the readers are imported inside the functions that use them).

    oracle/.venv/bin/python evals/analysis.py          # recompute evals/facts/analysis.json
    oracle/.venv/bin/python evals/analysis.py --check  # exit 1 if the committed facts differ
"""

from __future__ import annotations

import argparse
import functools
import json
import math
import re
import sys
import tempfile
from collections.abc import Callable
from dataclasses import dataclass
from importlib.metadata import version
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
import assay_facts
import facts
import generate as g

ROOT = Path(__file__).resolve().parent.parent
OUT = Path(__file__).resolve().parent / "facts" / "analysis.json"


def path(corpus_id: str, role: str = "input") -> Path:
    p = facts.corpus_dir() / facts.manifest_file(corpus_id, role)
    if not p.exists():
        raise SystemExit(f"{corpus_id}: {p} is missing; fetch the corpus first (cargo xtask corpus fetch)")
    return p


def rd(*packages: str) -> str:
    return " + ".join(f"{p} {version(p)}" for p in packages)


def tidy(v: Any) -> Any:
    """JSON-stable values: 8 significant digits for floats, recursively."""
    if isinstance(v, bool | str) or v is None:
        return v
    if isinstance(v, int) or (hasattr(v, "dtype") and v.dtype.kind in "iu"):
        return int(v)
    if isinstance(v, float) or hasattr(v, "dtype"):
        return float(f"{float(v):.8g}")
    if isinstance(v, dict):
        return {k: tidy(x) for k, x in v.items()}
    if isinstance(v, list | tuple):
        return [tidy(x) for x in v]
    raise TypeError(f"cannot store {type(v)}")


def agree(a: Any, b: Any, rel: float = 1e-6) -> bool:
    if isinstance(a, dict):
        return isinstance(b, dict) and a.keys() == b.keys() and all(agree(a[k], b[k], rel) for k in a)
    if isinstance(a, float) or isinstance(b, float):
        return math.isclose(float(a), float(b), rel_tol=rel, abs_tol=1e-9)
    return a == b


# ---------------------------------------------------------------- microscopy


def czi_scene(p: Path, scene: int = 0):
    """(array, dims) of one scene, czifile (BSD-3)."""
    import czifile

    with czifile.CziFile(p) as c:
        s = c.scenes[scene]
        return s.asarray(), tuple(s.dims)


def czi_planes(p: Path, planes: list[dict], scene: int = 0):
    """Planes read with pylibCZIrw (LGPL, black box), stacked."""
    import numpy as np
    from pylibCZIrw import czi as pyczi

    with pyczi.open_czi(str(p)) as d:
        kw = {"scene": scene} if d.scenes_bounding_rectangle else {}  # files without scenes
        return np.stack([d.read(plane=pl, **kw)[..., 0] for pl in planes])


def czi_mip_mean(cid: str, channel: int):
    a, dims = czi_scene(path(cid))
    assert dims == ("C", "Z", "Y", "X"), dims
    return a[channel].max(axis=0).mean(dtype="float64"), rd("czifile")


def czi_mip_mean_cross(cid: str, channel: int):
    a, dims = czi_scene(path(cid))
    st = czi_planes(path(cid), [{"C": channel, "Z": z, "T": 0} for z in range(a.shape[dims.index("Z")])])
    return st.max(axis=0).mean(dtype="float64"), rd("pylibCZIrw") + " (black box)"


def czi_count_value(cid: str, scene: int, channel: int, value: int):
    a, dims = czi_scene(path(cid), scene)
    assert dims == ("C", "Y", "X"), dims
    return int((a[channel] == value).sum()), rd("czifile")


def czi_count_value_cross(cid: str, scene: int, channel: int, value: int):
    st = czi_planes(path(cid), [{"C": channel, "Z": 0, "T": 0}], scene)
    return int((st == value).sum()), rd("pylibCZIrw") + " (black box)"


def czi_means(cids: list[str]):
    out = {}
    for cid in cids:
        a, _ = czi_scene(path(cid))
        out[cid] = a.mean(dtype="float64")
    return out, rd("czifile")


def czi_means_cross(cids: list[str]):
    out = {}
    for cid in cids:
        a, dims = czi_scene(path(cid))
        sizes = dict(zip(dims, a.shape, strict=True))
        planes = [
            {"C": c, "Z": z, "T": t}
            for t in range(sizes.get("T", 1))
            for z in range(sizes.get("Z", 1))
            for c in range(sizes.get("C", 1))
        ]
        out[cid] = czi_planes(path(cid), planes).mean(dtype="float64")
    return out, rd("pylibCZIrw") + " (black box)"


def nd2_array(cid: str):
    import nd2

    with nd2.ND2File(path(cid)) as h:
        return h.asarray(), list(h.sizes)


def nd2_frame_means(cid: str, axis: str):
    """Mean of every frame along `axis` (the other non-YX axes must be absent)."""
    a, axes = nd2_array(cid)
    assert axes == [axis, "Y", "X"], axes
    return [float(x.mean(dtype="float64")) for x in a], rd("nd2")


def nd2_argmin_t(cid: str):
    means, reader = nd2_frame_means(cid, "T")
    return means.index(min(means)) + 1, reader


def nd2_argmax_position(cid: str, channel: int):
    a, axes = nd2_array(cid)
    assert axes == ["P", "C", "Y", "X"], axes
    means = a[:, channel].reshape(a.shape[0], -1).mean(axis=1, dtype="float64")
    return int(means.argmax()) + 1, rd("nd2")


def lif_image(p: Path, name: str | None = None, index: int = 0):
    import liffile

    with liffile.LifFile(p) as f:
        im = next(i for i in f.images if i.name == name) if name else f.images[index]
        return im.asarray(), tuple(im.dims), {k: [float(x) for x in v] for k, v in im.coords.items() if k == "λ"}


def lif_brightest_band_nm(cid: str):
    """Lower edge of the brightest band: the λ coordinate liffile reports (the band's start), checked
    against the file's LambdaEmission definition (begin + index × step, bandwidth)."""
    import liffile

    a, dims, coords = lif_image(path(cid))
    assert dims == ("λ", "Y", "X"), dims
    i = int(a.reshape(a.shape[0], -1).mean(axis=1, dtype="float64").argmax())
    with liffile.LifFile(path(cid)) as f:
        el = f.images[0].xml_element.find(".//LambdaEmission")
        begin, step = float(el.get("LambdaDetectionBegin")), float(el.get("LambdaDetectionStepSize"))
    nm = coords["λ"][i] * 1e9
    assert abs(nm - (begin + i * step)) < 1e-6, (nm, begin, step)
    return round(nm, 3), rd("liffile")


def lif_brightest_band_cross(cid: str):
    import numpy as np
    from readlif.reader import LifFile

    im = LifFile(str(path(cid))).get_image(0)
    n = im.dims_n[5]
    means = [np.asarray(im.get_plane(display_dims=(1, 2), c=0, requested_dims={5: i})).mean() for i in range(n)]
    i = int(np.argmax(means))
    return round(420.0 + 10.0 * i, 3), rd("readlif") + " (black box; λ from the dimension origin 420 nm, step 10 nm)"


def lif_series_means(cid: str, channel: int):
    import liffile

    out = {}
    with liffile.LifFile(path(cid)) as f:
        for im in f.images:
            if im.name.startswith("Preview"):
                continue
            a = im.asarray()
            assert tuple(im.dims) == ("C", "Y", "X"), im.dims
            out[im.name] = a[channel].mean(dtype="float64")
    return out, rd("liffile")


def lif_series_means_cross(cid: str, channel: int):
    import numpy as np
    from readlif.reader import LifFile

    out = {}
    for im in LifFile(str(path(cid))).get_iter_image():
        if not im.name.startswith("Preview"):
            out[im.name] = np.asarray(im.get_frame(z=0, t=0, c=channel)).mean(dtype="float64")
    return out, rd("readlif") + " (black box)"


# ---------------------------------------------------------------- electron microscopy


def mrc_data(cid: str):
    import mrcfile

    with mrcfile.open(path(cid), permissive=True) as m:
        return m.data.copy()


def mrc_numpy(cid: str):
    """The MRC2014 layout (public): 1024-byte header, NX NY NZ MODE at words 1-4, NSYMBT (extended
    header bytes) at word 24, data after the extended header, x fastest; little-endian here."""
    import numpy as np

    b = path(cid).read_bytes()
    nx, ny, nz, mode = np.frombuffer(b[:16], "<i4")
    nsymbt = int(np.frombuffer(b[92:96], "<i4")[0])
    dt = {0: "i1", 1: "<i2", 2: "<f4", 6: "<u2"}[int(mode)]
    n = int(nx) * int(ny) * int(nz)
    return np.frombuffer(b, dt, count=n, offset=1024 + nsymbt).reshape(int(nz), int(ny), int(nx))


def mrc_std(cid: str, reader: Callable = mrc_data, name: str = "mrcfile"):
    return reader(cid).std(dtype="float64"), rd(name) if name == "mrcfile" else name


def mrc_count_above(cid: str, threshold: float, reader: Callable = mrc_data, name: str = "mrcfile"):
    d = reader(cid).astype("float64")
    assert not ((d > threshold - 1e-4) & (d < threshold + 1e-4)).any(), "a voxel sits on the threshold"
    return int((d > threshold).sum()), rd(name) if name == "mrcfile" else name


def mrc_argmax_section_std(cid: str, reader: Callable = mrc_data, name: str = "mrcfile"):
    d = reader(cid)
    s = d.reshape(d.shape[0], -1).std(axis=1, dtype="float64")
    return int(s.argmax()) + 1, rd(name) if name == "mrcfile" else name


NUMPY_MRC = "numpy on the MRC2014 layout"


def emd_image_mean(cid: str):
    """The Velox EMD's one image (h5py): `Data/Image/<uuid>/Data`, (y, x, frame)."""
    import h5py

    with h5py.File(path(cid), "r") as f:
        (key,) = list(f["Data/Image"])
        d = f["Data/Image"][key]["Data"][()]
    assert d.shape[2] == 1, d.shape
    return d.mean(dtype="float64"), rd("h5py")


# ---------------------------------------------------------------- electrophysiology


def abf_sweeps(cid: str, channel: int):
    import pyabf

    a = pyabf.ABF(str(path(cid)))
    out = []
    for s in a.sweepList:
        a.setSweep(s, channel)
        out.append(a.sweepY.astype("float64").copy())
    return out, a


def abf_sweeps_neo(cid: str, channel: int):
    import numpy as np
    from neo.rawio import AxonRawIO

    r = AxonRawIO(filename=str(path(cid)))
    r.parse_header()
    out = []
    for s in range(r.segment_count(0)):
        raw = r.get_analogsignal_chunk(block_index=0, seg_index=s, stream_index=0, channel_indexes=[channel])
        y = r.rescale_signal_raw_to_float(raw, dtype="float64", stream_index=0, channel_indexes=[channel])
        out.append(np.asarray(y)[:, 0])
    return out, r


def abf_peak(cid: str, sweep: int, channel: int, neo: bool = False):
    sw, _ = (abf_sweeps_neo if neo else abf_sweeps)(cid, channel)
    return float(sw[sweep - 1].max()), rd("neo") + " AxonRawIO" if neo else rd("pyabf")


def abf_count_peaks_above(cid: str, channel: int, threshold: float, neo: bool = False):
    sw, _ = (abf_sweeps_neo if neo else abf_sweeps)(cid, channel)
    return sum(1 for y in sw if y.max() > threshold), rd("neo") + " AxonRawIO" if neo else rd("pyabf")


def abf_window_mean(cid: str, sweep: int, channel: int, seconds: float, neo: bool = False):
    sw, h = (abf_sweeps_neo if neo else abf_sweeps)(cid, channel)
    rate = float(h.header["signal_channels"]["sampling_rate"][channel]) if neo else float(h.sampleRate)
    n = round(seconds * rate)
    return float(sw[sweep - 1][:n].mean()), rd("neo") + " AxonRawIO" if neo else rd("pyabf")


def abf_argmax_sweep(cid: str, channel: int, neo: bool = False):
    sw, _ = (abf_sweeps_neo if neo else abf_sweeps)(cid, channel)
    peaks = [float(y.max()) for y in sw]
    return peaks.index(max(peaks)) + 1, rd("neo") + " AxonRawIO" if neo else rd("pyabf")


def ncs_std_neo(cid: str):
    """Every sample of every Neo segment × |gain| (µV); the standard deviation does not depend on the
    sign convention (-InputInverted)."""
    import shutil

    import numpy as np
    from neo.rawio import NeuralynxRawIO

    with tempfile.TemporaryDirectory() as d:
        shutil.copy(path(cid), d)
        r = NeuralynxRawIO(dirname=d)
        r.parse_header()
        gain = abs(float(r.header["signal_channels"]["gain"][0]))
        parts = []
        for s in range(r.segment_count(0)):
            n = r.get_signal_size(0, s, 0)
            parts.append(np.asarray(r.get_analogsignal_chunk(0, s, 0, n, 0), dtype="float64")[:, 0] * gain)
    assert r.header["signal_channels"]["units"][0] == "uV"
    return np.concatenate(parts).std(), rd("neo") + " NeuralynxRawIO"


def ncs_std_numpy(cid: str):
    """The NCS layout (Neuralynx's public file description): 16 KiB text header (`-ADBitVolts`), then
    records of u64 time stamp, u32 channel, u32 rate, u32 valid-sample count, 512 × i16."""
    import numpy as np

    b = path(cid).read_bytes()
    adbit = float(re.search(r"-ADBitVolts\s+(\S+)", b[:16384].decode("latin-1")).group(1))
    rec = np.frombuffer(
        b[16384:], np.dtype([("ts", "<u8"), ("ch", "<u4"), ("fs", "<u4"), ("nv", "<u4"), ("s", "<i2", (512,))])
    )
    v = np.concatenate([r["s"][: r["nv"]] for r in rec]).astype("float64") * adbit * 1e6
    return v.std(), NUMPY_NCS


NUMPY_NCS = "numpy on the NCS record layout"


# ---------------------------------------------------------------- flow cytometry


def fcs_events(cid: str, fcsparser: bool = False):
    """(event matrix, $PnN names, $PnS labels) as stored: uncompensated, unscaled."""
    import numpy as np

    if fcsparser:
        import fcsparser as fp

        api = fp.api
        if not getattr(api, "_analysis_shim", False):  # fcsparser 0.2.4 calls a NumPy 1 method

            class _Compat(np.ndarray):
                def newbyteorder(self, order="S"):
                    return self.view(self.dtype.newbyteorder(order))

            orig = api.fromfile
            api.fromfile = lambda *a, **k: orig(*a, **k).view(_Compat)
            api._analysis_shim = True
        meta, df = fp.parse(str(path(cid)), channel_naming="$PnN", dtype="float64", reformat_meta=False)
        par = int(meta["$PAR"])
        return df.to_numpy(), [meta[f"$P{i}N"] for i in range(1, par + 1)], None
    import flowio

    f = flowio.FlowData(str(path(cid)))
    t = f.text
    for i in range(1, f.channel_count + 1):
        assert t.get(f"p{i}e", "0,0").replace(".", "").replace("0", "").strip(",") == "", "log amplification"
    names = [t[f"p{i}n"] for i in range(1, f.channel_count + 1)]
    labels = [t.get(f"p{i}s", "") for i in range(1, f.channel_count + 1)]
    return np.asarray(f.as_array(preprocess=False), dtype="float64"), names, labels


def fcs_col(cid: str, param: str, fcsparser: bool = False, by_label: bool = False):
    a, names, _ = fcs_events(cid, fcsparser)
    if by_label:
        _, names2, labels2 = fcs_events(cid)
        param = names2[[s.strip() for s in labels2].index(param)]
    return a[:, names.index(param)]


def fcs_median(cid: str, param: str, fcsparser: bool = False, by_label: bool = False):
    import numpy as np

    return float(np.median(fcs_col(cid, param, fcsparser, by_label))), rd("fcsparser" if fcsparser else "flowio")


def fcs_count_above(cid: str, param: str, threshold: float, fcsparser: bool = False):
    col = fcs_col(cid, param, fcsparser)
    return int((col > threshold).sum()), rd("fcsparser" if fcsparser else "flowio")


def fcs_medians(cids: list[str], param: str, fcsparser: bool = False):
    import numpy as np

    return {c: float(np.median(fcs_col(c, param, fcsparser))) for c in cids}, rd("fcsparser" if fcsparser else "flowio")


# ---------------------------------------------------------------- mass spectrometry


@functools.cache
def mzml_spectra(cid: str) -> list[dict]:
    """Every spectrum of the depositor's mzML (pyteomics): level, polarity, time (min), arrays and
    the TIC / base-peak cvParams."""
    import numpy as np
    from pyteomics import mzml

    out = []
    with mzml.MzML(str(path(cid, "oracle-export"))) as r:
        for s in r:
            sc = s["scanList"]["scan"][0]
            out.append(
                {
                    "level": s["ms level"],
                    "polarity": "+" if "positive scan" in s else "-" if "negative scan" in s else None,
                    "rt_min": float(sc["scan start time"]),
                    "time_unit": getattr(sc["scan start time"], "unit_info", "minute"),
                    "mz": np.asarray(s["m/z array"], dtype="float64"),
                    "i": np.asarray(s["intensity array"], dtype="float64"),
                    "tic": float(s["total ion current"]),
                    "bp_mz": float(s["base peak m/z"]),
                }
            )
    assert {s["time_unit"] for s in out} == {"minute"}
    return out


def ms_tic_apex_rt(cid: str, from_arrays: bool = False):
    ms1 = [s for s in mzml_spectra(cid) if s["level"] == 1]
    best = max(ms1, key=lambda s: s["i"].sum() if from_arrays else s["tic"])
    how = "summed intensity arrays" if from_arrays else "`total ion current` cvParams"
    return best["rt_min"], f"{rd('pyteomics')} on the depositor's mzML ({how})"


def ms_xic_apex_rt(cid: str, mz: float, ppm: float, polarity: str):
    import numpy as np

    lo, hi = mz * (1 - ppm * 1e-6), mz * (1 + ppm * 1e-6)
    ms1 = [s for s in mzml_spectra(cid) if s["level"] == 1 and s["polarity"] == polarity]
    y = np.array([s["i"][(s["mz"] >= lo) & (s["mz"] <= hi)].sum() for s in ms1])
    return ms1[int(y.argmax())]["rt_min"], f"{rd('pyteomics')} on the depositor's mzML"


def ms_nth_ms2_base_peak(cid: str, n: int, from_cvparam: bool = False):
    s = [s for s in mzml_spectra(cid) if s["level"] == 2][n - 1]
    if from_cvparam:
        return s["bp_mz"], f"{rd('pyteomics')} on the depositor's mzML (`base peak m/z` cvParam)"
    return float(s["mz"][s["i"].argmax()]), f"{rd('pyteomics')} on the depositor's mzML (intensity array)"


def ms_srm_top_precursor(cid: str):
    """The SRM chromatogram with the highest point; its precursor isolation-window target m/z."""
    from pyteomics import mzml

    best = None
    with mzml.MzML(str(path(cid, "oracle-export"))) as r:
        for c in r.iterfind("chromatogram"):
            if "selected reaction monitoring chromatogram" not in c:
                continue
            top = float(c["intensity array"].max())
            if best is None or top > best[0]:
                target = c["precursor"][0]["isolationWindow"]["isolation window target m/z"]
                best = (top, float(target))
    return best[1], f"{rd('pyteomics')} on the depositor's mzML"


def ms2_precursors(cid: str, openms: bool = False) -> list[dict]:
    """MS/MS spectra of the depositor's mzML in file order: scan start time (min), the selected
    ion's m/z and charge state (None when not recorded). pyteomics, or pyOpenMS as the second
    reader (charge 0 = not recorded)."""
    p = str(path(cid, "oracle-export"))
    out = []
    if openms:
        import pyopenms as oms

        exp = oms.MSExperiment()
        oms.MzMLFile().load(p, exp)
        for s in exp:
            if s.getMSLevel() == 2:
                pr = s.getPrecursors()[0]
                out.append({"rt_min": s.getRT() / 60.0, "mz": pr.getMZ(), "charge": pr.getCharge() or None})
        return out
    from pyteomics import mzml

    with mzml.MzML(p, decode_binary=False) as r:
        for s in r:
            if s["ms level"] != 2:
                continue
            ion = s["precursorList"]["precursor"][0]["selectedIonList"]["selectedIon"][0]
            z = ion.get("charge state")
            out.append(
                {
                    "rt_min": float(s["scanList"]["scan"][0]["scan start time"]),
                    "mz": float(ion["selected ion m/z"]),
                    "charge": int(z) if z is not None else None,
                }
            )
    return out


def ms2_charge_count(cid: str, charge: int, openms: bool = False):
    n = sum(1 for s in ms2_precursors(cid, openms) if s["charge"] == charge)
    who = rd("pyopenms") if openms else rd("pyteomics")
    return n, f"{who} on the depositor's mzML (selected ion `charge state`)"


def ms2_near(cid: str, mz: float, tol: float, what: str, openms: bool = False):
    """MS/MS scans whose selected-ion m/z is within `tol` of `mz`: `count`, or the scan start time
    of the first (`first_rt`)."""
    hits = [s for s in ms2_precursors(cid, openms) if abs(s["mz"] - mz) <= tol]
    who = rd("pyopenms") if openms else rd("pyteomics")
    value = len(hits) if what == "count" else hits[0]["rt_min"]
    return value, f"{who} on the depositor's mzML (selected ion m/z)"


# ---------------------------------------------------------------- chromatography


def andi_apex_min(cid: str):
    """ANDI/AIA netCDF (scipy): time of the highest `ordinate_values` point, delay + index × interval."""
    from scipy.io import netcdf_file

    with netcdf_file(path(cid), "r", mmap=False) as f:
        v = f.variables
        y = v["ordinate_values"][:].astype("float64")
        dt = float(v["actual_sampling_interval"][()])
        delay = float(v["actual_delay_time"][()]) if "actual_delay_time" in v else 0.0
        unit = f.retention_unit.decode().lower()
    t = delay + int(y.argmax()) * dt
    return t / 60.0 if unit.startswith("sec") else t, f"scipy {version('scipy')} netcdf_file"


def rainbow_traces(cid: str) -> dict:
    """{file name: (times in min, values, signal description)} of a ChemStation `.D` (rainbow-api,
    LGPL, run as a black box)."""
    import numpy as np
    import rainbow as rb

    out = {}
    for f in rb.read(str(path(cid))).datafiles:
        y = np.asarray(f.data, dtype="float64")
        out[f.name] = (np.asarray(f.xlabels, dtype="float64"), y, (f.metadata or {}).get("signal", ""))
    return out


def chemstation_apex_min(cid: str, file_name: str):
    x, y, _ = rainbow_traces(cid)[file_name]
    y = y[: len(x), 0] if y.ndim == 2 else y[: len(x)]
    return float(x[int(y.argmax())]), rd("rainbow-api") + " (black box)"


def chemstation_tallest_wavelength(cid: str):
    best = None
    for _name, (x, y, signal) in sorted(rainbow_traces(cid).items()):
        m = re.search(r"Sig=(\d+(?:\.\d+)?)", signal)
        if not m or y.ndim != 2 or y.shape[1] != 1:
            continue
        top = float(y[: len(x), 0].max())
        if best is None or top > best[0]:
            best = (top, float(m.group(1)))
    return best[1], rd("rainbow-api") + " (black box)"


def gcms_tic_apex_min(cid: str, rainbow: bool = False):
    import numpy as np

    if rainbow:
        x, y, _ = rainbow_traces(cid)["DATA.MS"]
        return float(x[int(y.sum(axis=1).argmax())]), rd("rainbow-api") + " (black box)"
    import scipy.io
    import scipy.io.netcdf

    scipy.io.netcdf.NetCDFFile = scipy.io.netcdf_file  # Aston imports a name newer SciPy dropped
    from aston.tracefile import TraceFile

    tr = TraceFile(str(path(cid) / "DATA.MS")).total_trace()
    t = np.asarray(tr.index, dtype="float64")
    return float(t[int(np.asarray(tr.values, dtype="float64").ravel().argmax())]), rd("aston")


# ---------------------------------------------------------------- NMR


def pdata_ppm(cid: str, lo: float | None = None, hi: float | None = None, raw: bool = False):
    """Chemical shift of the highest point of the processed real spectrum `pdata/1/1r`, optionally
    within [lo, hi] ppm. ppm(i) = OFFSET − i · SW_p / SF / SI (TopSpin `procs`). `raw`: read `1r`
    with NumPy (i4, byte order BYTORDP, × 2^NC_proc) instead of nmrglue."""
    import nmrglue as ng
    import numpy as np

    pd = path(cid) / "pdata" / "1"
    dic, data = ng.bruker.read_pdata(str(pd), scale_data=True)
    p = dic["procs"]
    if raw:
        data = np.fromfile(pd / "1r", dtype=">i4" if p["BYTORDP"] else "<i4").astype("float64") * 2.0 ** p["NC_proc"]
    si = int(p["SI"])
    assert data.shape == (si,), data.shape
    ppm = p["OFFSET"] - np.arange(si) * p["SW_p"] / p["SF"] / si
    mask = np.ones(si, bool) if lo is None else (ppm >= lo) & (ppm <= hi)
    i = int(np.flatnonzero(mask)[data[mask].argmax()])
    return round(float(ppm[i]), 4), "numpy on pdata/1/1r and procs" if raw else rd("nmrglue")


# ---------------------------------------------------------------- signal analysis (nmr-peaks, ephys-features, spikes)


def signal_oracle(name: str) -> dict:
    """A ground-truth file `oracle/signal/ephys.py` wrote with eFEL (LGPL, black box), pyABF and
    SpikeInterface into corpus/oracle/ephys-analysis/."""
    return json.loads((ROOT / "corpus" / "oracle" / "ephys-analysis" / name).read_text())


def efel_sweep_count(cid: str, sweep: int):
    o = signal_oracle(cid + ".json")
    row = o["sweeps"][sweep - 1]
    return int(row["spike_count"][0]), o["tool"] + " (spike_count, voltage threshold -20 mV)"


def abf_ap_count_numpy(cid: str, sweep: int, threshold: float = 0.0):
    """Upward crossings of `threshold` mV in one sweep (pyABF samples, NumPy)."""
    import numpy as np

    sw, _ = abf_sweeps(cid, 0)
    y = sw[sweep - 1]
    return int(np.count_nonzero((y[:-1] < threshold) & (y[1:] >= threshold))), rd(
        "pyabf"
    ) + f" + numpy (crossings of {threshold} mV)"


def efel_cell(cid: str, key: str):
    o = signal_oracle(cid + ".json")
    return o["cell"][key], o["tool"] + f" (cell.{key} from eFEL per-sweep values)"


def abf_rheobase_numpy(cid: str, threshold: float = 0.0):
    """Smallest positive step (pyABF epoch levels) whose step window holds an upward crossing of
    `threshold` mV (pyABF samples, NumPy)."""
    import numpy as np
    import pyabf

    a = pyabf.ABF(str(path(cid)))
    levels = []
    for s in a.sweepList:
        a.setSweep(s)
        levels.append(list(a.sweepEpochs.levels))
    k = next(
        i for i, t in enumerate(a.sweepEpochs.types) if t == "Step" and len({round(lv[i], 9) for lv in levels}) > 1
    )
    best = None
    for s in a.sweepList:
        a.setSweep(s, 0)
        p1, p2 = a.sweepEpochs.p1s[k], a.sweepEpochs.p2s[k]
        y = a.sweepY[p1:p2]
        level = float(a.sweepEpochs.levels[k])
        if level > 0 and np.count_nonzero((y[:-1] < threshold) & (y[1:] >= threshold)) > 0:
            best = level if best is None else min(best, level)
    return best, rd("pyabf") + f" + numpy (step epoch levels; crossings of {threshold} mV)"


def spikeinterface_rate(cid: str, channel: int):
    o = signal_oracle(cid + ".spikes.json")
    rate = o["channels"][channel]["count"] / (o["samples"] / o["sample_rate_hz"])
    return rate, o["tool"] + " (bandpass_filter 300-6000 Hz order 5, detect_peaks by_channel, 5 x MAD, neg)"


def topspin_integral_ratio(cid: str, num: int, den: int):
    """Ratio of two integrals TopSpin stored in pdata/1/integrals.txt (1-based region numbers)."""
    rows = {}
    for line in (path(cid) / "pdata" / "1" / "integrals.txt").read_text(errors="replace").splitlines():
        m = re.match(r"\s*(\d+)\s+([-\d.]+)\s+([-\d.]+)\s+([-\d.eE+]+)\s*$", line)
        if m:
            rows[int(m.group(1))] = float(m.group(4))
    return round(rows[num] / rows[den], 5), "TopSpin pdata/1/integrals.txt (vendor-computed integrals)"


def jcamp_tallest_ppm(cid: str, use_jcamp: bool = False):
    """Chemical shift of the highest point of a MestReNova JCAMP-DX 1H spectrum (X in Hz / the
    observe frequency)."""
    import numpy as np

    p = path(cid)
    if use_jcamp:
        import jcamp

        d = jcamp.readfile(str(p))
        blk = d["children"][0] if "children" in d else d
        x = np.asarray(blk["x"], dtype="float64")
        y = np.asarray(blk["y"], dtype="float64")
        obs = float(blk[".observe frequency"])
        return round(float(x[int(y.argmax())] / obs), 3), rd("jcamp")
    import nmrglue as ng

    dic, data = ng.jcampdx.read(str(p))
    y = np.asarray(data, dtype="float64").ravel()
    first, last = float(dic["FIRSTX"][0]), float(dic["LASTX"][0])
    obs = float(dic[".OBSERVEFREQUENCY"][0])
    i = int(y.argmax())
    return round((first + (last - first) * i / (len(y) - 1)) / obs, 3), rd("nmrglue")


# ---------------------------------------------------------------- plates


def plate_values(cid: str) -> dict[str, float]:
    """{well: value} of a single-read plate export (allotropy ASM, MIT), measurements with an error
    document left out."""
    sys.path.insert(0, str(ROOT / "oracle"))
    import plate

    p = path(cid)
    asm = plate.allotropy_asm(p, plate.vendor_for(p.name))
    out: dict[str, float] = {}
    for doc in asm["plate reader aggregate document"]["plate reader document"]:
        for m in doc["measurement aggregate document"]["measurement document"]:
            if "error aggregate document" in m:
                continue
            for k, v in m.items():
                if plate.mode_of(k) and isinstance(v, dict) and "value" in v:
                    well = m["sample document"]["location identifier"]
                    assert well not in out, well
                    out[well] = float(v["value"])
    return out


def plate_top_well(cid: str):
    vals = plate_values(cid)
    return max(vals, key=vals.get), rd("allotropy")


def plate_column_mean(cid: str, column: int):
    vals = [v for w, v in plate_values(cid).items() if int(re.fullmatch(r"[A-Z]+0*(\d+)", w).group(1)) == column]
    return sum(vals) / len(vals), rd("allotropy")


# ---------------------------------------------------------------- regressions of fixed bugs
# Questions that caught OpenReadout bugs (trace start times, saturation at the recorded bit depth,
# spectral detection windows, calculated plate reads), each asked about a second file or a second way.


def chemstation_window_apex_min(cid: str, file_name: str, lo: float, hi: float):
    """Retention time (min) of the highest point of one signal within [lo, hi] min (rainbow-api)."""
    import numpy as np

    x, y, _ = rainbow_traces(cid)[file_name]
    y = y[: len(x), 0] if y.ndim == 2 else y[: len(x)]
    mask = (x >= lo) & (x <= hi)
    i = int(np.flatnonzero(mask)[y[mask].argmax()])
    return float(x[i]), rd("rainbow-api") + " (black box)"


def lif_count_at(cid: str, channel: int, value: int):
    """Samples of one channel (all time points) equal to `value`, liffile; the file's
    `ChannelDescription@Resolution` must make `value` the full-scale 2^bits − 1."""
    import liffile

    with liffile.LifFile(path(cid)) as f:
        im = f.images[0]
        a = im.asarray()
        assert tuple(im.dims) == ("T", "C", "Y", "X"), im.dims
        bits = {int(c.get("Resolution")) for c in im.xml_element.iter("ChannelDescription")}
    assert bits == {12} and value == 4095, bits
    return int((a[:, channel] == value).sum()), rd("liffile")


def lif_count_at_cross(cid: str, channel: int, value: int):
    import numpy as np
    from readlif.reader import LifFile

    im = LifFile(str(path(cid))).get_image(0)
    assert im.bit_depth[channel] == 12, im.bit_depth
    n = sum(int((np.asarray(im.get_frame(z=0, t=t, c=channel)) == value).sum()) for t in range(im.dims.t))
    return n, rd("readlif") + " (black box; bit depth 12)"


def lif_brightest_band_center_nm(cid: str):
    """Centre of the brightest λ band's detection window: begin + index × step + bandwidth / 2 from the
    file's LambdaEmission definition, the band picked by mean intensity (liffile)."""
    import liffile

    a, dims, coords = lif_image(path(cid))
    assert dims == ("λ", "Y", "X"), dims
    i = int(a.reshape(a.shape[0], -1).mean(axis=1, dtype="float64").argmax())
    with liffile.LifFile(path(cid)) as f:
        el = f.images[0].xml_element.find(".//LambdaEmission")
        begin, step = float(el.get("LambdaDetectionBegin")), float(el.get("LambdaDetectionStepSize"))
        width = float(el.get("LambdaDetectionBandWidth"))
    assert abs(coords["λ"][i] * 1e9 - (begin + i * step)) < 1e-6
    return round(begin + i * step + width / 2, 3), rd("liffile")


def lif_brightest_band_center_cross(cid: str):
    start, reader = lif_brightest_band_cross(cid)
    return round(start + 10.0, 3), reader + ", + half the 20 nm bandwidth"


def plate_calculated_values(cid: str, name: str) -> dict[str, float]:
    """{well: value} of one calculated read (allotropy calculated-data documents, mapped to wells
    through their data-source measurement identifiers)."""
    sys.path.insert(0, str(ROOT / "oracle"))
    import plate

    p = path(cid)
    agg = plate.allotropy_asm(p, plate.vendor_for(p.name))["plate reader aggregate document"]
    well_of: dict[str, str] = {}
    for doc in agg["plate reader document"]:
        for m in doc["measurement aggregate document"]["measurement document"]:
            well_of[m["measurement identifier"]] = m["sample document"]["location identifier"]
    out: dict[str, float] = {}
    for c in agg["calculated data aggregate document"]["calculated data document"]:
        if c["calculated data name"] != name:
            continue
        (src,) = c["data source aggregate document"]["data source document"]
        well = well_of[src["data source identifier"]]
        assert well not in out, well
        out[well] = float(c["calculated result"]["value"])
    return out


def plate_calculated_count_above(cid: str, name: str, threshold: float):
    vals = plate_calculated_values(cid, name)
    assert len(vals) == 96, len(vals)
    return sum(1 for v in vals.values() if v > threshold), rd("allotropy")


# ---------------------------------------------------------------- high-content screening plates
# The plate index (Harmony Index.idx.xml, ImageXpress HTD, CellVoyager .mlf/.mes) is read with the
# Python standard library to find the plane files; their pixels with tifffile (BSD-3).


def _local(tag: str) -> str:
    return tag.rsplit("}", 1)[-1].rsplit(":", 1)[-1]


def _tif_mean(files: list[Path]) -> float:
    """Mean over every pixel of the given single-plane TIFFs (pooled)."""
    import numpy as np
    import tifffile

    total, count = 0.0, 0
    for f in files:
        a = tifffile.imread(f).astype(np.float64)
        total += float(a.sum())
        count += a.size
    return total / count


def hcs_harmony_mean(cid: str, row: int, col: int, field: int, channel: str):
    """Mean of the plane of (well row/col 1-based, field, channel name) named by Index.idx.xml."""
    import xml.etree.ElementTree as ET

    folder = path(cid)
    index = folder / "Images" / "Index.idx.xml"
    url = None
    for _, el in ET.iterparse(index, events=("end",)):
        if _local(el.tag) == "Image" and any(_local(c.tag) == "URL" for c in el):
            d = {_local(c.tag): (c.text or "").strip() for c in el}
            if (d.get("Row"), d.get("Col"), d.get("FieldID"), d.get("ChannelName"), d.get("PlaneID")) == (
                str(row),
                str(col),
                str(field),
                channel,
                "1",
            ):
                url = d["URL"]
            el.clear()
    return _tif_mean([index.parent / url]), rd("tifffile")


def hcs_ix_brightest_well(cid: str, wave_name: str):
    """Well whose image of wavelength `wave_name` (HTD WaveName) has the highest mean, among the
    wells whose file of that wavelength is in the folder."""
    import re as _re

    folder = path(cid)
    htd = next(q for q in folder.iterdir() if q.suffix.lower() == ".htd")
    wave = None
    for line in htd.read_bytes().decode("latin-1").splitlines():
        m = _re.match(r'"WaveName(\d+)",\s*"(.*)"', line.strip())
        if m and m.group(2) == wave_name:
            wave = int(m.group(1))
    means = {}
    for q in folder.iterdir():
        m = _re.fullmatch(_re.escape(htd.stem) + r"_([A-P]\d{2})_w(\d+)\.tif", q.name, _re.I)
        if m and int(m.group(2)) == wave:
            means[m.group(1)] = _tif_mean([q])
    return max(means, key=means.get), rd("tifffile")


def hcs_cv_well_mean(cid: str, row: int, col: int, target: str):
    """Pooled mean of every plane file of well (row, col) and the channel whose .mes Target is
    `target`, over the fields present in the folder (MeasurementData.mlf records)."""
    import xml.etree.ElementTree as ET

    folder = path(cid)
    mrf = ET.parse(folder / "MeasurementDetail.mrf").getroot()
    mes = folder / {_local(k): v for k, v in mrf.attrib.items()}["MeasurementSettingFileName"]
    ch = next(
        {_local(k): v for k, v in c.attrib.items()}["Ch"]
        for c in ET.parse(mes).getroot().iter()
        if _local(c.tag) == "Channel" and {_local(k): v for k, v in c.attrib.items()}.get("Target") == target
    )
    files = []
    for _, el in ET.iterparse(folder / "MeasurementData.mlf", events=("end",)):
        if _local(el.tag) == "MeasurementRecord":
            a = {_local(k): v for k, v in el.attrib.items()}
            if (a.get("Row"), a.get("Column"), a.get("Ch")) == (str(row), str(col), ch):
                f = folder / (el.text or "").strip()
                if f.exists():
                    files.append(f)
            el.clear()
    return _tif_mean(sorted(files)), rd("tifffile")


def hcs_ix_folder_brightest_channel(cid: str, well: str, site: int):
    """Channel (MetaSeries illumination setting) with the highest mean in (well, site) of an
    ImageXpress folder without HTD."""
    import re as _re

    import tifffile

    folder = path(cid)
    means = {}
    for q in folder.iterdir():
        m = _re.match(rf".+_{well}_s{site}_w(\d+)(?!_thumb)[0-9A-F]{{8}}-", q.name)
        if m and "_thumb" not in q.name and q.suffix.lower() == ".tif":
            with tifffile.TiffFile(q) as tf:
                name = tf.metaseries_metadata["PlaneInfo"]["_IllumSetting_"]
            means[name] = _tif_mean([q])
    return max(means, key=means.get), rd("tifffile")


def czi_jxr_scene_channel_mean(cid: str, scene: int, rgb: int):
    """Mean of one colour channel (0 red, 1 green, 2 blue) of a Bgr24 scene stored as JPEG XR subblocks,
    czifile (BSD-3), which hands each subblock to imagecodecs' jpegxr_decode (jxrlib); that returns the
    samples as R, G, B (unlike czifile's uncompressed Bgr24 path, which keeps B, G, R)."""
    with __import__("czifile").CziFile(path(cid)) as c:
        s = c.scenes[scene]
        assert tuple(s.dims) == ("Y", "X", "S"), s.dims
        a = s.asarray()
    return a[..., rgb].mean(dtype="float64"), rd("czifile", "imagecodecs")


def czi_jxr_scene_channel_mean_cross(cid: str, scene: int, rgb: int):
    """The same with pylibCZIrw (LGPL, black box; libCZI's own JPEG XR decoder), which returns B, G, R."""
    from pylibCZIrw import czi as pyczi

    with pyczi.open_czi(str(path(cid))) as d:
        a = d.read(plane={"C": 0, "Z": 0, "T": 0}, scene=scene)
    return a[..., 2 - rgb].mean(dtype="float64"), rd("pylibCZIrw") + " (black box)"


def svs_level0_mean(cid: str):
    """Mean of every R, G, B sample of page 0 (full resolution), tifffile + imagecodecs (OpenJPEG decodes
    the JPEG 2000 tiles, including 33003's Y, Cb, Cr to R, G, B)."""
    import tifffile

    with tifffile.TiffFile(path(cid)) as tf:
        a = tf.pages[0].asarray()
    assert a.ndim == 3 and a.shape[2] == 3, a.shape
    return a.mean(dtype="float64"), rd("tifffile", "imagecodecs")


def svs_level0_mean_cross(cid: str):
    """The same from OpenSlide (LGPL, black box), read in 2048-pixel squares (RGBA; alpha ignored)."""
    import numpy as np
    import openslide

    s = openslide.OpenSlide(str(path(cid)))
    w, h = s.dimensions
    total, n, step = 0.0, 0, 2048
    for y in range(0, h, step):
        for x in range(0, w, step):
            size = (min(step, w - x), min(step, h - y))
            t = np.asarray(s.read_region((x, y), 0, size))[..., :3]
            total += t.sum(dtype="float64")
            n += t.size
    return total / n, rd("openslide-python", "openslide-bin") + " (black box)"


# ---------------------------------------------------------------- held-out finding regressions
# Files added for the 2026-09-24 held-out findings (Orbitrap Exploris spectra, Thermo flagged peaks,
# comma-delimited Tecan exports). The answers are read from the depositors' exports and the CSV text.


def mzxml_scan_peaks(cid: str, num: int, from_attribute: bool = False):
    """Number of peaks of scan `num` in the depositor's mzXML: the decoded array, or the
    `peaksCount` attribute."""
    from pyteomics import mzxml

    with mzxml.MzXML(str(path(cid, "oracle-export"))) as r:
        for sp in r:
            if int(sp["num"]) == num:
                if from_attribute:
                    return int(sp["peaksCount"]), f"{rd('pyteomics')} on the depositor's mzXML (`peaksCount`)"
                return len(sp["m/z array"]), f"{rd('pyteomics')} on the depositor's mzXML (decoded m/z array)"
    raise SystemExit(f"{cid}: no scan {num}")


def tecan_csv_rows(cid: str) -> list[list[str]]:
    """The rows of a comma-delimited Tecan export (UTF-8 with a BOM; Windows-1252 when not UTF-8)."""
    import csv

    raw = path(cid).read_bytes()
    try:
        text = raw.decode("utf-8-sig")
    except UnicodeDecodeError:
        text = raw.removeprefix(b"\xef\xbb\xbf").decode("cp1252")
    return list(csv.reader(text.splitlines()))


def _csv_frame(rows: list[list[str]], **kw):
    import csv
    import io

    import pandas as pd

    buf = io.StringIO()
    csv.writer(buf).writerows(rows)
    buf.seek(0)
    return pd.read_csv(buf, **kw)


def icontrol_csv_table(cid: str, title: str):
    """The i-control 1.11 table under the title line starting `title` (one line per cycle), read with
    pandas."""
    rows = tecan_csv_rows(cid)
    i = next(k for k, r in enumerate(rows) if r and r[0].startswith(title))
    j = i + 2
    while j < len(rows) and rows[j] and rows[j][0].strip():
        j += 1
    return _csv_frame(rows[i + 1 : j])


def icontrol_csv_last_top_well(cid: str, title: str):
    df = icontrol_csv_table(cid, title)
    wells = [c for c in df.columns if re.fullmatch(r"[A-P]\d{1,2}", c)]
    last = df.iloc[-1][wells].astype(float)
    return str(last.idxmax()), f"{rd('pandas')} on the CSV text (last line of the `{title}` table)"


def icontrol_csv_last_top_well_csv(cid: str, title: str):
    rows = tecan_csv_rows(cid)
    i = next(k for k, r in enumerate(rows) if r and r[0].startswith(title))
    head = rows[i + 1]
    j = i + 2
    while j + 1 < len(rows) and rows[j + 1] and rows[j + 1][0].strip():
        j += 1
    vals = {w: float(v) for w, v in zip(head, rows[j], strict=False) if re.fullmatch(r"[A-P]\d{1,2}", w)}
    return max(vals, key=vals.get), "Python csv module on the CSV text"


def icontrol_csv_first_cycle_above(cid: str, title: str, well: str, threshold: float):
    df = icontrol_csv_table(cid, title)
    hit = df[df[well].astype(float) > threshold]
    return int(hit.iloc[0]["Cycle Nr."]), f"{rd('pandas')} on the CSV text (`{title}` table, column {well})"


def spark_over_count(cid: str, read_name: str):
    """Wells whose value is `OVER` in the SparkControl endpoint read named `read_name`."""
    rows = tecan_csv_rows(cid)
    name = None
    for i, r in enumerate(rows):
        if r and r[0] == "Name" and len(r) > 1 and r[1]:
            name = r[1]
        if r and r[0] == "<>" and name == read_name:
            n = 0
            for rr in rows[i + 1 :]:
                if not rr or not re.fullmatch(r"[A-P]\d{1,2}", rr[0]):
                    break
                n += rr[1].strip() == "OVER"
            return n, "Python csv module on the CSV text"
    raise SystemExit(f"{cid}: no read {read_name}")


def spark_kinetic_max(cid: str, label: str, well: str):
    """Largest numeric value of `well` in the SparkControl kinetic table titled `label` (pandas)."""
    import pandas as pd

    rows = tecan_csv_rows(cid)
    i = next(k for k, r in enumerate(rows) if r and r[0] == label and rows[k + 1][0].startswith("Cycle"))
    j = i + 1
    while j < len(rows) and rows[j] and rows[j][0].strip():
        j += 1
    df = _csv_frame(rows[i + 1 : j], header=None, index_col=0, dtype=str)
    v = pd.to_numeric(df.loc[well], errors="coerce").dropna()
    return float(v.max()), f"{rd('pandas')} on the CSV text (`{label}` table, line {well})"


# ---------------------------------------------------------------- facts


@dataclass
class Fact:
    corpus_id: str
    name: str
    how: str
    compute: Callable[[], tuple[Any, str]]
    cross: Callable[[], tuple[Any, str]] | None = None
    role: str = "input"  # the manifest entry the value is read from (Thermo runs: the depositor's mzML)
    also: tuple[str, ...] = ()  # other corpus files read (two-file questions)
    # relative tolerance of the cross-check (lossy codecs: decoders reconstruct differently)
    cross_rel: float = 1e-6


CZI_A, CZI_B = "zenodo7015307-T-1-Z-5-CH-1", "zenodo7015307-T-2-CH-1"
FCS_A, FCS_B = "fcsparser-fortessa-a01", "fcsparser-hts-lsr-ii-d06"
MWD = "entab-chemstation-mwd-d"

FACTS: list[Fact] = [
    # microscopy
    Fact(
        "zenodo7015307-Z-5-CH-2",
        "mip_mean_c2",
        "second channel (C index 1), maximum over the 5 z-slices per pixel, mean of that 256×256 projection",
        lambda: czi_mip_mean("zenodo7015307-Z-5-CH-2", 1),
        lambda: czi_mip_mean_cross("zenodo7015307-Z-5-CH-2", 1),
    ),
    Fact(
        "zenodo7015307-S-3-CH-2",
        "scene2_c1_at_16383",
        "scene index 1 (the second of three), channel index 0: number of pixels equal to 16383",
        lambda: czi_count_value("zenodo7015307-S-3-CH-2", 1, 0, 16383),
        lambda: czi_count_value_cross("zenodo7015307-S-3-CH-2", 1, 0, 16383),
    ),
    Fact(
        CZI_A,
        "mean_by_file",
        "mean of every pixel of every plane (single-channel files), per file",
        lambda: czi_means([CZI_A, CZI_B]),
        lambda: czi_means_cross([CZI_A, CZI_B]),
        also=(CZI_B,),
    ),
    Fact(
        "ome-aryeh-MeOh-high-fluo-003",
        "dimmest_t",
        "mean of each of the 13 frames (T axis; one channel, one position), 1-based index of the lowest",
        lambda: nd2_argmin_t("ome-aryeh-MeOh-high-fluo-003"),
    ),
    Fact(
        "zenodo21162526-nested-loop",
        "brightest_position_egfp",
        "channel `eGFP` (C index 1 of DAPI, eGFP, mCherry, Cy5, TD): mean of each of the 24 positions, "
        "1-based index of the highest",
        lambda: nd2_argmax_position("zenodo21162526-nested-loop", 1),
    ),
    Fact(
        "zenodo14976703-Convalaria-LambdaScan",
        "brightest_band_start_nm",
        "mean of each of the 29 λ planes; the λ coordinate (band start; bands are 20 nm wide, 10 nm apart) "
        "of the highest",
        lambda: lif_brightest_band_nm("zenodo14976703-Convalaria-LambdaScan"),
        lambda: lif_brightest_band_cross("zenodo14976703-Convalaria-LambdaScan"),
    ),
    Fact(
        "bsst749-4i-bodipy-ctl",
        "series_mean_c1",
        "mean of channel index 0 of every image series except the `Preview*` image",
        lambda: lif_series_means("bsst749-4i-bodipy-ctl", 0),
        lambda: lif_series_means_cross("bsst749-4i-bodipy-ctl", 0),
    ),
    # electron microscopy
    Fact(
        "empiar10045-class2d-it025",
        "highest_std_class",
        "standard deviation of each 200×200 section, 1-based index of the highest",
        lambda: mrc_argmax_section_std("empiar10045-class2d-it025"),
        lambda: mrc_argmax_section_std("empiar10045-class2d-it025", mrc_numpy, NUMPY_MRC),
    ),
    Fact(
        "zenodo16462008-8RRH-molmap-30A",
        "voxels_above_0_08",
        "number of voxels with a value > 0.08 (none within 1e-4 of 0.08)",
        lambda: mrc_count_above("zenodo16462008-8RRH-molmap-30A", 0.08),
        lambda: mrc_count_above("zenodo16462008-8RRH-molmap-30A", 0.08, mrc_numpy, NUMPY_MRC),
    ),
    Fact(
        "mrcfile-fei-extended",
        "std",
        "population standard deviation of all 3838×3710 pixel values (the header's RMS field is -1)",
        lambda: mrc_std("mrcfile-fei-extended"),
        lambda: mrc_std("mrcfile-fei-extended", mrc_numpy, NUMPY_MRC),
    ),
    Fact(
        "zenodo20040988-0050-STEM-15.4nm",
        "mean",
        "mean of the 512×512 raw detector values of the one image",
        lambda: emd_image_mean("zenodo20040988-0050-STEM-15.4nm"),
    ),
    # electrophysiology
    Fact(
        "pyabf-14o16001-vc-pair-step",
        "sweep5_ch1_max",
        "maximum of sweep index 4 (the fifth), channel index 0 (IN 0, pA)",
        lambda: abf_peak("pyabf-14o16001-vc-pair-step", 5, 0),
        lambda: abf_peak("pyabf-14o16001-vc-pair-step", 5, 0, neo=True),
    ),
    Fact(
        "pyabf-2015-09-10-0001",
        "sweeps_peak_above_600pA",
        "number of the 13 sweeps whose maximum on channel index 0 (Ipatch, pA) exceeds 600",
        lambda: abf_count_peaks_above("pyabf-2015-09-10-0001", 0, 600.0),
        lambda: abf_count_peaks_above("pyabf-2015-09-10-0001", 0, 600.0, neo=True),
    ),
    Fact(
        "pyabf-171116sh-0011",
        "sweep3_first_100ms_mean",
        "mean of the first round(0.1 s × 20 kHz) = 2000 samples of sweep index 2 (the third), pA",
        lambda: abf_window_mean("pyabf-171116sh-0011", 3, 0, 0.1),
        lambda: abf_window_mean("pyabf-171116sh-0011", 3, 0, 0.1, neo=True),
    ),
    Fact(
        "pyabf-171117-hfmixfret",
        "yfp_brightest_sweep",
        "channel `YFP` (index 2 of Current, Voltage, YFP, CFP): maximum per sweep, 1-based index of the highest",
        lambda: abf_argmax_sweep("pyabf-171117-hfmixfret", 2),
        lambda: abf_argmax_sweep("pyabf-171117-hfmixfret", 2, neo=True),
    ),
    Fact(
        "nlx-cheetah-v5-7-4-csc1-ncs",
        "std_uv",
        "population standard deviation of every valid sample of every record, in µV (ADBitVolts × 1e6)",
        lambda: ncs_std_neo("nlx-cheetah-v5-7-4-csc1-ncs"),
        lambda: ncs_std_numpy("nlx-cheetah-v5-7-4-csc1-ncs"),
    ),
    # flow cytometry
    Fact(
        "flowio-g11",
        "median_gfp_a",
        "median of parameter BL1-A ($PnS GFP-A) over all 5,785 events, values as stored",
        lambda: fcs_median("flowio-g11", "BL1-A"),
        lambda: fcs_median("flowio-g11", "BL1-A", fcsparser=True),
    ),
    Fact(
        "flowio-g11",
        "gfp_a_above_10000",
        "number of events with BL1-A ($PnS GFP-A) > 10000, values as stored",
        lambda: fcs_count_above("flowio-g11", "BL1-A", 10000.0),
        lambda: fcs_count_above("flowio-g11", "BL1-A", 10000.0, fcsparser=True),
    ),
    Fact(
        "flowio-100715",
        "median_cd3",
        "median of the parameter whose $PnS is `CD3` (R780-A) over all events, values as stored",
        lambda: fcs_median("flowio-100715", "CD3", by_label=True),
        lambda: fcs_median("flowio-100715", "CD3", fcsparser=True, by_label=True),
    ),
    Fact(
        FCS_A,
        "median_fitc_a_by_file",
        "median of FITC-A ($PnG 1.0) over all events, values as stored, per file",
        lambda: fcs_medians([FCS_A, FCS_B], "FITC-A"),
        lambda: fcs_medians([FCS_A, FCS_B], "FITC-A", fcsparser=True),
        also=(FCS_B,),
    ),
    # mass spectrometry (the depositor's mzML stands in for the .raw the agent gets)
    Fact(
        "mtbls20-caffeine-pos",
        "tic_apex_rt_min",
        "MS1 spectrum with the highest total ion current: its scan start time (min)",
        lambda: ms_tic_apex_rt("mtbls20-caffeine-pos"),
        lambda: ms_tic_apex_rt("mtbls20-caffeine-pos", from_arrays=True),
        role="oracle-export",
    ),
    Fact(
        "mtbls755-hilic-dpoly",
        "xic_146_1176_apex_rt_min",
        "positive-polarity MS1 spectra: intensities summed over m/z 146.1176 ± 10 ppm, scan start time "
        "of the maximum (min)",
        lambda: ms_xic_apex_rt("mtbls755-hilic-dpoly", 146.1176, 10.0, "+"),
        role="oracle-export",
    ),
    Fact(
        "mtbls755-hilic-dpoly",
        "ms2_100_base_peak_mz",
        "the 100th MS2 spectrum in file order: m/z of its most intense peak",
        lambda: ms_nth_ms2_base_peak("mtbls755-hilic-dpoly", 100),
        lambda: ms_nth_ms2_base_peak("mtbls755-hilic-dpoly", 100, from_cvparam=True),
        role="oracle-export",
    ),
    Fact(
        "mtbls1822-tsq-74",
        "srm_top_precursor_mz",
        "SRM chromatogram with the highest intensity point: its precursor isolation-window target m/z",
        lambda: ms_srm_top_precursor("mtbls1822-tsq-74"),
        role="oracle-export",
    ),
    Fact(
        "mtbls755-hilic-dpoly",
        "ms2_charge2_count",
        "MS/MS spectra whose selected precursor ion has charge state 2",
        lambda: ms2_charge_count("mtbls755-hilic-dpoly", 2),
        lambda: ms2_charge_count("mtbls755-hilic-dpoly", 2, openms=True),
        role="oracle-export",
    ),
    Fact(
        "mtbls20-caffeine-pos",
        "ms2_first_rt_195_0877",
        "scan start time (min) of the first MS/MS spectrum whose selected precursor m/z is within 0.01 of "
        "195.0877 (caffeine [M+H]+)",
        lambda: ms2_near("mtbls20-caffeine-pos", 195.0877, 0.01, "first_rt"),
        lambda: ms2_near("mtbls20-caffeine-pos", 195.0877, 0.01, "first_rt", openms=True),
        role="oracle-export",
    ),
    Fact(
        "mtbls20-caffeine-pos",
        "ms2_count_195_0877",
        "MS/MS spectra whose selected precursor m/z is within 0.01 of 195.0877",
        lambda: ms2_near("mtbls20-caffeine-pos", 195.0877, 0.01, "count"),
        lambda: ms2_near("mtbls20-caffeine-pos", 195.0877, 0.01, "count", openms=True),
        role="oracle-export",
    ),
    # chromatography
    Fact(
        "cheminfo-agilent-hplc-cdf",
        "apex_rt_min",
        "time of the highest detector value (actual_delay_time + index × actual_sampling_interval, s → min)",
        lambda: andi_apex_min("cheminfo-agilent-hplc-cdf"),
    ),
    Fact(
        "chromhandler-001f0101-d",
        "fid_apex_rt_min",
        "FID1A.ch: time (min) of the highest value",
        lambda: chemstation_apex_min("chromhandler-001f0101-d", "FID1A.ch"),
    ),
    Fact(
        MWD,
        "mwd_230nm_apex_rt_min",
        "mwd1B.ch (signal `MWD B, Sig=230,5`): time (min) of the highest value",
        lambda: chemstation_apex_min(MWD, "mwd1B.ch"),
    ),
    Fact(
        "chromhandler-ca10-100um-d",
        "tallest_wavelength_nm",
        "the five DAD signals (Sig=254, 248, 210, 260, 280 nm): `Sig=` wavelength of the one with the highest value",
        lambda: chemstation_tallest_wavelength("chromhandler-ca10-100um-d"),
    ),
    Fact(
        "mtbls75-x-fsfa-hl-gc-o7c-1-d",
        "tic_apex_rt_min",
        "DATA.MS: total ion current per scan, time (min) of the highest",
        lambda: gcms_tic_apex_min("mtbls75-x-fsfa-hl-gc-o7c-1-d"),
        lambda: gcms_tic_apex_min("mtbls75-x-fsfa-hl-gc-o7c-1-d", rainbow=True),
    ),
    # NMR
    Fact(
        "nmrxiv-s846-50",
        "tallest_ppm",
        "pdata/1/1r: chemical shift of the highest point",
        lambda: pdata_ppm("nmrxiv-s846-50"),
        lambda: pdata_ppm("nmrxiv-s846-50", raw=True),
    ),
    Fact(
        "nmrxiv-s846-50",
        "tallest_ppm_7_5_to_8_5",
        "pdata/1/1r: chemical shift of the highest point between 7.5 and 8.5 ppm",
        lambda: pdata_ppm("nmrxiv-s846-50", 7.5, 8.5),
        lambda: pdata_ppm("nmrxiv-s846-50", 7.5, 8.5, raw=True),
    ),
    Fact(
        "nmrxiv-s596-1",
        "tallest_ppm",
        "pdata/1/1r (13C): chemical shift of the highest point",
        lambda: pdata_ppm("nmrxiv-s596-1"),
        lambda: pdata_ppm("nmrxiv-s596-1", raw=True),
    ),
    # signal analysis (nmr-peaks, ephys-features, spikes)
    Fact(
        "nmrxiv-s846-50",
        "integral_ratio_2_to_1",
        "TopSpin's stored integrals (pdata/1/integrals.txt): region 2 (7.901-7.814 ppm) / region 1 (8.239-8.178 ppm)",
        lambda: topspin_integral_ratio("nmrxiv-s846-50", 2, 1),
    ),
    Fact(
        "nmrxiv-s200-qhnmr-jdf",
        "mestrenova_tallest_ppm",
        "the MestReNova-processed spectrum of the same acquisition (JCAMP-DX export in the deposit): chemical shift "
        "of the highest point",
        lambda: jcamp_tallest_ppm("nmrxiv-s200-qhnmr"),
        lambda: jcamp_tallest_ppm("nmrxiv-s200-qhnmr", use_jcamp=True),
        also=("nmrxiv-s200-qhnmr",),
    ),
    Fact(
        "pyabf-171116sh-0018",
        "ap_count_sweep12",
        "sweep 12 (counting from 1), whole sweep: action potentials (eFEL spike_count)",
        lambda: efel_sweep_count("pyabf-171116sh-0018", 12),
        lambda: abf_ap_count_numpy("pyabf-171116sh-0018", 12),
    ),
    Fact(
        "pyabf-171116sh-0018",
        "rheobase_pa",
        "smallest positive current step with at least one action potential inside the step (eFEL spike_count_stimint "
        "per sweep)",
        lambda: efel_cell("pyabf-171116sh-0018", "rheobase_pa"),
        lambda: abf_rheobase_numpy("pyabf-171116sh-0018"),
    ),
    Fact(
        "pyabf-171116sh-0018",
        "input_resistance_mohm",
        "least-squares slope of (eFEL steady_state_voltage_stimend - voltage_base) against the step current over the "
        "spike-free negative steps",
        lambda: efel_cell("pyabf-171116sh-0018", "input_resistance_mohm"),
    ),
    Fact(
        "pyabf-2019-07-24-0055-fsi",
        "ap_count_sweep17",
        "sweep 17 (the last, counting from 1), whole sweep: action potentials (eFEL spike_count)",
        lambda: efel_sweep_count("pyabf-2019-07-24-0055-fsi", 17),
        lambda: abf_ap_count_numpy("pyabf-2019-07-24-0055-fsi", 17),
    ),
    Fact(
        "pyabf-190619b-0003",
        "rheobase_pa",
        "smallest positive current step with at least one action potential inside the step (eFEL spike_count_stimint "
        "per sweep)",
        lambda: efel_cell("pyabf-190619b-0003", "rheobase_pa"),
        lambda: abf_rheobase_numpy("pyabf-190619b-0003"),
    ),
    Fact(
        "brk-filespec2-3001-ns5",
        "spike_rate_ch1_hz",
        "first channel (chan1, index 0): negative peaks of the 300-6000 Hz band-passed signal beyond 5 x "
        "median(|x|)/0.6745 (SpikeInterface detect_peaks), divided by the recording duration",
        lambda: spikeinterface_rate("brk-filespec2-3001", 0),
    ),
    # plates
    Fact(
        "gen5-lum-endpoint",
        "top_well",
        "well with the highest luminescence value",
        lambda: plate_top_well("gen5-lum-endpoint"),
    ),
    Fact(
        "skanit-luciferase",
        "top_well",
        "well with the highest luminescence value",
        lambda: plate_top_well("skanit-luciferase"),
    ),
    Fact(
        "kaleido-abs-endpoint",
        "column1_mean",
        "mean absorbance of the 8 wells of column 1 (A1-H1)",
        lambda: plate_column_mean("kaleido-abs-endpoint", 1),
    ),
    # ------------------------------------------------ regressions of fixed bugs
    Fact(
        "chromhandler-ca10-100um-d",
        "dad_210nm_apex_rt_min",
        "dad1C.ch (signal `DAD C, Sig=210,4`, acquisition from -0.0417 min): time (min) of the highest value",
        lambda: chemstation_apex_min("chromhandler-ca10-100um-d", "dad1C.ch"),
    ),
    Fact(
        MWD,
        "mwd_210nm_apex_6_8_min",
        "mwd1A.ch (signal `MWD A, Sig=210,5`): time (min) of the highest value between 6 and 8 min "
        "(the whole run's maximum is at 11.9 min)",
        lambda: chemstation_window_apex_min(MWD, "mwd1A.ch", 6.0, 8.0),
    ),
    Fact(
        "aics-s-1-t-4-c-2-z-1",
        "c1_pixels_at_4095",
        "first channel, all 4 time points: samples equal to 4095, the 12-bit full scale "
        "(ChannelDescription Resolution 12)",
        lambda: lif_count_at("aics-s-1-t-4-c-2-z-1", 0, 4095),
        lambda: lif_count_at_cross("aics-s-1-t-4-c-2-z-1", 0, 4095),
    ),
    Fact(
        "zenodo14976703-Convalaria-LambdaScan",
        "brightest_band_center_nm",
        "mean of each of the 29 λ planes; the centre of the highest one's detection window "
        "(LambdaEmission: begin + index × step + bandwidth / 2)",
        lambda: lif_brightest_band_center_nm("zenodo14976703-Convalaria-LambdaScan"),
        lambda: lif_brightest_band_center_cross("zenodo14976703-Convalaria-LambdaScan"),
    ),
    Fact(
        "gen5-lum-endpoint",
        "normlum_wells_above_100",
        "wells whose NormLum value (calculated by Gen5 from the luminescence read) exceeds 100",
        lambda: plate_calculated_count_above("gen5-lum-endpoint", "NormLum", 100.0),
    ),
    Fact(
        "gen5-lum-endpoint",
        "column12_mean",
        "mean luminescence (RLU, the measured read) of the 8 wells of column 12 (A12-H12)",
        lambda: plate_column_mean("gen5-lum-endpoint", 12),
    ),
    # high-content screening plates (plate folders)
    Fact(
        "hcs-harmony-idr0034-folder",
        "dapi_mean_a01_f1",
        "Index.idx.xml (stdlib XML): the plane of well row 1 / column 1, field 1, ChannelName DAPI; "
        "mean of its 1360x1024 pixels (tifffile)",
        lambda: hcs_harmony_mean("hcs-harmony-idr0034-folder", 1, 1, 1, "DAPI"),
    ),
    Fact(
        "hcs-imagexpress-idr0081-folder",
        "brightest_dapi_well",
        "HTD (text): the wavelength named DAPI; among the wells whose file of that wavelength is in the "
        "folder, the one whose image has the highest mean (tifffile)",
        lambda: hcs_ix_brightest_well("hcs-imagexpress-idr0081-folder", "DAPI"),
    ),
    Fact(
        "hcs-cellvoyager-jump-1053601756-folder",
        "hoechst_mean_a01",
        "the .mes channel whose Target is Hoechst; every MeasurementData.mlf record of well row 1 / "
        "column 1 in that channel whose file is in the folder (fields 1 and 2); mean over all their pixels (tifffile)",
        lambda: hcs_cv_well_mean("hcs-cellvoyager-jump-1053601756-folder", 1, 1, "Hoechst"),
    ),
    Fact(
        "hcs-imagexpress-jump-a1170383-folder",
        "brightest_channel_a01_s1",
        "the five wavelength files of well A01 site 1; channel name = MetaSeries `_IllumSetting_`; the "
        "one with the highest mean (tifffile)",
        lambda: hcs_ix_folder_brightest_channel("hcs-imagexpress-jump-a1170383-folder", "A01", 1),
    ),
    # compressed whole-slide pixels (JPEG XR in CZI, JPEG 2000 in Aperio SVS)
    Fact(
        "openslide-zeiss-5-flat",
        "scene2_red_mean",
        "scene index 1 (the second of two), 11312x11337 Bgr24 pixels in JPEG XR subblocks: mean of the red "
        "samples (czifile/imagecodecs sample 0 of R, G, B; pylibCZIrw sample 2 of B, G, R)",
        lambda: czi_jxr_scene_channel_mean("openslide-zeiss-5-flat", 1, 0),
        lambda: czi_jxr_scene_channel_mean_cross("openslide-zeiss-5-flat", 1, 0),
    ),
    Fact(
        "openslide-aperio-jp2k-33003-1",
        "level0_mean",
        "page 0 (full resolution) of the Aperio SVS, JPEG 2000 tiles with Y, Cb, Cr components (compression "
        "33003) converted to R, G, B: mean over every R, G and B sample",
        lambda: svs_level0_mean("openslide-aperio-jp2k-33003-1"),
        lambda: svs_level0_mean_cross("openslide-aperio-jp2k-33003-1"),
        # OpenSlide converts 33003's sub-sampled YCbCr itself; its level-0 samples differ from OpenJPEG's
        # by at most 1 in 42 % of samples
        cross_rel=5e-4,
    ),
]
# qPCR (RDML, .eds, .rex): evals/qpcr_facts.py
FACTS += __import__("qpcr_facts").facts(Fact, path)
# fixes of the second held-out generalization report: evals/heldout_report_fixes.py
FACTS += __import__("heldout_report_fixes").facts(Fact, path)

# ------------------------------------------------ held-out finding regressions (2026-09-24)
FACTS += [
    Fact(
        "mtbls14508-mh-b4-c18-pos-dda-r01",
        "ms2_500_base_peak_mz",
        "the 500th MS2 spectrum in file order: m/z of its most intense peak",
        lambda: ms_nth_ms2_base_peak("mtbls14508-mh-b4-c18-pos-dda-r01", 500),
        lambda: ms_nth_ms2_base_peak("mtbls14508-mh-b4-c18-pos-dda-r01", 500, from_cvparam=True),
        role="oracle-export",
    ),
    Fact(
        "mtbls13930-neg-sqc-5",
        "scan_21_peak_count",
        "scan number 21 (an MS2 scan): number of centroid peaks in the depositor's export",
        lambda: mzxml_scan_peaks("mtbls13930-neg-sqc-5", 21),
        lambda: mzxml_scan_peaks("mtbls13930-neg-sqc-5", 21, from_attribute=True),
        role="oracle-export",
    ),
    Fact(
        "tecan-icontrol-csv-kinetic-wellr",
        "od_last_cycle_top_well",
        "absorbance table (`OD600:600`): well with the highest value on the last cycle line",
        lambda: icontrol_csv_last_top_well("tecan-icontrol-csv-kinetic-wellr", "OD600:"),
        lambda: icontrol_csv_last_top_well_csv("tecan-icontrol-csv-kinetic-wellr", "OD600:"),
    ),
    Fact(
        "tecan-icontrol-csv-kinetic-wellr",
        "a7_lum_first_cycle_above_100000",
        "luminescence table (`LUMI:Lum`): first cycle number whose A7 value exceeds 100,000",
        lambda: icontrol_csv_first_cycle_above("tecan-icontrol-csv-kinetic-wellr", "LUMI:", "A7", 100000),
    ),
    Fact(
        "tecan-sparkcontrol-csv-endpoint-flopr",
        "gain_100_over_wells",
        "the read named `Gain 100`: wells whose value is `OVER`",
        lambda: spark_over_count("tecan-sparkcontrol-csv-endpoint-flopr", "Gain 100"),
    ),
    Fact(
        "tecan-sparkcontrol-csv-kinetic-flopr",
        "od600_b1_max",
        "kinetic table `OD600`: largest value of well B1 over all cycles",
        lambda: spark_kinetic_max("tecan-sparkcontrol-csv-kinetic-flopr", "OD600", "B1"),
    ),
]

# plate analysis (`openreadout analyze assay`): evals/assay_facts.py
FACTS += assay_facts.facts(Fact)


def compute() -> dict:
    out: dict = {}
    for f in FACTS:
        value, reader = f.compute()
        value = tidy(value)
        rec: dict[str, Any] = {"value": value, "reader": reader, "how": f.how}
        if f.also:
            rec["files"] = [facts.manifest_file(c) for c in (f.corpus_id, *f.also)]
        if f.cross:
            other, other_reader = f.cross()
            if not agree(value, tidy(other), f.cross_rel):
                raise SystemExit(f"{f.corpus_id} {f.name}: {reader} says {value!r}, {other_reader} says {other!r}")
            rec["cross_check"] = f"{other_reader}: agrees"
            if f.cross_rel != 1e-6:
                rec["cross_check"] += f" within {f.cross_rel:.2%} ({tidy(other)!r})"
        entry = out.setdefault(
            f.corpus_id,
            {"file": facts.manifest_file(f.corpus_id, f.role), "extractor": "evals/analysis.py", "facts": {}},
        )
        entry["facts"][f.name] = rec
        print(f"{f.corpus_id} {f.name} = {value!r}", file=sys.stderr)
    return out


def render(data: dict) -> str:
    return json.dumps(data, indent=1, ensure_ascii=False, sort_keys=True) + "\n"


# ---------------------------------------------------------------- questions

HINT_RAW = "a number (raw values as stored in the file)"


def larger(values: dict[str, float], names: dict[str, str]) -> dict:
    """The staged name of the file with the larger value; naming the other file fails the answer."""
    win = max(values, key=values.get)
    lose = [names[c] for c in values if c != win]
    return g.string(names[win], [names[win].rsplit(".", 1)[0]], reject=[n.rsplit(".", 1)[0] for n in lose])


def only(value: str, others: list[str]) -> dict:
    return g.string(value, reject=[o for o in others if o != value])


def top_by(values: dict[str, float]) -> str:
    return max(values, key=values.get)


ANALYSIS_SPECS: list[g.Spec] = [
    # ------------------------------------------------------------ microscopy
    g.Spec(
        "ana-mic-czi-mip-mean",
        "zenodo7015307-Z-5-CH-2",
        "analysis",
        "Make a maximum-intensity projection of the second channel along z (for every x/y position, the "
        "brightest value over all z-slices). What is the mean raw pixel value of that projection?",
        lambda f, m: g.number(f["mip_mean_c2"], None, rel=0.005),
        "facts-analysis: mip_mean_c2 (czifile; pylibCZIrw agrees)",
        answer_hint=HINT_RAW,
    ),
    g.Spec(
        "ana-mic-czi-scene-saturated",
        "zenodo7015307-S-3-CH-2",
        "analysis",
        "This file holds several scenes. In the second scene (counting from 1), how many pixels of the first "
        "channel are saturated at the detector's 14-bit ceiling, i.e. have the raw value 16383?",
        lambda f, m: g.integer(f["scene2_c1_at_16383"]),
        "facts-analysis: scene2_c1_at_16383 (czifile; pylibCZIrw agrees)",
        answer_hint="the number of pixels",
    ),
    g.Spec(
        "ana-mic-czi-compare-mean",
        CZI_A,
        "analysis",
        "Which of the two images, sample_a.czi or sample_b.czi, has the higher mean raw intensity (averaged "
        "over every pixel of every plane in the file)?",
        lambda f, m: larger(f["mean_by_file"], {CZI_A: "sample_a.czi", CZI_B: "sample_b.czi"}),
        "facts-analysis: mean_by_file (czifile; pylibCZIrw agrees)",
        stage_as="sample_a.czi",
        extra=[(CZI_B, "sample_b.czi")],
        answer_hint="the file name only",
    ),
    g.Spec(
        "ana-mic-nd2-dimmest-timepoint",
        "ome-aryeh-MeOh-high-fluo-003",
        "analysis",
        "This is a time-lapse. At which time point (counting from 1) is the mean raw intensity of the frame lowest?",
        lambda f, m: g.integer(f["dimmest_t"]),
        "facts-analysis: dimmest_t (nd2)",
        answer_hint="the time point number, counting from 1",
    ),
    g.Spec(
        "ana-mic-nd2-brightest-position",
        "zenodo21162526-nested-loop",
        "analysis",
        "This experiment imaged many stage (XY) positions in several channels. At which position (counting "
        "from 1, in acquisition order) is the mean raw intensity of the eGFP channel highest?",
        lambda f, m: g.integer(f["brightest_position_egfp"]),
        "facts-analysis: brightest_position_egfp (nd2)",
        answer_hint="the position number, counting from 1",
    ),
    g.Spec(
        "ana-mic-lif-brightest-band",
        "zenodo14976703-Convalaria-LambdaScan",
        "analysis",
        "This is a spectral (lambda) scan: one image per emission band. Which band is brightest, i.e. has the "
        "highest mean raw intensity? Give the lower edge (start) of that band's detection window in nm.",
        lambda f, m: g.number(f["brightest_band_start_nm"], "nm", abs_=1),
        "facts-analysis: brightest_band_start_nm (liffile; readlif agrees)",
        answer_hint="a wavelength in nm",
    ),
    g.Spec(
        "ana-mic-lif-brightest-series",
        "bsst749-4i-bodipy-ctl",
        "analysis",
        "This Leica project holds several image series (ignore the preview image). Which series has the "
        "highest mean raw intensity in its first channel? Give the series name.",
        lambda f, m: only(top_by(f["series_mean_c1"]), list(f["series_mean_c1"])),
        "facts-analysis: series_mean_c1 (liffile; readlif agrees)",
        answer_hint="the series name only",
    ),
    # ------------------------------------------------------------ electron microscopy
    g.Spec(
        "ana-em-mrcs-contrast-class",
        "empiar10045-class2d-it025",
        "analysis",
        "Which 2D class average (counting from 1, in stack order) has the highest standard deviation of its "
        "pixel values, i.e. the most contrast?",
        lambda f, m: g.integer(f["highest_std_class"]),
        "facts-analysis: highest_std_class (mrcfile; NumPy on the MRC2014 layout agrees)",
        answer_hint="the class number, counting from 1",
    ),
    g.Spec(
        "ana-em-mrc-voxels-above",
        "zenodo16462008-8RRH-molmap-30A",
        "analysis",
        "How many voxels of this density map have a value greater than 0.08?",
        lambda f, m: g.integer(f["voxels_above_0_08"]),
        "facts-analysis: voxels_above_0_08 (mrcfile; NumPy on the MRC2014 layout agrees)",
        answer_hint="the number of voxels",
    ),
    g.Spec(
        "ana-em-mrc-std",
        "mrcfile-fei-extended",
        "analysis",
        "What is the standard deviation of the raw pixel values of this micrograph?",
        lambda f, m: g.number(f["std"], None, rel=0.005),
        "facts-analysis: std (mrcfile; NumPy on the MRC2014 layout agrees)",
        answer_hint=HINT_RAW,
    ),
    g.Spec(
        "ana-em-emd-mean",
        "zenodo20040988-0050-STEM-15.4nm",
        "analysis",
        "What is the mean raw detector value (counts, as stored) over the whole STEM image?",
        lambda f, m: g.number(f["mean"], None, rel=0.005),
        "facts-analysis: mean (h5py)",
        answer_hint=HINT_RAW,
    ),
    # ------------------------------------------------------------ electrophysiology
    g.Spec(
        "ana-ephys-abf-sweep-peak",
        "pyabf-14o16001-vc-pair-step",
        "analysis",
        "What is the peak (maximum) value of sweep 5 (counting from 1) on the first recorded channel?",
        lambda f, m: g.number(f["sweep5_ch1_max"], "pA", rel=0.005),
        "facts-analysis: sweep5_ch1_max (pyabf; neo AxonRawIO agrees)",
        answer_hint="a number with its unit",
    ),
    g.Spec(
        "ana-ephys-abf-sweeps-above",
        "pyabf-2015-09-10-0001",
        "analysis",
        "How many sweeps reach a peak current above 600 pA on the patch-current channel (the first channel)?",
        lambda f, m: g.integer(f["sweeps_peak_above_600pA"]),
        "facts-analysis: sweeps_peak_above_600pA (pyabf; neo AxonRawIO agrees)",
        answer_hint="the number of sweeps",
    ),
    g.Spec(
        "ana-ephys-abf-baseline",
        "pyabf-171116sh-0011",
        "analysis",
        "What is the mean current during the first 100 ms of sweep 3 (counting from 1)?",
        lambda f, m: g.number(f["sweep3_first_100ms_mean"], "pA", abs_=0.5),
        "facts-analysis: sweep3_first_100ms_mean (pyabf; neo AxonRawIO agrees)",
        answer_hint="a number with its unit",
    ),
    g.Spec(
        "ana-ephys-abf-yfp-sweep",
        "pyabf-171117-hfmixfret",
        "analysis",
        "This recording has several input channels, one of them the YFP fluorescence. In which sweep "
        "(counting from 1) does the YFP channel reach its highest value?",
        lambda f, m: g.integer(f["yfp_brightest_sweep"]),
        "facts-analysis: yfp_brightest_sweep (pyabf; neo AxonRawIO agrees)",
        answer_hint="the sweep number, counting from 1",
    ),
    g.Spec(
        "ana-ephys-ncs-std",
        "nlx-cheetah-v5-7-4-csc1-ncs",
        "analysis",
        "What is the standard deviation of this continuously sampled channel over the whole recording, in µV?",
        lambda f, m: g.number(f["std_uv"], "µV", rel=0.01),
        "facts-analysis: std_uv (neo NeuralynxRawIO; NumPy on the NCS record layout agrees)",
        answer_hint="a number with its unit",
    ),
    # ------------------------------------------------------------ flow cytometry
    g.Spec(
        "ana-flow-fcs-median-gfp",
        "flowio-g11",
        "analysis",
        "What is the median GFP-A value over all events? Use the values as stored (uncompensated, untransformed).",
        lambda f, m: g.number(f["median_gfp_a"], None, rel=0.005),
        "facts-analysis: median_gfp_a (flowio; fcsparser agrees)",
        answer_hint=HINT_RAW,
    ),
    g.Spec(
        "ana-flow-fcs-gfp-positive",
        "flowio-g11",
        "analysis",
        "How many events have a GFP-A value above 10,000? Use the values as stored (uncompensated, untransformed).",
        lambda f, m: g.integer(f["gfp_a_above_10000"]),
        "facts-analysis: gfp_a_above_10000 (flowio; fcsparser agrees)",
        answer_hint="the number of events",
    ),
    g.Spec(
        "ana-flow-fcs-median-cd3",
        "flowio-100715",
        "analysis",
        "What is the median CD3 signal over all events? Use the values as stored (uncompensated, untransformed).",
        lambda f, m: g.number(f["median_cd3"], None, rel=0.005),
        "facts-analysis: median_cd3 (flowio; fcsparser agrees)",
        answer_hint=HINT_RAW,
    ),
    g.Spec(
        "ana-flow-fcs-compare-fitc",
        FCS_A,
        "analysis",
        "Which of the two samples, sample_a.fcs or sample_b.fcs, has the higher median FITC-A signal (values as "
        "stored, uncompensated)?",
        lambda f, m: larger(f["median_fitc_a_by_file"], {FCS_A: "sample_a.fcs", FCS_B: "sample_b.fcs"}),
        "facts-analysis: median_fitc_a_by_file (flowio; fcsparser agrees)",
        stage_as="sample_a.fcs",
        extra=[(FCS_B, "sample_b.fcs")],
        answer_hint="the file name only",
    ),
    # ------------------------------------------------------------ mass spectrometry
    g.Spec(
        "ana-ms-raw-tic-apex",
        "mtbls20-caffeine-pos",
        "analysis",
        "At what retention time was the full scan (MS1) with the highest total ion current acquired?",
        lambda f, m: g.number(f["tic_apex_rt_min"], "min", abs_=0.01),
        "facts-analysis: tic_apex_rt_min (pyteomics on the depositor's mzML; TIC cvParams and summed arrays agree)",
        answer_hint="a retention time with its unit",
    ),
    g.Spec(
        "ana-ms-raw-xic-apex",
        "mtbls755-hilic-dpoly",
        "analysis",
        "Extract the ion chromatogram of m/z 146.1176 (± 10 ppm) from the positive-mode full scans (MS1) of "
        "this run. At what retention time is its apex?",
        lambda f, m: g.number(f["xic_146_1176_apex_rt_min"], "min", abs_=0.1),
        "facts-analysis: xic_146_1176_apex_rt_min (pyteomics on the depositor's mzML)",
        answer_hint="a retention time with its unit",
    ),
    g.Spec(
        "ana-ms-raw-ms2-base-peak",
        "mtbls755-hilic-dpoly",
        "analysis",
        "What is the m/z of the most intense peak in the 100th MS/MS spectrum of this run (counting MS/MS "
        "scans only, from the start)?",
        lambda f, m: g.number(round(f["ms2_100_base_peak_mz"], 4), None, abs_=0.005),
        "facts-analysis: ms2_100_base_peak_mz (pyteomics on the depositor's mzML; base-peak cvParam agrees)",
        answer_hint="the m/z value",
    ),
    g.Spec(
        "ana-ms-raw-ms2-charge2-count",
        "mtbls755-hilic-dpoly",
        "analysis",
        "How many of the MS/MS scans in this run have a precursor ion assigned charge state 2?",
        lambda f, m: g.integer(f["ms2_charge2_count"]),
        "facts-analysis: ms2_charge2_count (pyteomics on the depositor's mzML; pyOpenMS agrees)",
        answer_hint="the number of scans",
    ),
    g.Spec(
        "ana-ms-raw-first-ms2-caffeine-rt",
        "mtbls20-caffeine-pos",
        "analysis",
        "At what retention time was the first MS/MS scan of caffeine acquired, i.e. the first MS/MS scan "
        "whose precursor m/z is within 0.01 of 195.0877?",
        lambda f, m: g.number(f["ms2_first_rt_195_0877"], "min", abs_=0.01),
        "facts-analysis: ms2_first_rt_195_0877 (pyteomics on the depositor's mzML; pyOpenMS agrees)",
        answer_hint="a retention time with its unit",
    ),
    g.Spec(
        "ana-ms-raw-caffeine-ms2-count",
        "mtbls20-caffeine-pos",
        "analysis",
        "How many MS/MS scans in this run fragmented a precursor within 0.01 m/z of 195.0877 (caffeine [M+H]+)?",
        lambda f, m: g.integer(f["ms2_count_195_0877"]),
        "facts-analysis: ms2_count_195_0877 (pyteomics on the depositor's mzML; pyOpenMS agrees)",
        answer_hint="the number of scans",
    ),
    g.Spec(
        "ana-ms-raw-srm-top",
        "mtbls1822-tsq-74",
        "analysis",
        "This triple-quadrupole run monitored many SRM transitions. Which transition reached the highest "
        "intensity? Give its precursor m/z.",
        lambda f, m: g.number(round(f["srm_top_precursor_mz"], 3), None, abs_=0.01),
        "facts-analysis: srm_top_precursor_mz (pyteomics on the depositor's mzML)",
        answer_hint="the precursor m/z",
    ),
    # ------------------------------------------------------------ chromatography
    g.Spec(
        "ana-chrom-andi-apex",
        "cheminfo-agilent-hplc-cdf",
        "analysis",
        "At what retention time does the detector signal of this chromatogram reach its highest point?",
        lambda f, m: g.number(round(f["apex_rt_min"], 4), "min", abs_=0.05),
        "facts-analysis: apex_rt_min (scipy netcdf_file)",
        answer_hint="a retention time with its unit",
    ),
    g.Spec(
        "ana-chrom-fid-apex",
        "chromhandler-001f0101-d",
        "analysis",
        "At what retention time is the tallest peak of the FID signal of this GC run?",
        lambda f, m: g.number(round(f["fid_apex_rt_min"], 4), "min", abs_=0.02),
        "facts-analysis: fid_apex_rt_min (rainbow-api, black box)",
        stage_as="sample.D",
        answer_hint="a retention time with its unit",
    ),
    g.Spec(
        "ana-chrom-mwd-230-apex",
        MWD,
        "analysis",
        "This HPLC run recorded several UV wavelengths. At what retention time is the tallest peak of the "
        "230 nm signal?",
        lambda f, m: g.number(round(f["mwd_230nm_apex_rt_min"], 4), "min", abs_=0.02),
        "facts-analysis: mwd_230nm_apex_rt_min (rainbow-api, black box)",
        stage_as="sample.D",
        answer_hint="a retention time with its unit",
    ),
    g.Spec(
        "ana-chrom-dad-tallest-wavelength",
        "chromhandler-ca10-100um-d",
        "analysis",
        "Which of the recorded detection wavelengths shows the tallest peak (the highest absorbance anywhere "
        "in the run)?",
        lambda f, m: g.number(f["tallest_wavelength_nm"], "nm", abs_=0.5),
        "facts-analysis: tallest_wavelength_nm (rainbow-api, black box)",
        stage_as="sample.D",
        answer_hint="a wavelength in nm",
    ),
    g.Spec(
        "ana-chrom-gcms-tic-apex",
        "mtbls75-x-fsfa-hl-gc-o7c-1-d",
        "analysis",
        "At what retention time does the total ion chromatogram of this GC-MS run reach its maximum?",
        lambda f, m: g.number(round(f["tic_apex_rt_min"], 4), "min", abs_=0.02),
        "facts-analysis: tic_apex_rt_min (aston; rainbow-api agrees)",
        stage_as="sample.D",
        answer_hint="a retention time with its unit",
    ),
    # ------------------------------------------------------------ NMR
    g.Spec(
        "ana-nmr-bruker-tallest-peak",
        "nmrxiv-s846-50",
        "analysis",
        "In the processed 1H spectrum (processing number 1), at what chemical shift is the tallest peak?",
        lambda f, m: g.number(f["tallest_ppm"], None, abs_=0.01),
        "facts-analysis: tallest_ppm (nmrglue; NumPy on 1r agrees)",
        answer_hint="the chemical shift in ppm",
    ),
    g.Spec(
        "ana-nmr-bruker-aromatic-peak",
        "nmrxiv-s846-50",
        "analysis",
        "In the processed 1H spectrum (processing number 1), what is the chemical shift of the tallest peak "
        "between 7.5 and 8.5 ppm?",
        lambda f, m: g.number(f["tallest_ppm_7_5_to_8_5"], None, abs_=0.01),
        "facts-analysis: tallest_ppm_7_5_to_8_5 (nmrglue; NumPy on 1r agrees)",
        answer_hint="the chemical shift in ppm",
    ),
    g.Spec(
        "ana-nmr-bruker-13c-tallest",
        "nmrxiv-s596-1",
        "analysis",
        "In the processed 13C spectrum (processing number 1), at what chemical shift is the tallest peak?",
        lambda f, m: g.number(f["tallest_ppm"], None, abs_=0.05),
        "facts-analysis: tallest_ppm (nmrglue; NumPy on 1r agrees)",
        answer_hint="the chemical shift in ppm",
    ),
    # ------------------------------------------------------------ signal analysis
    g.Spec(
        "ana-nmr-bruker-fid-tallest",
        "nmrxiv-s846-50",
        "analysis",
        "This Bruker experiment holds only the raw 1H FID (no processed spectrum). Process it into a "
        "spectrum (Fourier transform, phase, ppm referencing). At what chemical shift is the tallest peak?",
        lambda f, m: g.number(f["tallest_ppm"], None, abs_=0.01),
        "facts-analysis: tallest_ppm (TopSpin's own pdata/1/1r of the same FID, read with nmrglue; NumPy agrees)",
        prepare={"remove": ["pdata"]},
        answer_hint="the chemical shift in ppm",
    ),
    g.Spec(
        "ana-nmr-bruker-fid-integral-ratio",
        "nmrxiv-s846-50",
        "analysis",
        "This Bruker experiment holds only the raw 1H FID. Process it into a spectrum and integrate two "
        "regions: 7.901-7.814 ppm and 8.239-8.178 ppm. What is the ratio of the first integral to the second?",
        lambda f, m: g.number(f["integral_ratio_2_to_1"], None, rel=0.01),
        "facts-analysis: integral_ratio_2_to_1 (TopSpin pdata/1/integrals.txt)",
        prepare={"remove": ["pdata"]},
        answer_hint="a ratio (a number)",
    ),
    g.Spec(
        "ana-nmr-jeol-fid-tallest",
        "nmrxiv-s200-qhnmr-jdf",
        "analysis",
        "This JEOL file holds a raw 1H FID. Process it into a spectrum (Fourier transform, phase, ppm "
        "referencing). At what chemical shift is the tallest peak?",
        lambda f, m: g.number(f["mestrenova_tallest_ppm"], None, abs_=0.01),
        "facts-analysis: mestrenova_tallest_ppm (MestReNova-processed JCAMP-DX of the same acquisition, read with "
        "nmrglue; jcamp agrees)",
        answer_hint="the chemical shift in ppm",
    ),
    g.Spec(
        "ana-ephys-abf-ap-count-sweep",
        "pyabf-171116sh-0018",
        "analysis",
        "How many action potentials fire in sweep 12 (counting sweeps from 1), over the whole sweep?",
        lambda f, m: g.integer(f["ap_count_sweep12"]),
        "facts-analysis: ap_count_sweep12 (eFEL on pyABF samples; NumPy 0 mV crossings agree)",
        answer_hint="a whole number",
    ),
    g.Spec(
        "ana-ephys-abf-rheobase",
        "pyabf-171116sh-0018",
        "analysis",
        "This current-clamp recording steps the injected current from sweep to sweep. What is the cell's "
        "rheobase (the smallest current step that makes it fire at least one action potential during the step)?",
        lambda f, m: g.number(f["rheobase_pa"], "pA", abs_=0.5),
        "facts-analysis: rheobase_pa (eFEL per sweep; pyABF epochs + NumPy agree)",
        answer_hint="a current with its unit",
    ),
    g.Spec(
        "ana-ephys-abf-input-resistance",
        "pyabf-171116sh-0018",
        "analysis",
        "What is this cell's input resistance? Use the hyperpolarising (negative) current steps: for each, "
        "the steady-state voltage (mean of the last 10 % of the step) minus the baseline (mean of the last "
        "10 % of the time before the step), and fit a straight line of that deflection against the injected "
        "current; the slope is the input resistance.",
        lambda f, m: g.number(round(f["input_resistance_mohm"], 2), "MOhm", rel=0.01),
        "facts-analysis: input_resistance_mohm (eFEL voltage_base / steady_state_voltage_stimend)",
        answer_hint="a resistance with its unit",
    ),
    g.Spec(
        "ana-ephys-abf-fsi-ap-count",
        "pyabf-2019-07-24-0055-fsi",
        "analysis",
        "How many action potentials does this fast-spiking cell fire in its last sweep (the whole sweep)?",
        lambda f, m: g.integer(f["ap_count_sweep17"]),
        "facts-analysis: ap_count_sweep17 (eFEL on pyABF samples; NumPy 0 mV crossings agree)",
        answer_hint="a whole number",
    ),
    g.Spec(
        "ana-ephys-abf1-rheobase",
        "pyabf-190619b-0003",
        "analysis",
        "The first channel of this recording is the membrane potential under current clamp, with the "
        "injected current stepped from sweep to sweep. What is the rheobase (the smallest current step that "
        "evokes at least one action potential during the step)?",
        lambda f, m: g.number(f["rheobase_pa"], "pA", abs_=0.5),
        "facts-analysis: rheobase_pa (eFEL per sweep; pyABF epochs + NumPy agree)",
        answer_hint="a current with its unit",
    ),
    g.Spec(
        "ana-ephys-blackrock-spike-count",
        "brk-filespec2-3001-ns5",
        "analysis",
        "Detect extracellular spikes on the first electrode channel (chan1) of this 30 kHz recording: "
        "band-pass 300-6000 Hz, threshold at 5 times the noise level (median absolute value / 0.6745 of "
        "the filtered signal), negative-going peaks, each the most negative point within 0.1 ms on either "
        "side. What is the spike rate over the whole recording?",
        lambda f, m: g.number(round(f["spike_rate_ch1_hz"], 3), "Hz", rel=0.01),
        "facts-analysis: spike_rate_ch1_hz (Neo + SpikeInterface detect_peaks)",
        answer_hint="a rate with its unit",
    ),
    # ------------------------------------------------------------ plates
    g.Spec(
        "ana-plate-gen5-top-well",
        "gen5-lum-endpoint",
        "analysis",
        "Which well gave the highest luminescence reading?",
        lambda f, m: g.string(f["top_well"]),
        "facts-analysis: top_well (allotropy)",
        answer_hint="the well, e.g. C7",
    ),
    g.Spec(
        "ana-plate-skanit-top-well",
        "skanit-luciferase",
        "analysis",
        "Which well gave the highest luminescence reading?",
        lambda f, m: g.string(f["top_well"]),
        "facts-analysis: top_well (allotropy)",
        answer_hint="the well, e.g. C7",
    ),
    g.Spec(
        "ana-plate-kaleido-column-mean",
        "kaleido-abs-endpoint",
        "analysis",
        "What is the mean absorbance of the wells in column 1 (A1 to H1)?",
        lambda f, m: g.number(f["column1_mean"], None, rel=0.005),
        "facts-analysis: column1_mean (allotropy)",
        answer_hint="the mean absorbance",
    ),
    # ------------------------------------------------------------ regressions of fixed bugs
    g.Spec(
        "ana-chrom-dad-210-apex",
        "chromhandler-ca10-100um-d",
        "analysis",
        "This HPLC run recorded a diode-array detector at several wavelengths. At what retention time is the "
        "highest point of the 210 nm signal?",
        lambda f, m: g.number(round(f["dad_210nm_apex_rt_min"], 4), "min", abs_=0.02),
        "facts-analysis: dad_210nm_apex_rt_min (rainbow-api, black box)",
        stage_as="sample.D",
        answer_hint="a retention time with its unit",
    ),
    g.Spec(
        "ana-chrom-mwd-210-window-apex",
        MWD,
        "analysis",
        "Look only at the part of the run between 6 and 8 minutes. At what retention time does the 210 nm "
        "signal reach its highest value in that window?",
        lambda f, m: g.number(round(f["mwd_210nm_apex_6_8_min"], 4), "min", abs_=0.02),
        "facts-analysis: mwd_210nm_apex_6_8_min (rainbow-api, black box)",
        stage_as="sample.D",
        answer_hint="a retention time with its unit",
    ),
    g.Spec(
        "ana-mic-czi-jpegxr-scene-red-mean",
        "openslide-zeiss-5-flat",
        "analysis",
        "This brightfield slide scan holds two scenes stored as compressed RGB tiles. What is the mean raw "
        "value of the red channel over the whole second scene (counting from 1)?",
        lambda f, m: g.number(f["scene2_red_mean"], None, rel=0.002),
        "facts-analysis: scene2_red_mean (czifile + imagecodecs; pylibCZIrw agrees)",
        answer_hint=HINT_RAW,
    ),
    g.Spec(
        "ana-mic-svs-jpeg2000-mean",
        "openslide-aperio-jp2k-33003-1",
        "analysis",
        "What is the mean pixel value of this whole-slide image at full resolution, averaged over all three "
        "colour channels (red, green and blue, 0-255)?",
        lambda f, m: g.number(f["level0_mean"], None, rel=0.002),
        "facts-analysis: level0_mean (tifffile + imagecodecs/OpenJPEG; OpenSlide agrees)",
        answer_hint="a number (0-255 scale)",
    ),
    g.Spec(
        "ana-mic-lif-saturated-pixels",
        "aics-s-1-t-4-c-2-z-1",
        "analysis",
        "Over all time points, how many pixels of the first channel are saturated, i.e. sit at the "
        "detector's full-scale value?",
        lambda f, m: g.integer(f["c1_pixels_at_4095"]),
        "facts-analysis: c1_pixels_at_4095 (liffile; readlif agrees)",
        answer_hint="the number of pixels",
    ),
    g.Spec(
        "ana-mic-lif-brightest-band-center",
        "zenodo14976703-Convalaria-LambdaScan",
        "analysis",
        "This is a spectral (lambda) scan: one image per emission band. Which band is brightest, i.e. has the "
        "highest mean raw intensity? Give the centre wavelength of that band's detection window in nm.",
        lambda f, m: g.number(f["brightest_band_center_nm"], "nm", abs_=1),
        "facts-analysis: brightest_band_center_nm (liffile; readlif agrees)",
        answer_hint="a wavelength in nm",
    ),
    g.Spec(
        "ana-plate-gen5-normalized-above",
        "gen5-lum-endpoint",
        "analysis",
        "Besides the luminescence readings, this export holds values the reader software normalized. How "
        "many wells have a normalized value above 100?",
        lambda f, m: g.integer(f["normlum_wells_above_100"]),
        "facts-analysis: normlum_wells_above_100 (allotropy)",
        answer_hint="the number of wells",
    ),
    g.Spec(
        "ana-plate-gen5-column-mean",
        "gen5-lum-endpoint",
        "analysis",
        "What is the mean measured luminescence (as read by the instrument, not a normalized value) of the "
        "wells in column 12 (A12 to H12)?",
        lambda f, m: g.number(f["column12_mean"], "RLU", rel=0.005),
        "facts-analysis: column12_mean (allotropy)",
        answer_hint="the mean luminescence",
    ),
    # ------------------------------------------------------------ high-content screening plates
    g.Spec(
        "ana-hcs-harmony-dapi-mean",
        "hcs-harmony-idr0034-folder",
        "analysis",
        "This folder is a high-content screening plate exported from the imager (an index file plus one "
        "image file per plane). What is the mean raw intensity of the DAPI channel in well A01, field 1 "
        "(counting fields from 1 as the index numbers them)?",
        lambda f, m: g.number(f["dapi_mean_a01_f1"], None, rel=0.005),
        "facts-analysis: dapi_mean_a01_f1 (Index.idx.xml via the standard library; tifffile)",
        stage_as="sample_plate",
        answer_hint=HINT_RAW,
    ),
    g.Spec(
        "ana-hcs-imagexpress-brightest-well",
        "hcs-imagexpress-idr0081-folder",
        "analysis",
        "Only some wells of this ImageXpress plate were copied into this folder. Among the wells whose "
        "DAPI image is present, which well has the highest mean DAPI intensity?",
        lambda f, m: only(f["brightest_dapi_well"], ["A01", "A02"]),
        "facts-analysis: brightest_dapi_well (HTD via the standard library; tifffile)",
        stage_as="sample_plate",
        answer_hint="the well, e.g. B07",
    ),
    g.Spec(
        "ana-hcs-cellvoyager-well-mean",
        "hcs-cellvoyager-jump-1053601756-folder",
        "analysis",
        "This CellVoyager plate folder holds only some of the plate's images. What is the mean raw "
        "Hoechst intensity in well A01, over every pixel of the Hoechst images of that well present here?",
        lambda f, m: g.number(f["hoechst_mean_a01"], None, rel=0.005),
        "facts-analysis: hoechst_mean_a01 (MeasurementData.mlf and .mes via the standard library; tifffile)",
        stage_as="sample_plate",
        answer_hint=HINT_RAW,
    ),
    g.Spec(
        "ana-hcs-imagexpress-brightest-channel",
        "hcs-imagexpress-jump-a1170383-folder",
        "analysis",
        "This folder holds ImageXpress images of one well of a Cell Painting plate (no plate description "
        "file). In well A01, site 1, which channel has the highest mean raw intensity? Name the channel.",
        lambda f, m: only(f["brightest_channel_a01_s1"], ["Cy5", "Texas Red", "Cy3", "FITC", "DAPI"]),
        "facts-analysis: brightest_channel_a01_s1 (MetaSeries metadata and pixels via tifffile)",
        stage_as="sample_plate",
        answer_hint="the channel name",
    ),
]
ANALYSIS_SPECS += __import__("qpcr_facts").analysis_specs(g)
ANALYSIS_SPECS += __import__("heldout_report_fixes").specs(g)  # second held-out report's fixes

# ------------------------------------------------------------ held-out finding regressions (2026-09-24)
ANALYSIS_SPECS += [
    g.Spec(
        "ana-ms-raw-exploris-ms2-base-peak",
        "mtbls14508-mh-b4-c18-pos-dda-r01",
        "analysis",
        "This run is from an Orbitrap Exploris 480. What is the m/z of the most intense peak in the 500th "
        "MS/MS spectrum (counting MS/MS scans only, from the start)?",
        lambda f, m: g.number(round(f["ms2_500_base_peak_mz"], 4), None, abs_=0.005),
        "facts-analysis: ms2_500_base_peak_mz (pyteomics on the depositor's mzML; base-peak cvParam agrees)",
        answer_hint="the m/z value",
    ),
    g.Spec(
        "ana-ms-raw-exploris120-scan-peaks",
        "mtbls13930-neg-sqc-5",
        "analysis",
        "Scan number 21 of this Orbitrap Exploris 120 run is stored as a profile spectrum together with the "
        "centroid peak list the instrument software computed from it. How many peaks (m/z, intensity pairs) "
        "are in that centroid list? Count every stored peak.",
        lambda f, m: g.integer(f["scan_21_peak_count"]),
        "facts-analysis: scan_21_peak_count (pyteomics on the depositor's mzXML; `peaksCount` agrees)",
        answer_hint="the number of peaks",
    ),
    g.Spec(
        "ana-plate-icontrol-csv-top-well",
        "tecan-icontrol-csv-kinetic-wellr",
        "analysis",
        "This Tecan kinetic run measured absorbance and luminescence every cycle. Which well had the highest "
        "absorbance at the last cycle?",
        lambda f, m: g.string(f["od_last_cycle_top_well"]),
        "facts-analysis: od_last_cycle_top_well (pandas on the CSV text; the csv module agrees)",
        answer_hint="the well, e.g. C7",
    ),
    g.Spec(
        "ana-plate-icontrol-csv-lum-onset",
        "tecan-icontrol-csv-kinetic-wellr",
        "analysis",
        "In this Tecan kinetic run, at which cycle (counting cycles from 1, as the file numbers them) does the "
        "luminescence of well A7 first exceed 100,000 RLU?",
        lambda f, m: g.integer(f["a7_lum_first_cycle_above_100000"]),
        "facts-analysis: a7_lum_first_cycle_above_100000 (pandas on the CSV text)",
        answer_hint="the cycle number",
    ),
    g.Spec(
        "ana-plate-spark-csv-saturated-wells",
        "tecan-sparkcontrol-csv-endpoint-flopr",
        "analysis",
        "This Tecan Spark export reads a fluorescein calibration plate at several detector gains. How many "
        "wells were saturated (no numeric value, reported as over range) in the read at gain 100?",
        lambda f, m: g.integer(f["gain_100_over_wells"]),
        "facts-analysis: gain_100_over_wells (the csv module on the CSV text)",
        answer_hint="the number of wells",
    ),
    g.Spec(
        "ana-plate-spark-csv-od-max",
        "tecan-sparkcontrol-csv-kinetic-flopr",
        "analysis",
        "In this Tecan Spark kinetic run, what was the highest OD600 reached by well B1?",
        lambda f, m: g.number(f["od600_b1_max"], None, rel=0.001),
        "facts-analysis: od600_b1_max (pandas on the CSV text)",
        answer_hint="the OD value",
    ),
]

# plate analysis (`openreadout analyze assay`): evals/assay_facts.py
ANALYSIS_SPECS += assay_facts.specs()


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true", help="exit 1 if evals/facts/analysis.json is out of date")
    args = ap.parse_args()
    text = render(compute())
    if args.check:
        if not OUT.exists() or OUT.read_text(encoding="utf-8") != text:
            print("evals/facts/analysis.json is out of date; run evals/analysis.py", file=sys.stderr)
            return 1
        print("evals/facts/analysis.json is up to date")
        return 0
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(text, encoding="utf-8")
    print(f"wrote {OUT.relative_to(ROOT)}: {len(FACTS)} facts")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
