"""Vibrational spectroscopy: a second reader or the vendor's own export for each file.

- Thermo OMNIC .spa: OMNIC's CSV export of the same spectrum where the depositor gave one (the
  Toffolo mineral library ships a `.CSV` beside each `.SPA`): the absorbance values (count,
  first values, statistics) and the wavenumber range. The primary oracle is SpectroChemPy.
- Bruker OPUS: brukeropusreader (GPL-3.0, run as a black box; the primary oracle is brukeropus):
  the parameter blocks (sample, optics, acquisition, Fourier transform, instrument) and the
  result spectrum's data parameters (points, first/last wavenumber, date and time with zone),
  set against our normalized fields and `extra`.
- JASCO .jws already have two second opinions (the Spectra Manager export and jws2txt;
  oracle/jasco_oracle.py); Renishaw .wdf and PerkinElmer .sp have no second reader available
  offline (docs/benchmark/second-opinions.md).
"""
from __future__ import annotations

import csv
import re
from datetime import datetime, timedelta, timezone
from importlib.metadata import version
from pathlib import Path

import numpy as np

from . import Rec, trace_values

FAMILY = "vibrational-spectroscopy"
FORMATS = {"thermo-omnic", "bruker-opus", "galactic-spc"}

RESULT_BLOCKS = ("AB", "TR", "KM", "RAM", "EMI", "RFL", "LRF", "ATR", "PAS")


def omnic(rec: Rec, path: Path) -> None:
    twin = path.with_suffix(".CSV")
    if not twin.exists():
        twin = path.with_suffix(".csv")
    if not twin.exists() or path.suffix.lower() != ".spa":
        return
    rows = []
    with twin.open(newline="", encoding="latin-1") as f:
        for row in csv.reader(f):
            try:
                rows.append((float(row[0]), float(row[1])))
            except (ValueError, IndexError):
                continue
    if not rows:
        return
    r = rec.reader(f"OMNIC's CSV export of the same spectrum ({twin.name}), read with Python's csv module")
    a = np.array(rows)
    # the export lists ascending wavenumbers; the file (and ours) runs from high to low
    if a[0, 0] < a[-1, 0]:
        a = a[::-1]
    # OMNIC's CSV export adds a point one step below the spectrum's low end with absorbance 0
    # (the SPA header's point count is one less; on every Toffolo file): left out here
    step = a[-2, 0] - a[-3, 0] if a.shape[0] > 3 else 0
    if a.shape[0] > 3 and a[-1, 1] == 0.0 and abs((a[-1, 0] - a[-2, 0]) - step) < 1e-3 * abs(step):
        rec.note("the export's last row (one step below the spectrum, absorbance 0) is export padding; left out")
        a = a[:-1]
    rec.check("points", "/traces/0/sample_count", int(a.shape[0]), reader=r, required=True)
    rec.check("first wavenumber", "/traces/0/extra/axis/first", float(a[0, 0]), cmp="num", abs=5e-4, reader=r)
    rec.check("last wavenumber", "/traces/0/extra/axis/last", float(a[-1, 0]), cmp="num", abs=5e-4, reader=r)
    # the export writes 7 significant digits
    trace_values(rec, 0, 0, 0, a[:, 1], r, rel=6e-7, abs=6e-7 * float(np.max(np.abs(a[:, 1]))), n_samples=64)


def _opus_time(dat: str | None, tim: str | None) -> str | None:
    """DAT `03/05/2019` (dd/mm/yyyy) and TIM `13:34:44.641 (GMT-4)` → ISO with the offset."""
    if not dat or not tim:
        return None
    m = re.match(r"(\d{1,2}):(\d{2}):(\d{2})(\.\d+)?\s*(?:\(GMT([+-]\d+(?::?\d{2})?)\))?", tim.strip())
    d = re.match(r"(\d{1,2})/(\d{1,2})/(\d{4})", dat.strip())
    if not m or not d:
        return None
    day, mon, yr = int(d.group(1)), int(d.group(2)), int(d.group(3))
    frac = m.group(4) or ""
    base = f"{yr:04d}-{mon:02d}-{day:02d}T{int(m.group(1)):02d}:{m.group(2)}:{m.group(3)}{frac}"
    z = m.group(5)
    if z is None:
        return base
    sign = "-" if z.startswith("-") else "+"
    z = z.lstrip("+-").replace(":", "")
    hh, mm = (int(z[:-2]), int(z[-2:])) if len(z) > 2 else (int(z), 0)
    return f"{base}{sign}{hh:02d}:{mm:02d}"


