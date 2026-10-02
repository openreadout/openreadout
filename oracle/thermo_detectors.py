"""LC-detector data (UV/PDA) in a third party's mzML conversion of a Thermo .raw file, as
ground truth for openreadout's detector traces (see docs/provenance/thermo-raw.md).

No reader of the .raw is run. The mzML (made by someone else with ProteoWizard msconvert) is
read with pyteomics (Apache-2.0):

- spectra whose native id starts `controllerType=N` with N != 0 are detector spectra (a PDA's
  absorbance spectra: a `wavelength array` and an `intensity array`); they are summarized here
  and left out of the mass spectra;
- chromatograms other than the TIC/BPC (`UV 1`, `PDA 1`, ...) are summarized with their time
  grid and value hashes.

Hashes: PDA intensities are the stored integers (the export writes them as float32 of whole
numbers), hashed as int32 little-endian, spectrum after spectrum, wavelength by wavelength
(`xxh3_i32`). Chromatogram values are hashed the same way when every value is a whole number,
else as float32 (`xxh3_f32`). Times are summarized as a regular grid: first, step (last minus
first over n - 1) and the largest departure of any time from that grid (minutes).
"""

import re

import numpy as np
import xxhash


def is_detector_id(native_id) -> bool:
    m = re.match(r"controllerType=(\d+)", native_id or "")
    return bool(m) and m.group(1) != "0"


def has_detector_ids(q) -> bool:
    """Whether the mzML holds any spectrum of a non-MS controller (a quick byte search)."""
    import mmap

    with open(q, "rb") as fh, mmap.mmap(fh.fileno(), 0, access=mmap.ACCESS_READ) as mm:
        return re.search(rb'id="controllerType=[1-9]', mm) is not None


def _grid(t) -> dict:
    t = np.asarray(t, dtype=np.float64)
    n = int(t.size)
    if n == 0:
        return {"n": 0}
    step = float((t[-1] - t[0]) / (n - 1)) if n > 1 else 0.0
    resid = float(np.max(np.abs(t - (t[0] + step * np.arange(n))))) if n > 1 else 0.0
    return {"n": n, "first_min": float(t[0]), "last_min": float(t[-1]), "step_min": step,
            "max_departure_min": resid}


def _values(v) -> dict:
    v = np.asarray(v, dtype=np.float64)
    whole = bool(v.size) and bool(np.all(v == np.round(v))) and float(np.max(np.abs(v))) < 2**31
    out = {
        "n": int(v.size),
        "sum": float(v.sum()),
        "min": float(v.min()) if v.size else None,
        "max": float(v.max()) if v.size else None,
        "argmax": int(v.argmax()) if v.size else None,
        "first": [float(x) for x in v[:8]],
        "whole_numbers": whole,
    }
    if whole:
        out["xxh3_i32"] = xxhash.xxh3_128_hexdigest(v.astype("<i4").tobytes())
    else:
        out["xxh3_f32"] = xxhash.xxh3_128_hexdigest(v.astype("<f4").tobytes())
    return out


def detector_export(q) -> dict:
    """Summary of every detector spectrum and non-MS chromatogram of the mzML `q`."""
    from pyteomics import mzml

    groups = {}
    with mzml.MzML(str(q), decode_binary=True) as f:
        for sp in f:
            nid = sp.get("id")
            if not is_detector_id(nid):
                continue
            key = nid.rsplit(" scan=", 1)[0]
            g = groups.setdefault(key, {"ids": [], "t": [], "wl": None, "rows": [], "tic": []})
            scan = sp.get("scanList", {}).get("scan", [{}])[0]
            g["ids"].append(nid)
            g["t"].append(float(scan.get("scan start time")))
            wl = np.asarray(sp.get("wavelength array", []), dtype=np.float64)
            if g["wl"] is None:
                g["wl"] = wl
            elif wl.size != g["wl"].size or np.any(wl != g["wl"]):
                raise ValueError(f"{nid}: wavelength grid changes")
            g["rows"].append(np.asarray(sp.get("intensity array", []), dtype=np.float64))
            g["tic"].append(float(sp.get("total ion current", np.nan)))
        spectra = []
        for key, g in groups.items():
            m = np.vstack(g["rows"])
            nums = [int(re.search(r"scan=(\d+)", i).group(1)) for i in g["ids"]]
            wl = g["wl"]
            sample = list(range(0, m.shape[0], 500)) + [m.shape[0] - 1]
            spectra.append({
                "controller": key,
                "spectrum_count": int(m.shape[0]),
                "scan_numbers_consecutive": nums == list(range(nums[0], nums[0] + len(nums))),
                "first_scan_number": nums[0],
                "wavelengths_nm": {"n": int(wl.size), "first": float(wl[0]), "last": float(wl[-1]),
                                   "step": float((wl[-1] - wl[0]) / (wl.size - 1)) if wl.size > 1 else 0.0},
                "times": _grid(g["t"]),
                "intensity": _values(m.ravel()),
                "spectrum_sums": _values(m.sum(axis=1)),
                # the export's per-spectrum "total ion current" (the file's stored total)
                "stored_totals": _values(np.asarray(g["tic"])),
                # every 500th spectrum in full (row index, values), for readable diffs
                "sample_spectra": [[int(i), [float(x) for x in m[i]]] for i in sample],
            })
        chroms = []
        for c in f.iterfind("chromatogram"):
            cid = c.get("id")
            kind = next((k for k in ("emission chromatogram", "absorption chromatogram") if k in c), None)
            if kind is None:  # TIC, BPC, SRM: mass-spectrometer chromatograms
                continue
            t = np.asarray(c.get("time array", []), dtype=np.float64)
            v = np.asarray(c.get("intensity array", []), dtype=np.float64)
            chroms.append({"id": cid, "kind": kind, "times": _grid(t), "values": _values(v)})
    return {"detector_spectra": spectra, "chromatograms": chroms}
