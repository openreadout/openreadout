"""Microscopy and other image formats: Bio-Formats 8.5 (GPL, `showinf -nopix -omexml`, run as a
black box) on the file's OME-XML metadata: the acquisition date (the
earliest image's `AcquisitionDate`) and the microscope model, set against the normalized
`experiment.acquisition.started_at` and `experiment.instrument.model`, plus the first image's
dimensions.

The primary oracles of these formats (czifile, the nd2 library, liffile, oirfile, tifffile,
mrcfile, dm3_lib, h5py) compare pixels; oracle/metadata_compare.py compares more fields for six
formats but is not part of the corpus test. Bio-Formats writes `AcquisitionDate` without a
zone, some readers the local clock and some UTC: compared as the same wall clock or ours in UTC.
Its date is the first image's (plane's) capture, which follows the start of the measurement that
we report by a few seconds on screening plates and some ZVI/MetaMorph files: agreement within
60 s (the check is for the date, the clock and the zone, not the definition of "start").
"""
from __future__ import annotations

import os
import sys
from pathlib import Path

from importlib.metadata import version

from . import Rec

FAMILY = "imaging"
FORMATS = {"nd2", "lif", "oir", "vsi", "zvi", "oib", "oif", "dcimg", "dm", "mrc", "ser", "emd", "ims", "tiff",
           "biorad-scn", "opera-harmony", "cellvoyager", "imagexpress"}
MAX_BYTES = 2 << 30


def nd2_date(rec: Rec, path: Path) -> None:
    """The nd2 library (BSD-3): the text info's `date`, the local clock NIS-Elements wrote in the
    Windows short-date format of the machine (`9/28/2021  9:34:47 AM`, `29/12/2010  12:13:08`).
    Ours is the stored start (UTC): compared modulo the zone offset, within 3 min (the text date
    is written when the document is, a few seconds to minutes after the first frame). Dates whose
    day and month cannot be told apart are skipped."""
    import re
    from datetime import datetime
    import nd2
    with nd2.ND2File(str(path)) as f:
        d = (f.text_info or {}).get("date")
    if not d:
        return
    m = re.match(r"\s*(\d{1,2})[/.-](\d{1,2})[/.-](\d{4})\s+(\d{1,2}):(\d{2}):(\d{2})\s*([AP]M)?", d)
    if not m:
        return
    a, b, y, hh, mi, ss, ap = m.groups()
    a, b, hh = int(a), int(b), int(hh)
    if a > 12:
        day, month = a, b
    elif b > 12:
        day, month = b, a
    elif a == b:
        day = month = a
    else:
        return
    if ap:
        hh = hh % 12 + (12 if ap == "PM" else 0)
    t = datetime(int(y), month, day, hh, int(mi), int(ss)).isoformat()
    r = rec.reader(f"nd2 {version('nd2')} (BSD-3) text_info date")
    rec.check("text info date (local clock)", "/experiment/acquisition/started_at", t, cmp="time_mod_zone", abs=180.0,
              reader=r)


def olympus(rec: Rec, path: Path) -> None:
    import oiffile
    with oiffile.OifFile(str(path)) as o:
        ms = o.mainfile
    acq = ms.get("Acquisition Parameters Common", {})
    ver = ms.get("Version Info", {})
    r = rec.reader(f"oiffile {version('oiffile')} (BSD-3) main settings")
    t = acq.get("ImageCaputreDate")
    if t:
        ms_ = acq.get("ImageCaputreDate+MilliSec")
        iso = str(t).strip().replace(" ", "T") + (f".{int(ms_):03d}" if ms_ is not None else "")
        rec.check("ImageCaputreDate (+MilliSec)", "/experiment/acquisition/started_at", iso, cmp="time", abs=0.001,
                  reader=r)
    if ver.get("SystemName"):
        rec.check("Version Info SystemName", "/experiment/instrument/model", str(ver["SystemName"]), cmp="text",
                  reader=r)


def imaris(rec: Rec, path: Path) -> None:
    import h5py
    with h5py.File(path, "r") as h:
        rd = h["DataSetInfo/Image"].attrs.get("RecordingDate") if "DataSetInfo/Image" in h else None
    if rd is None:
        return
    t = b"".join(rd).decode("latin-1").strip()
    if t and not t.startswith("0000"):
        r = rec.reader(f"h5py {version('h5py')} (BSD-3): DataSetInfo/Image RecordingDate")
        rec.check("RecordingDate", "/experiment/acquisition/started_at", t.replace(" ", "T"), cmp="time", abs=0.001,
                  reader=r)


EXTRA = {"nd2": nd2_date, "oib": olympus, "oif": olympus, "ims": imaris}


def run(rec: Rec, entry: dict, path: Path, ctx) -> None:
    extra = EXTRA.get(entry["format"])
    if extra is not None:
        try:
            extra(rec, path)
        except Exception as e:  # the format's own reader cannot read it: Bio-Formats still runs
            rec.note(f"{extra.__name__}: {type(e).__name__}: {e}")
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
    import metadata_compare as mc  # noqa: E402  (the Bio-Formats OME-XML summarizer)
    size = sum(f.stat().st_size for f in path.rglob("*") if f.is_file()) if path.is_dir() else path.stat().st_size
    if size > MAX_BYTES or os.environ.get("SECOND_NO_BF"):
        return
    images = mc.bf(path)
    if not images:
        return
    r = rec.reader("Bio-Formats 8.5.0 (GPL, showinf -omexml, run as a black box)")
    dates = sorted(i["acquired"] for i in images if i.get("acquired"))
    if dates:
        rec.check("acquisition date (earliest image)", "/experiment/acquisition/started_at", dates[0],
                  cmp="time_or_utc", abs=60.0, reader=r)
    models = [i["microscope"] for i in images if i.get("microscope")]
    if models:
        rec.check("microscope model", "/experiment/instrument/model", models[0], cmp="text", reader=r)
    i0 = images[0]
    for k in ("size_x", "size_y"):
        if i0.get(k):
            rec.check(f"image 0 {k}", f"/images/0/{k}", int(i0[k]), reader=r)