def opus(rec: Rec, path: Path) -> None:
    from brukeropusreader import read_file
    import contextlib
    import io
    import warnings
    with warnings.catch_warnings(), contextlib.redirect_stdout(io.StringIO()):
        warnings.simplefilter("ignore")
        d = read_file(str(path))
    r = rec.reader("brukeropusreader 1.3 (GPL-3.0, run as a black box): parameter blocks")
    ex = "/traces/0/extra"

    def block(name):
        v = d.get(name)
        return v if isinstance(v, dict) else {}
    sample, optik, acq = block("Sample"), block("Optik"), block("Acquisition")
    ft, ins = block("Fourier Transformation"), block("Instrument")
    text = dict(cmp="text", reader=r)
    num = dict(cmp="num", rel=1e-9, reader=r)
    rec.check("Sample SNM", f"{ex}/sample_name", sample.get("SNM"), **text)
    rec.check("Sample CNM", f"{ex}/operator", sample.get("CNM"), **text)
    rec.check("Sample EXP", f"{ex}/experiment", sample.get("EXP"), **text)
    rec.check("Sample SFM", f"{ex}/sample_form", sample.get("SFM"), **text)
    for k, field in (("SRC", "source"), ("BMS", "beamsplitter"), ("DTC", "detector"), ("APT", "aperture"),
                     ("ACC", "accessory"), ("CHN", "measurement_channel")):
        rec.check(f"Optik {k}", f"{ex}/{field}", optik.get(k), **text)
    if isinstance(acq.get("RES"), (int, float)):
        rec.check("Acquisition RES", f"{ex}/resolution_cm1", float(acq["RES"]), **num)
    if isinstance(acq.get("NSS"), (int, float)):
        rec.check("Acquisition NSS", f"{ex}/scans", int(acq["NSS"]), reader=r)
    for k, field in (("HFW", "range_high_cm1"), ("LFW", "range_low_cm1")):
        if isinstance(acq.get(k), (int, float)):
            rec.check(f"Acquisition {k}", f"{ex}/{field}", float(acq[k]), **num)
    rec.check("Fourier Transformation APF", f"{ex}/apodization", ft.get("APF"), **text)
    rec.check("Fourier Transformation PHZ", f"{ex}/phase_correction", ft.get("PHZ"), **text)
    if isinstance(ft.get("PHR"), (int, float)):
        rec.check("Fourier Transformation PHR", f"{ex}/phase_resolution_cm1", float(ft["PHR"]), **num)
    if ft.get("ZFF") not in (None, ""):
        try:
            rec.check("Fourier Transformation ZFF", f"{ex}/zero_filling_factor", float(ft["ZFF"]), **num)
        except ValueError:
            pass
    if isinstance(ins.get("LWN"), (int, float)):
        rec.check("Instrument LWN", f"{ex}/laser_wavenumber_cm1", float(ins["LWN"]), **num)
    rec.check("Instrument INS", f"{ex}/instrument", ins.get("INS"), **text)
    rec.check("Instrument SRN", f"{ex}/instrument_serial", ins.get("SRN"), **text)
    result = next((b for b in RESULT_BLOCKS if isinstance(d.get(f"{b} Data Parameter"), dict)), None)
    if result is None:
        return
    rec.check("result spectrum block", f"{ex}/result_spectrum", result, **text)
    dp = d[f"{result} Data Parameter"]
    if isinstance(dp.get("NPT"), (int, float)):
        rec.check(f"{result} NPT", "/traces/0/sample_count", int(dp["NPT"]), reader=r)
    for k, field in (("FXV", "axis/first"), ("LXV", "axis/last")):
        if isinstance(dp.get(k), (int, float)):
            rec.check(f"{result} {k}", f"{ex}/{field}", float(dp[k]), cmp="num", rel=1e-9, reader=r)
    t = _opus_time(dp.get("DAT"), dp.get("TIM"))
    if t:
        rec.check(f"{result} DAT/TIM", "/experiment/acquisition/started_at", t, cmp="time", zone="(GMT" in str(dp.get("TIM")),
                  abs=0.002, reader=r)


def omnic_scp(rec: Rec, path: Path) -> None:
    """SpectroChemPy (CeCILL-B, run as a black box): `read_omnic`'s acquisition date, written in
    the machine's zone with its offset; compared as an instant."""
    import contextlib
    import io
    import logging
    import warnings
    logging.disable(logging.WARNING)
    with warnings.catch_warnings(), contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        warnings.simplefilter("ignore")
        import spectrochempy as scp
        d = scp.read_omnic(str(path))
    t = getattr(d, "acquisition_date", None)
    if isinstance(t, str):
        t = datetime.fromisoformat(t.strip())
    if t is not None and t.year > 1980:
        r = rec.reader(f"SpectroChemPy {version('spectrochempy')} (CeCILL-B, run as a black box) read_omnic")
        rec.check("acquisition_date", "/experiment/acquisition/started_at", t.isoformat(), cmp="time", abs=1.0,
                  reader=r)


def spc(rec: Rec, path: Path) -> None:
    """spc_io (MIT): the SPC header's packed date (`fdate`); no seconds are stored."""
    import spc_io
    with path.open("rb") as fh:
        s = spc_io.SPC.from_bytes_io(fh)
    t = getattr(s, "date", None)
    if t is not None and 1980 < t.year < 2100:
        r = rec.reader(f"spc_io {version('spc_io')} (MIT) header date")
        rec.check("header date (fdate)", "/experiment/acquisition/started_at", t.isoformat(), cmp="wallclock",
                  reader=r)


def run(rec: Rec, entry: dict, path: Path, ctx) -> None:
    if entry["format"] == "galactic-spc":
        return spc(rec, path)
    if entry["format"] == "thermo-omnic" and path.suffix.lower() in (".spa", ".spg"):
        try:
            omnic_scp(rec, path)
        except Exception as e:  # SpectroChemPy cannot read the file: the other checks stand
            rec.note(f"spectrochempy: {type(e).__name__}")
    {"thermo-omnic": omnic, "bruker-opus": opus}[entry["format"]](rec, path)
